use std::{cell::RefCell, rc::Rc, time::Instant};

use anyhow::{Context, Result};
use smithay::{
    backend::{
        egl::EGLDevice,
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, ImportDma, ImportEgl},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    input::pointer::CursorImageStatus,
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::Display,
    },
    utils::Transform,
    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal},
        presentation::Refresh,
    },
};

use crate::{
    config::{CompositorConfig, OutputTransform},
    input::{process_input_event, reset_keyboard_state, InputHooks},
    render::{
        cursor_is_animated, output_elements, send_frame_callbacks, take_presentation_feedback,
        update_primary_scanout_output, CursorMode, Shaders, CLEAR_COLOR,
    },
    state::BlairState,
    stats::FrameTimer,
};

const OUTPUT_NAME: &str = "winit-0";
const REFRESH_MILLIHZ: i32 = 60_000;
const FRAME_PACING: std::time::Duration = std::time::Duration::from_millis(16);

struct NestedHooks;

impl InputHooks for NestedHooks {}

struct WinitData {
    backend: WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: OutputDamageTracker,
    output: Output,
    shaders: Option<Shaders>,
    timer: FrameTimer,
    host_cursor: Option<CursorImageStatus>,
    pacer: super::FramePacer,
    force_redraw: bool,
    /// Buffer age of the next back buffer, sampled right after a swap so the
    /// EGL surface is guaranteed to be current.
    age: usize,
}

impl WinitData {
    fn resize(&mut self, state: &mut BlairState) {
        let size = self.backend.window_size();
        let mode = OutputMode {
            size,
            refresh: REFRESH_MILLIHZ,
        };
        self.output
            .change_current_state(Some(mode), None, None, None);
        self.output.set_preferred(mode);
        self.damage_tracker = OutputDamageTracker::new(
            size,
            self.output.current_scale().fractional_scale(),
            Transform::Flipped180,
        );
        self.force_redraw = true;
        self.age = 0;
        state.output_changed(&self.output);
        tracing::debug!(w = size.w, h = size.h, "host window resized");
    }

    /// Reflects the pointer cursor onto the host window. Client-provided
    /// cursor surfaces are drawn by us, so the host cursor hides then.
    fn sync_host_cursor(&mut self, state: &BlairState) {
        if self.host_cursor.as_ref() == Some(&state.pointer_cursor) {
            return;
        }
        self.host_cursor = Some(state.pointer_cursor.clone());
        let window = self.backend.window();
        match &state.pointer_cursor {
            CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => {
                window.set_cursor_visible(false)
            }
            CursorImageStatus::Named(icon) => {
                window.set_cursor(*icon);
                window.set_cursor_visible(true);
            }
        }
    }

    fn render(&mut self, state: &mut BlairState) {
        profiling::scope!("render_winit");
        let build_start = Instant::now();
        let elements = output_elements(
            self.backend.renderer(),
            state,
            &self.output,
            self.shaders.as_ref(),
            CursorMode::HostNamed,
        );
        let build = build_start.elapsed();

        let render_start = Instant::now();
        let age = self.age;
        let (renderer, mut framebuffer) = match self.backend.bind() {
            Ok(bound) => bound,
            Err(error) => {
                tracing::warn!(%error, "failed to bind the host surface");
                return;
            }
        };
        let result = self.damage_tracker.render_output(
            renderer,
            &mut framebuffer,
            age,
            &elements,
            CLEAR_COLOR,
        );
        drop(framebuffer);

        let (damage, states) = match result {
            Ok(result) => (
                result.damage.filter(|damage| !damage.is_empty()).cloned(),
                result.states,
            ),
            Err(error) => {
                tracing::warn!(%error, "render failed");
                return;
            }
        };
        // Submitting blocks on the host vsync, so it is not part of the
        // measured render time.
        let render = render_start.elapsed();
        let damaged = damage.is_some();
        if let Some(damage) = damage {
            match self.backend.submit(Some(&damage)) {
                Ok(()) => self.age = self.backend.buffer_age().unwrap_or(0),
                Err(error) => {
                    tracing::warn!(%error, "failed to submit the host surface");
                    self.age = 0;
                }
            }
        }

        update_primary_scanout_output(state, &self.output, &states);
        let mut feedback = take_presentation_feedback(state, &self.output, &states);
        feedback.presented(
            state.clock.now(),
            Refresh::Unknown,
            0,
            wp_presentation_feedback::Kind::empty(),
        );
        send_frame_callbacks(state, &self.output, state.clock_now());

        self.timer.record_frame(build, render, damaged);
        self.timer.record_presented(Instant::now());
        self.timer.maybe_report(&mut state.render_stats);

        let animating = state.animations_active() || cursor_is_animated(state, &self.output);
        self.pacer.keep_awake(state, animating);
    }

    /// Renders when a repaint is due, at most once per host frame.
    fn dispatch_redraw(&mut self, state: &mut BlairState) {
        if std::mem::take(&mut self.force_redraw) {
            self.pacer.force_redraw();
        }
        if self.pacer.poll(state) {
            self.render(state);
        }
    }
}

pub fn run(config: CompositorConfig) -> Result<()> {
    let mut event_loop: EventLoop<'static, BlairState> =
        EventLoop::try_new().context("failed to create the event loop")?;
    let display: Display<BlairState> =
        Display::new().context("failed to create the Wayland display")?;
    let display_handle = display.handle();

    let (socket, events) = super::setup(&event_loop, display, &config, false)?;
    let mut state = BlairState::new(
        display_handle.clone(),
        event_loop.handle(),
        event_loop.get_signal(),
        config,
        events,
    );
    state.set_wayland_display(&socket);

    let (mut backend, winit_source) = winit::init::<GlesRenderer>()
        .map_err(|error| anyhow::anyhow!("failed to init the winit backend: {error:?}"))?;

    let size = backend.window_size();
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Blair".to_string(),
            model: "Winit".to_string(),
        },
    );
    let mode = OutputMode {
        size,
        refresh: REFRESH_MILLIHZ,
    };
    let output_config = state.config.outputs.get(OUTPUT_NAME);
    let location = output_config
        .and_then(|config| config.position)
        .unwrap_or([0, 0]);
    let scale = output_config
        .and_then(|config| config.scale)
        .map(Scale::Fractional);
    warn_unsupported_output_config(&state.config, mode);
    output.change_current_state(
        Some(mode),
        None,
        scale,
        Some((location[0], location[1]).into()),
    );
    output.set_preferred(mode);
    let _global = output.create_global::<BlairState>(&display_handle);
    state.add_output(&output, (location[0], location[1]).into());
    tracing::info!(width = size.w, height = size.h, "winit output created");

    let shaders = Shaders::compile(backend.renderer());
    init_dmabuf(&mut state, &mut backend, &display_handle);

    let scale = output.current_scale().fractional_scale();
    let data = Rc::new(RefCell::new(WinitData {
        damage_tracker: OutputDamageTracker::new(size, scale, Transform::Flipped180),
        backend,
        output,
        shaders,
        timer: FrameTimer::new(OUTPUT_NAME),
        host_cursor: None,
        pacer: super::FramePacer::new(FRAME_PACING),
        force_redraw: true,
        age: 0,
    }));

    let event_data = Rc::clone(&data);
    event_loop
        .handle()
        .insert_source(winit_source, move |event, _, state: &mut BlairState| {
            let mut data = event_data.borrow_mut();
            match event {
                WinitEvent::Resized { .. } => data.resize(state),
                WinitEvent::Input(event) => {
                    process_input_event(state, event, &mut NestedHooks);
                }
                WinitEvent::Focus(false) => reset_keyboard_state(state),
                WinitEvent::Redraw => {
                    state.request_redraw();
                }
                WinitEvent::CloseRequested => {
                    tracing::info!("host window closed — stopping compositor");
                    state.request_exit();
                }
                WinitEvent::Focus(true) => {}
            }
        })
        .map_err(|error| anyhow::anyhow!("winit source: {error}"))?;

    state.spawn_autostarts();
    tracing::info!("entering the nested event loop");

    event_loop.run(None, &mut state, move |state| {
        let mut data = data.borrow_mut();
        let WinitData {
            backend,
            output,
            shaders,
            ..
        } = &mut *data;
        super::import_pending_dmabufs(state, backend.renderer());
        super::process_screenshots(state, backend.renderer(), output, shaders.as_ref());
        super::process_captures(state, backend.renderer(), output, shaders.as_ref());
        state.refresh();
        data.sync_host_cursor(state);
        data.dispatch_redraw(state);
        if let Err(error) = state.display_handle.flush_clients() {
            tracing::warn!(%error, "failed to flush clients");
        }
    })?;

    tracing::info!("compositor exiting");
    Ok(())
}

fn init_dmabuf(
    state: &mut BlairState,
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    display_handle: &smithay::reexports::wayland_server::DisplayHandle,
) {
    let renderer = backend.renderer();
    if let Err(error) = renderer.bind_wl_display(display_handle) {
        tracing::debug!(%error, "EGL wl_display binding unavailable");
    }
    let render_node = EGLDevice::device_for_display(renderer.egl_context().display())
        .ok()
        .and_then(|device| device.try_get_render_node().ok().flatten());
    let formats: Vec<_> = renderer.dmabuf_formats().iter().copied().collect();
    let global: Option<DmabufGlobal> = match render_node {
        Some(node) => DmabufFeedbackBuilder::new(node.dev_id(), formats.clone())
            .build()
            .map_err(|error| tracing::warn!(%error, "failed to build dmabuf feedback"))
            .ok()
            .map(|feedback| {
                state
                    .dmabuf_state
                    .create_global_with_default_feedback::<BlairState>(display_handle, &feedback)
            }),
        None => Some(
            state
                .dmabuf_state
                .create_global::<BlairState>(display_handle, formats),
        ),
    };
    if global.is_some() {
        tracing::info!("linux-dmabuf enabled");
    }
    state.dmabuf_global = global;
}

fn warn_unsupported_output_config(config: &CompositorConfig, mode: OutputMode) {
    let Some(output) = config.outputs.get(OUTPUT_NAME) else {
        return;
    };
    if let Ok(Some(requested)) = output.parsed_mode() {
        if requested.width != mode.size.w
            || requested.height != mode.size.h
            || requested.refresh_millihz != mode.refresh
        {
            tracing::warn!(
                output = OUTPUT_NAME,
                ?requested,
                "the nested backend cannot change its host window mode"
            );
        }
    }
    if output.enabled == Some(false) {
        tracing::warn!(output = OUTPUT_NAME, "refusing to disable the only output");
    }
    if output.vrr.is_some() {
        tracing::warn!(
            output = OUTPUT_NAME,
            "VRR is unavailable in the nested backend"
        );
    }
    if output
        .parsed_transform()
        .ok()
        .flatten()
        .is_some_and(|transform| transform != OutputTransform::Normal)
    {
        tracing::warn!(
            output = OUTPUT_NAME,
            "output transforms are unavailable in the nested backend"
        );
    }
}
