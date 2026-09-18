mod drm;
mod winit;

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
            EventLoop, Interest, LoopHandle, Mode, PostAction,
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
    wakeup_at: Option<Instant>,
}

impl FramePacer {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            redraw_pending: true,
            frame_pending: false,
            last_frame: Instant::now() - interval,
            wakeup_at: None,
        }
    }

    /// Returns true when a frame is due now.
    fn poll(&mut self, state: &mut BlairState) -> bool {
        if state.take_redraw_request() {
            self.redraw_pending = true;
        }
        if self.frame_pending {
            if self.last_frame.elapsed() < self.interval * MISSED_FRAME_INTERVALS {
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
    fn frame_presented(&mut self) {
        self.frame_pending = false;
        self.last_frame = Instant::now() - self.interval;
    }

    fn force_redraw(&mut self) {
        self.redraw_pending = true;
        self.frame_pending = false;
        self.last_frame = Instant::now() - self.interval;
    }

    fn schedule_wakeup(&mut self, state: &mut BlairState, delay: Duration) {
        let now = Instant::now();
        let at = now + delay;
        if self
            .wakeup_at
            .is_some_and(|scheduled| scheduled > now && scheduled <= at)
        {
            return;
        }
        self.wakeup_at = Some(at);
        let result = state.loop_handle.insert_source(
            Timer::from_duration(delay),
            |_, _, state: &mut BlairState| {
                state.request_redraw();
                TimeoutAction::Drop
            },
        );
        if let Err(error) = result {
            tracing::warn!(%error, "failed to schedule the next repaint");
            self.wakeup_at = None;
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
) -> Result<Arc<dyn EventChannel>> {
    let (ping, source) =
        calloop::ping::make_ping().context("failed to create the integration wake-up source")?;
    let mut integrations = Integrations::start(
        config.integrations.dbus,
        Arc::new(move || ping.ping()),
        environment,
    );
    let events = integrations.event_channel();
    handle
        .insert_source(source, move |_, _, state: &mut BlairState| {
            integrations.drain(state);
        })
        .map_err(|error| anyhow::anyhow!("integration source: {error}"))?;
    Ok(events)
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
) -> Result<(String, Arc<dyn EventChannel>)> {
    let handle = event_loop.handle();
    let socket = init_wayland(&handle, display)?;
    let environment = if session {
        vec![
            ("WAYLAND_DISPLAY".to_owned(), socket.clone()),
            ("XDG_CURRENT_DESKTOP".to_owned(), "Blair".to_owned()),
            ("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned()),
        ]
    } else {
        Vec::new()
    };
    let events = init_integrations(&handle, config, environment)?;
    init_housekeeping(&handle)?;
    schedule_first_frame(&handle);
    crate::config::watch(&handle, config)?;
    Ok((socket, events))
}

/// Renders screenshots queued by integrations since the last iteration.
fn process_screenshots(
    state: &mut BlairState,
    renderer: &mut GlesRenderer,
    output: &smithay::output::Output,
    shaders: Option<&crate::render::Shaders>,
) {
    for request in std::mem::take(&mut state.pending_screenshots) {
        if request
            .output
            .as_deref()
            .is_some_and(|name| name != output.name())
        {
            tracing::warn!(output = ?request.output, "unknown screenshot output");
            (request.reply)(false);
            continue;
        }
        match crate::render::screenshot(renderer, state, output, shaders, &request.path) {
            Ok(()) => (request.reply)(true),
            Err(error) => {
                tracing::warn!(%error, path = %request.path.display(), "screenshot failed");
                (request.reply)(false);
            }
        }
    }
}

/// Completes `wlr-screencopy` captures queued since the last iteration.
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
