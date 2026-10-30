mod drm;
mod winit;

#[cfg(test)]
mod capture_tests;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use blair_integration::EventChannel;
use smithay::{
    backend::renderer::{gles::GlesRenderer, ImportDma},
    reexports::{
        calloop::{
            self,
            generic::Generic,
            timer::{TimeoutAction, Timer},
            EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
        },
        wayland_server::Display,
    },
    wayland::socket::ListeningSocketSource,
};

use crate::{
    config::CompositorConfig,
    integrations::Integrations,
    state::{BlairState, ClientState},
};

const SUPERVISE_INTERVAL: Duration = Duration::from_secs(1);
/// Frames to wait for presentation feedback before assuming it was lost.
const MISSED_FRAME_INTERVALS: u32 = 10;

/// Paces repaints so the compositor never renders faster than the output
/// refresh and never spins when clients keep committing unchanged content.
struct FramePacer {
    interval: Duration,
    redraw_pending: bool,
    frame_pending: bool,
    last_frame: Instant,
    wakeup: Option<(Instant, RegistrationToken)>,
}

impl FramePacer {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            redraw_pending: true,
            frame_pending: false,
            last_frame: Instant::now() - interval,
            wakeup: None,
        }
    }

    /// Returns true when a frame is due now.
    fn poll(&mut self, state: &mut BlairState) -> bool {
        if state.take_redraw_request() {
            self.redraw_pending = true;
        }
        if self.frame_pending {
            let elapsed = self.last_frame.elapsed();
            let timeout = self.interval * MISSED_FRAME_INTERVALS;
            if elapsed < timeout {
                self.schedule_wakeup(state, timeout - elapsed);
                return false;
            }
            tracing::warn!("no presentation feedback for the queued frame, repainting anyway");
            self.frame_pending = false;
        }
        if !self.redraw_pending {
            return false;
        }
        let elapsed = self.last_frame.elapsed();
        if elapsed < self.interval {
            self.schedule_wakeup(state, self.interval - elapsed);
            return false;
        }
        self.cancel_wakeup(state);
        self.redraw_pending = false;
        self.last_frame = Instant::now();
        true
    }

    /// Keeps the loop waking while something is animating.
    fn keep_awake(&mut self, state: &mut BlairState, animating: bool) {
        if !animating {
            return;
        }
        self.redraw_pending = true;
        self.schedule_wakeup(state, self.interval);
    }

    fn frame_queued(&mut self, state: &mut BlairState) {
        self.frame_pending = true;
        self.schedule_wakeup(state, self.interval * MISSED_FRAME_INTERVALS);
    }

    /// The display paced us, so the next repaint may start right away.
    fn frame_presented(&mut self, state: &BlairState) {
        self.cancel_wakeup(state);
        self.frame_pending = false;
        self.last_frame = Instant::now() - self.interval;
    }

    fn force_redraw(&mut self, state: &BlairState) {
        self.cancel_wakeup(state);
        self.redraw_pending = true;
        self.frame_pending = false;
        self.last_frame = Instant::now() - self.interval;
    }

    fn cancel_wakeup(&mut self, state: &BlairState) {
        if let Some((_, token)) = self.wakeup.take() {
            state.loop_handle.remove(token);
        }
    }

    fn schedule_wakeup(&mut self, state: &mut BlairState, delay: Duration) {
        let now = Instant::now();
        let at = now + delay;
        if self
            .wakeup
            .is_some_and(|(scheduled, _)| scheduled > now && scheduled <= at)
        {
            return;
        }
        self.cancel_wakeup(state);
        let result = state.loop_handle.insert_source(
            Timer::from_deadline(at),
            |_, _, state: &mut BlairState| {
                state.request_redraw();
                TimeoutAction::Drop
            },
        );
        match result {
            Ok(token) => self.wakeup = Some((at, token)),
            Err(error) => tracing::warn!(%error, "failed to schedule the next repaint"),
        }
    }
}

pub fn run(config: CompositorConfig) -> Result<()> {
    let in_display =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();

    match config.general.backend.as_str() {
        "drm" | "kms" => {
            tracing::info!("config requested DRM/KMS backend");
            drm::run(config)
        }
        "winit" | "nested" => {
            tracing::info!("config requested winit backend (nested)");
            winit::run(config)
        }
        "auto" if in_display => {
            tracing::info!("parent display detected — using winit backend (nested)");
            winit::run(config)
        }
        "auto" => {
            tracing::info!("no parent display — using DRM/KMS backend");
            drm::run(config)
        }
        other => {
            tracing::warn!(backend = other, "unknown backend, falling back to auto");
            if in_display {
                winit::run(config)
            } else {
                drm::run(config)
            }
        }
    }
}

/// Wakes the loop once so the first frame is painted even when no client
/// has connected yet.
fn schedule_first_frame(handle: &LoopHandle<'static, BlairState>) {
    let result = handle.insert_source(Timer::immediate(), |_, _, state: &mut BlairState| {
        state.request_redraw();
        TimeoutAction::Drop
    });
    if let Err(error) = result {
        tracing::warn!(%error, "failed to schedule the first frame");
    }
}

/// Inserts the Wayland socket and client dispatch sources, returning the
/// socket name to advertise to clients.
fn init_wayland(
    handle: &LoopHandle<'static, BlairState>,
    display: Display<BlairState>,
) -> Result<String> {
    let source = ListeningSocketSource::new_auto().context("failed to bind Wayland socket")?;
    let socket_name = source.socket_name().to_string_lossy().into_owned();
    handle
        .insert_source(source, |stream, _, state: &mut BlairState| {
            if let Err(error) = state
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                tracing::warn!(%error, "failed to insert Wayland client");
            }
        })
        .map_err(|error| anyhow::anyhow!("wayland socket source: {error}"))?;

    handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state: &mut BlairState| {
                profiling::scope!("dispatch_clients");
                // SAFETY: the display is only accessed from this callback.
                unsafe { display.get_mut() }.dispatch_clients(state)?;
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| anyhow::anyhow!("wayland display source: {error}"))?;

    tracing::info!(socket = %socket_name, "Wayland socket ready");
    Ok(socket_name)
}

/// Starts the enabled integrations. Their threads ping the event loop so
/// requests are handled without polling.
fn init_integrations(
    handle: &LoopHandle<'static, BlairState>,
    config: &CompositorConfig,
    environment: Vec<(String, String)>,
) -> Result<(Arc<dyn EventChannel>, Option<blair_integration_dbus::ScreencopyAuth>)> {
    let (ping, source) =
        calloop::ping::make_ping().context("failed to create the integration wake-up source")?;
    let mut integrations = Integrations::start(
        config.integrations.dbus,
        Arc::new(move || ping.ping()),
        environment,
    );
    let events = integrations.event_channel();
    let auth = integrations.screencopy_auth();
    handle
        .insert_source(source, move |_, _, state: &mut BlairState| {
            integrations.drain(state);
        })
        .map_err(|error| anyhow::anyhow!("integration source: {error}"))?;
    Ok((events, auth))
}

fn init_housekeeping(handle: &LoopHandle<'static, BlairState>) -> Result<()> {
    handle
        .insert_source(
            Timer::from_duration(SUPERVISE_INTERVAL),
            |_, _, state: &mut BlairState| {
                state.supervise_children();
                TimeoutAction::ToDuration(SUPERVISE_INTERVAL)
            },
        )
        .map_err(|error| anyhow::anyhow!("supervision timer: {error}"))?;
    Ok(())
}

/// Wires the sources every backend needs and returns the Wayland socket name
/// together with the integration event channel.
fn setup(
    event_loop: &EventLoop<'static, BlairState>,
    display: Display<BlairState>,
    config: &CompositorConfig,
    session: bool,
) -> Result<(
    String,
    Arc<dyn EventChannel>,
    Option<blair_integration_dbus::ScreencopyAuth>,
)> {
    let handle = event_loop.handle();
    let socket = init_wayland(&handle, display)?;
    let environment = if session {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "Blair".to_owned());
        vec![
            ("WAYLAND_DISPLAY".to_owned(), socket.clone()),
            ("XDG_CURRENT_DESKTOP".to_owned(), desktop),
            ("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned()),
        ]
    } else {
        Vec::new()
    };
    let (events, auth) = init_integrations(&handle, config, environment)?;
    init_housekeeping(&handle)?;
    schedule_first_frame(&handle);
    crate::config::watch(&handle, config)?;
    Ok((socket, events, auth))
}

/// Renders screenshots queued by integrations since the last iteration.
fn process_screenshots(
    state: &mut BlairState,
    renderer: &mut GlesRenderer,
    output: &smithay::output::Output,
    shaders: Option<&crate::render::Shaders>,
) {
    while let Some(request) = state.screenshots.next_for(output) {
        if output.current_mode().map(|mode| mode.size) != Some(request.size) {
            tracing::warn!(
                output = output.name(),
                "screenshot output mode changed before capture"
            );
            continue;
        }
        match crate::render::capture_region(
            renderer,
            state,
            output,
            shaders,
            smithay::utils::Rectangle::from_size(request.size),
            crate::render::CursorMode::Composited,
            smithay::backend::allocator::Fourcc::Abgr8888,
        ) {
            Ok(pixels) => state.screenshots.submit(request, pixels),
            Err(error) => {
                tracing::warn!(%error, path = %request.path.display(), "screenshot capture failed");
            }
        }
    }
}

/// Completes `wlr-screencopy` captures queued since the last iteration.
fn process_frame_captures(
    state: &mut BlairState,
    renderer: &mut GlesRenderer,
    output: &smithay::output::Output,
    framebuffer: &smithay::backend::renderer::gles::GlesTarget<'_>,
    transform: smithay::utils::Transform,
    cursor_mode: crate::render::CursorMode,
) {
    use crate::render::CursorMode;
    use smithay::input::pointer::CursorImageStatus;
    use smithay::{backend::renderer::ExportMem, utils::IsAlive};

    if state.pending_captures.is_empty() {
        return;
    }

    let Some(mode) = output.current_mode() else {
        return;
    };
    let size = output.current_transform().transform_size(mode.size);
    let pointer_inside = state
        .space
        .output_geometry(output)
        .is_some_and(|geometry| geometry.to_f64().contains(state.pointer_location()));
    let cursor = state.pointer_cursor();
    let visible_cursor = pointer_inside && cursor != CursorImageStatus::Hidden;
    let frame_has_cursor = visible_cursor
        && match cursor_mode {
            CursorMode::Composited => true,
            CursorMode::Hidden => false,
            CursorMode::HostNamed => {
                matches!(cursor, CursorImageStatus::Surface(ref surface) if surface.alive())
            }
        };
    let zoom = state.config.accessibility.zoom && state.config.accessibility.zoom_factor > 1.0;
    let cursor_capture = (!frame_has_cursor
        && visible_cursor
        && !zoom
        && state
            .pending_captures
            .iter()
            .any(|request| request.output == *output && request.overlay_cursor))
    .then(|| crate::render::capture_cursor(renderer, state, output));
    let time = state.clock_now();
    for request in std::mem::take(&mut state.pending_captures) {
        if request.output != *output || (frame_has_cursor && !request.overlay_cursor) {
            state.pending_captures.push(request);
            continue;
        }
        let overlay = if !frame_has_cursor && visible_cursor && request.overlay_cursor {
            match cursor_capture.as_ref() {
                Some(Ok(Some((region, mapping)))) => match renderer.map_texture(mapping) {
                    Ok(pixels) => Some(crate::screencopy::CursorOverlay {
                        region: *region,
                        pixels,
                    }),
                    Err(error) => {
                        tracing::debug!(%error, "cursor mapping unavailable; capturing offscreen");
                        state.pending_captures.push(request);
                        continue;
                    }
                },
                Some(Ok(None)) => None,
                _ => {
                    state.pending_captures.push(request);
                    continue;
                }
            }
        } else {
            None
        };
        let mapping = match crate::render::read_framebuffer(
            renderer,
            framebuffer,
            size,
            request.region,
            transform,
        ) {
            Ok(mapping) => mapping,
            Err(error) => {
                tracing::debug!(%error, "output readback unavailable; capturing offscreen");
                state.pending_captures.push(request);
                continue;
            }
        };
        match renderer.map_texture(&mapping) {
            Ok(pixels) => {
                tracing::trace!(output = output.name(), region = ?request.region, cursor_overlay = overlay.is_some(), "screencopy reused output framebuffer");
                request.submit_transformed(pixels, transform, overlay, time);
            }
            Err(error) => {
                tracing::debug!(%error, "output mapping unavailable; capturing offscreen");
                state.pending_captures.push(request);
            }
        }
    }
}

/// Captures requests that could not reuse the output framebuffer.
fn process_captures(
    state: &mut BlairState,
    renderer: &mut GlesRenderer,
    output: &smithay::output::Output,
    shaders: Option<&crate::render::Shaders>,
) {
    use smithay::backend::allocator::Fourcc;

    for request in std::mem::take(&mut state.pending_captures) {
        if request.output != *output {
            request.fail();
            continue;
        }
        let cursor = if request.overlay_cursor {
            crate::render::CursorMode::Composited
        } else {
            crate::render::CursorMode::Hidden
        };
        match crate::render::capture_region(
            renderer,
            state,
            output,
            shaders,
            request.region,
            cursor,
            Fourcc::Xrgb8888,
        ) {
            Ok(pixels) => request.submit(&pixels, state.clock_now()),
            Err(error) => {
                tracing::warn!(%error, "screencopy failed");
                request.fail();
            }
        }
    }
}

/// Completes dmabuf imports requested by clients since the last iteration.
fn import_pending_dmabufs(state: &mut BlairState, renderer: &mut GlesRenderer) {
    for (dmabuf, notifier) in std::mem::take(&mut state.pending_dmabuf_imports) {
        match renderer.import_dmabuf(&dmabuf, None) {
            Ok(_) => {
                let _ = notifier.successful::<BlairState>();
            }
            Err(error) => {
                tracing::warn!(%error, "client dmabuf import failed");
                notifier.failed();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::fixture;

    #[test]
    #[ignore = "requires an EGL device with OpenGL ES support"]
    fn screenshots_complete_after_gpu_capture_and_reject_mode_changes() {
        let egl = crate::test_support::EglTest::new();
        let (_display, _event_loop, mut state) = fixture();
        let output = crate::test_support::add_output(&mut state, "capture", (0, 0), (64, 64));
        let mut renderer = egl.renderer();
        let path = std::env::temp_dir().join(format!(
            "blair-backend-screenshot-{}.png",
            std::process::id()
        ));
        let (sender, receiver) = std::sync::mpsc::channel();
        state.request_screenshot(
            "",
            path.to_str().unwrap(),
            Box::new(move |success| {
                sender.send((success, std::thread::current().id())).unwrap();
            }),
        );
        assert!(receiver.try_recv().is_err());
        process_screenshots(&mut state, &mut renderer, &output, None);
        let (success, worker) = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(success);
        assert_ne!(worker, std::thread::current().id());
        let mut reader = png::Decoder::new(std::fs::File::open(&path).unwrap())
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (64, 64));
        assert_eq!(info.buffer_size(), 64 * 64 * 4);
        assert_eq!(&pixels[pixels.len() - 4..], &[20, 20, 31, 255]);
        std::fs::remove_file(&path).unwrap();

        let (sender, receiver) = std::sync::mpsc::channel();
        state.request_screenshot(
            "capture",
            path.to_str().unwrap(),
            Box::new(move |success| {
                sender.send(success).unwrap();
            }),
        );
        output.change_current_state(
            Some(smithay::output::Mode {
                size: (32, 32).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        process_screenshots(&mut state, &mut renderer, &output, None);
        assert!(!receiver.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(!path.exists());

        let (sender, receiver) = std::sync::mpsc::channel();
        state.take_redraw_request();
        state.request_screenshot(
            "missing",
            path.to_str().unwrap(),
            Box::new(move |success| {
                sender.send(success).unwrap();
            }),
        );
        assert!(!receiver.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(!state.take_redraw_request());
    }

    #[test]
    fn presentation_cancels_the_watchdog_without_an_idle_redraw() {
        let (_display, mut event_loop, mut state) = fixture();
        let mut pacer = FramePacer::new(Duration::from_millis(1));
        assert!(pacer.poll(&mut state));
        pacer.frame_queued(&mut state);
        let (_, token) = pacer.wakeup.unwrap();
        pacer.frame_presented(&state);
        assert!(state.loop_handle.update(&token).is_err());
        event_loop
            .dispatch(Duration::from_millis(20), &mut state)
            .unwrap();
        assert!(!pacer.poll(&mut state));
    }

    #[test]
    fn an_earlier_repaint_cancels_the_previous_timer_and_fires_once() {
        let (_display, mut event_loop, mut state) = fixture();
        let mut pacer = FramePacer::new(Duration::from_millis(1));
        assert!(pacer.poll(&mut state));
        pacer.schedule_wakeup(&mut state, Duration::from_secs(1));
        let (_, token) = pacer.wakeup.unwrap();
        pacer.schedule_wakeup(&mut state, Duration::ZERO);
        assert!(state.loop_handle.update(&token).is_err());
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        pacer.last_frame -= pacer.interval;
        assert!(pacer.poll(&mut state));
        assert!(pacer.wakeup.is_none());
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert!(!pacer.poll(&mut state));
    }

    #[test]
    fn an_animation_wakeup_preserves_recovery_from_missing_presentation() {
        let (_display, mut event_loop, mut state) = fixture();
        let mut pacer = FramePacer::new(Duration::from_secs(60));
        assert!(pacer.poll(&mut state));
        pacer.frame_queued(&mut state);
        pacer.schedule_wakeup(&mut state, Duration::ZERO);
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert!(!pacer.poll(&mut state));
        assert!(pacer.frame_pending);
        let (_, token) = pacer.wakeup.unwrap();
        assert!(state.loop_handle.update(&token).is_ok());
        pacer.last_frame -= pacer.interval * MISSED_FRAME_INTERVALS;
        assert!(pacer.poll(&mut state));
        assert!(!pacer.frame_pending);
        assert!(pacer.wakeup.is_none());
    }

    #[test]
    fn missing_presentation_wakes_the_event_loop_and_retries_the_frame() {
        let (_display, mut event_loop, mut state) = fixture();
        let mut pacer = FramePacer::new(Duration::from_millis(1));
        assert!(pacer.poll(&mut state));
        pacer.frame_queued(&mut state);
        event_loop
            .dispatch(Duration::from_millis(20), &mut state)
            .unwrap();
        assert!(pacer.poll(&mut state));
        assert!(!pacer.frame_pending);
        assert!(pacer.wakeup.is_none());
    }

    #[test]
    fn forcing_a_repaint_cancels_an_obsolete_wakeup() {
        let (_display, mut event_loop, mut state) = fixture();
        let mut pacer = FramePacer::new(Duration::from_millis(1));
        assert!(pacer.poll(&mut state));
        pacer.frame_queued(&mut state);
        let (_, token) = pacer.wakeup.unwrap();
        pacer.force_redraw(&state);
        assert!(state.loop_handle.update(&token).is_err());
        assert!(pacer.poll(&mut state));
        event_loop
            .dispatch(Duration::from_millis(20), &mut state)
            .unwrap();
        assert!(!pacer.poll(&mut state));
    }
}
