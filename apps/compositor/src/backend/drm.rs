//! DRM/KMS backend — used when launching from a TTY (no parent compositor).

use std::{
    cell::{Cell, RefCell},
    os::fd::{AsFd, BorrowedFd},
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use smithay::reexports::input::{AccelProfile, Device as LibinputDevice, Libinput};
use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{
            compositor::{DrmCompositor, FrameFlags, PrimaryPlaneElement},
            exporter::gbm::GbmFramebufferExporter,
            DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, DrmEventMetadata, DrmEventTime,
            DrmNode, NodeType,
        },
        egl::{EGLContext, EGLDevice, EGLDisplay},
        input::{Device as InputDevice, DeviceCapability, InputEvent},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{gles::GlesRenderer, ImportDma, ImportEgl},
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::UdevBackend,
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        drm::{
            self,
            control::{connector, crtc, Device as ControlDevice, ModeTypeFlags},
        },
        input::Led,
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::Display,
    },
    utils::{DeviceFd, Transform},
    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal},
        presentation::Refresh,
    },
};

use crate::{
    config::{CompositorConfig, InputConfig, OutputConfig, OutputTransform},
    input::{process_input_event, reset_keyboard_state, InputHooks},
    render::{
        cursor_is_animated, output_elements, send_frame_callbacks, take_presentation_feedback,
        update_primary_scanout_output, CursorMode, Shaders, CLEAR_COLOR,
    },
    state::BlairState,
    stats::FrameTimer,
};

type BlairDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<smithay::desktop::utils::OutputPresentationFeedback>,
    DrmDeviceFd,
>;

const COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Xrgb8888, Fourcc::Argb8888];

struct DrmBackend {
    drm: DrmDevice,
    notifier: Option<DrmDeviceNotifier>,
    compositor: BlairDrmCompositor,
    renderer: GlesRenderer,
    shaders: Option<Shaders>,
    output: Output,
    refresh_interval: Duration,
    pacer: super::FramePacer,
    libinput: Libinput,
    devices: Vec<LibinputDevice>,
    led_state: smithay::input::keyboard::LedState,
    timer: FrameTimer,
    frames: Arc<AtomicU64>,
    active: bool,
}

struct DrmHooks {
    session: LibSeatSession,
}

impl InputHooks for DrmHooks {
    fn change_vt(&mut self, vt: i32) {
        if let Err(error) = self.session.change_vt(vt) {
            tracing::warn!(%error, vt, "VT switch failed");
        }
    }
}

impl DrmBackend {
    /// Renders when a repaint is due and the previous frame was presented.
    fn dispatch_redraw(&mut self, state: &mut BlairState) {
        if !self.active {
            return;
        }
        if self.pacer.poll(state) {
            self.render(state);
        }
    }

    fn render(&mut self, state: &mut BlairState) {
        profiling::scope!("render_drm");
        let build_start = Instant::now();
        let elements = output_elements(
            &mut self.renderer,
            state,
            &self.output,
            self.shaders.as_ref(),
            CursorMode::Composited,
        );
        let build = build_start.elapsed();

        let render_start = Instant::now();
        let result = self.compositor.render_frame(
            &mut self.renderer,
            &elements,
            CLEAR_COLOR,
            FrameFlags::DEFAULT,
        );
        let frame = match result {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(%error, "frame rendering failed");
                self.pacer.keep_awake(state, true);
                return;
            }
        };
        if frame.needs_sync() {
            if let PrimaryPlaneElement::Swapchain(element) = &frame.primary_element {
                if let Err(error) = element.sync.wait() {
                    tracing::warn!(%error, "waiting for the render fence failed");
                }
            }
        }
        let damaged = !frame.is_empty;
        let render = render_start.elapsed();

        update_primary_scanout_output(state, &self.output, &frame.states);
        if damaged {
            let feedback = take_presentation_feedback(state, &self.output, &frame.states);
            match self.compositor.queue_frame(Some(feedback)) {
                Ok(()) => {
                    self.pacer.frame_queued(state);
                    self.frames.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => tracing::warn!(%error, "failed to queue the frame"),
            }
        }
        send_frame_callbacks(state, &self.output, state.clock_now());

        self.timer.record_frame(build, render, damaged);
        self.timer.maybe_report(&mut state.render_stats);
        let animating = state.animations_active() || cursor_is_animated(state, &self.output);
        self.pacer.keep_awake(state, animating);
    }

    fn on_vblank(&mut self, state: &mut BlairState, metadata: Option<DrmEventMetadata>) {
        self.pacer.frame_presented();
        let presentation_time = match metadata.as_ref().map(|metadata| metadata.time) {
            Some(DrmEventTime::Monotonic(time)) => time,
            _ => state.clock_now(),
        };
        let sequence = metadata.map(|metadata| metadata.sequence).unwrap_or(0);
        match self.compositor.frame_submitted() {
            Ok(Some(Some(mut feedback))) => {
                let flags = wp_presentation_feedback::Kind::Vsync
                    | wp_presentation_feedback::Kind::HwClock
                    | wp_presentation_feedback::Kind::HwCompletion;
                feedback.presented::<_, smithay::utils::Monotonic>(
                    presentation_time,
                    Refresh::fixed(self.refresh_interval),
                    u64::from(sequence),
                    flags,
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "frame submission failed"),
        }
        self.timer.record_presented(Instant::now());
        self.dispatch_redraw(state);
    }

    fn set_active(&mut self, state: &mut BlairState, active: bool) {
        self.active = active;
        if active {
            tracing::info!("session activated");
            if self.libinput.resume().is_err() {
                tracing::warn!("failed to resume libinput after VT switch");
            }
            if let Err(error) = self.drm.activate(false) {
                tracing::error!(%error, "failed to reactivate DRM after VT switch");
                state.request_exit();
                return;
            }
            if let Err(error) = self.compositor.reset_state() {
                tracing::error!(%error, "failed to reset the DRM compositor state");
            }
            self.compositor.reset_buffers();
            self.pacer.force_redraw();
            self.timer.reset_presentation();
            reset_keyboard_state(state);
            self.apply_input_config(&state.config.input);
            state.request_redraw();
        } else {
            tracing::info!("session paused (VT switch out)");
            self.libinput.suspend();
            self.drm.pause();
            // No vblank will arrive for an in-flight buffer once paused.
            self.pacer.frame_presented();
        }
    }

    fn apply_input_config(&mut self, config: &InputConfig) {
        for device in &mut self.devices {
            configure_input_device(device, config);
        }
    }

    fn sync_leds(&mut self, state: &BlairState) {
        if self.led_state == state.led_state {
            return;
        }
        self.led_state = state.led_state;
        let leds = Led::from(state.led_state);
        for device in &mut self.devices {
            device.led_update(leds);
        }
    }

    fn device_added(&mut self, mut device: LibinputDevice, state: &mut BlairState) {
        configure_input_device(&mut device, &state.config.input);
        log_input_device("input device added", &device);
        if InputDevice::has_capability(&device, DeviceCapability::Touch)
            && state.seat.get_touch().is_none()
        {
            state.seat.add_touch();
            tracing::info!("touch capability added to the seat");
        }
        self.devices.push(device);
    }

    fn device_removed(&mut self, device: &LibinputDevice) {
        log_input_device("input device removed", device);
        self.devices.retain(|known| known != device);
    }
}

pub fn run(config: CompositorConfig) -> Result<()> {
    diagnose_seat_environment();

    let (session, session_notifier) = LibSeatSession::new()
        .context("failed to open libseat session — is seatd/logind running?")?;
    let seat_name = session.seat();
    tracing::info!(
        seat = %seat_name,
        already_active = session.is_active(),
        "libseat session opened"
    );

    let mut event_loop: EventLoop<'static, BlairState> =
        EventLoop::try_new().context("failed to create the event loop")?;
    let display: Display<BlairState> =
        Display::new().context("failed to create the Wayland display")?;
    let display_handle = display.handle();
    let (socket, events) = super::setup(&event_loop, display, &config, true)?;
    let mut state = BlairState::new(
        display_handle.clone(),
        event_loop.handle(),
        event_loop.get_signal(),
        config,
        events,
    );
    state.set_wayland_display(&socket);

    let backend: Rc<RefCell<Option<DrmBackend>>> = Rc::new(RefCell::new(None));
    let session_active = Rc::new(Cell::new(session.is_active()));

    let notifier_backend = Rc::clone(&backend);
    let notifier_active = Rc::clone(&session_active);
    event_loop
        .handle()
        .insert_source(session_notifier, move |event, _, state: &mut BlairState| {
            let active = matches!(event, SessionEvent::ActivateSession);
            notifier_active.set(active);
            if let Some(backend) = notifier_backend.borrow_mut().as_mut() {
                backend.set_active(state, active);
            }
        })
        .map_err(|error| anyhow::anyhow!("session notifier: {error}"))?;

    wait_for_session(&mut event_loop, &mut state, &session_active)?;

    let mut libinput_context =
        Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput_context
        .udev_assign_seat(&seat_name)
        .map_err(|_| anyhow::anyhow!("failed to assign the libinput seat"))?;
    tracing::info!(seat = %seat_name, "libinput seat assigned");

    let mut data = init_drm(
        &mut state,
        &display_handle,
        session.clone(),
        &seat_name,
        libinput_context.clone(),
    )?;
    data.active = session_active.get();
    let frames = Arc::clone(&data.frames);
    let crtc = data.compositor.crtc();
    let drm_notifier = data.take_drm_notifier();
    *backend.borrow_mut() = Some(data);

    let vblank_backend = Rc::clone(&backend);
    event_loop
        .handle()
        .insert_source(
            drm_notifier,
            move |event, metadata, state: &mut BlairState| {
                let mut backend = vblank_backend.borrow_mut();
                let Some(backend) = backend.as_mut() else {
                    return;
                };
                match event {
                    DrmEvent::VBlank(vblank_crtc) if vblank_crtc == crtc => {
                        backend.on_vblank(state, metadata.take());
                    }
                    DrmEvent::VBlank(_) => {}
                    DrmEvent::Error(error) => tracing::warn!(%error, "DRM error"),
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("DRM notifier: {error}"))?;

    let input_backend = Rc::clone(&backend);
    let mut hooks = DrmHooks {
        session: session.clone(),
    };
    let input_events = Arc::new(AtomicU64::new(0));
    let seen_events = Arc::clone(&input_events);
    event_loop
        .handle()
        .insert_source(
            LibinputInputBackend::new(libinput_context),
            move |event, _, state: &mut BlairState| {
                seen_events.fetch_add(1, Ordering::Relaxed);
                match event {
                    InputEvent::DeviceAdded { device } => {
                        if let Some(backend) = input_backend.borrow_mut().as_mut() {
                            backend.device_added(device, state);
                        }
                    }
                    InputEvent::DeviceRemoved { device } => {
                        if let Some(backend) = input_backend.borrow_mut().as_mut() {
                            backend.device_removed(&device);
                        }
                    }
                    event => {
                        process_input_event(state, event, &mut hooks);
                        let mut backend = input_backend.borrow_mut();
                        if let Some(backend) = backend.as_mut() {
                            backend.sync_leds(state);
                        }
                    }
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("libinput source: {error}"))?;

    spawn_render_watchdog(frames, Duration::from_secs(5));
    spawn_input_watchdog(input_events, Duration::from_secs(3));

    state.spawn_autostarts();
    tracing::info!("entering the DRM event loop");

    let tick_backend = Rc::clone(&backend);
    event_loop.run(None, &mut state, move |state| {
        let mut backend = tick_backend.borrow_mut();
        let Some(backend) = backend.as_mut() else {
            return;
        };
        super::import_pending_dmabufs(state, &mut backend.renderer);
        super::process_screenshots(
            state,
            &mut backend.renderer,
            &backend.output,
            backend.shaders.as_ref(),
        );
        super::process_captures(
            state,
            &mut backend.renderer,
            &backend.output,
            backend.shaders.as_ref(),
        );
        state.refresh();
        if std::mem::take(&mut state.input_config_changed) {
            backend.apply_input_config(&state.config.input);
        }
        backend.dispatch_redraw(state);
        if let Err(error) = state.display_handle.flush_clients() {
            tracing::warn!(%error, "failed to flush clients");
        }
    })?;

    tracing::info!("DRM compositor exiting");
    Ok(())
}

fn wait_for_session(
    event_loop: &mut EventLoop<'static, BlairState>,
    state: &mut BlairState,
    active: &Rc<Cell<bool>>,
) -> Result<()> {
    if active.get() {
        return Ok(());
    }
    tracing::info!("waiting for the libseat ActivateSession event");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !active.get() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!(
                "timed out waiting for libseat ActivateSession after 3s. \
                 Is seatd/logind running and is our session foreground? \
                 foreground_vt={} XDG_VTNR={}",
                std::fs::read_to_string("/sys/class/tty/tty0/active")
                    .unwrap_or_default()
                    .trim(),
                std::env::var("XDG_VTNR").unwrap_or_else(|_| "<unset>".into()),
            );
        }
        event_loop.dispatch(Some(remaining.min(Duration::from_millis(200))), state)?;
    }
    tracing::info!("the libseat session is now active");
    Ok(())
}

impl DrmBackend {
    fn take_drm_notifier(&mut self) -> DrmDeviceNotifier {
        self.notifier
            .take()
            .expect("the DRM notifier is taken exactly once")
    }
}

fn init_drm(
    state: &mut BlairState,
    display_handle: &smithay::reexports::wayland_server::DisplayHandle,
    mut session: LibSeatSession,
    seat_name: &str,
    libinput: Libinput,
) -> Result<DrmBackend> {
    let udev = UdevBackend::new(seat_name).context("failed to init udev")?;
    let cards = enumerate_drm_cards(&udev);
    log_drm_cards(&cards);
    let device_path =
        pick_best_drm_card(&cards).context("no DRM card with a connected output was found")?;
    tracing::info!(device = %device_path.display(), "selected DRM device");

    let raw_device_fd = session
        .open(
            &device_path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .context("failed to open the DRM device")?;
    let raw_device_fd = DeviceFd::from(raw_device_fd);
    probe_drm_master(&device_path, &raw_device_fd);
    let device_fd = DrmDeviceFd::new(raw_device_fd);

    let (mut drm, notifier) =
        DrmDevice::new(device_fd.clone(), false).context("failed to create the DRM device")?;
    let gbm = GbmDevice::new(device_fd.clone()).context("failed to create the GBM device")?;

    let resources = device_fd
        .resource_handles()
        .context("failed to get the DRM resource handles")?;
    let (connector, drm_mode, crtc, output_name) =
        find_output(&device_fd, &resources, drm.crtcs(), &state.config.outputs)
            .context("no connected display found")?;
    let connector_info = device_fd
        .get_connector(connector, false)
        .context("failed to read the connector info")?;
    let physical = connector_info.size().unwrap_or((0, 0));
    let mode = OutputMode::from(drm_mode);
    tracing::info!(
        output = %output_name,
        width = mode.size.w,
        height = mode.size.h,
        refresh = mode.refresh,
        "DRM output ready"
    );

    let drm_surface = drm
        .create_surface(crtc, drm_mode, &[connector])
        .context("failed to create the DRM surface")?;

    let egl_display =
        unsafe { EGLDisplay::new(gbm.clone()).context("failed to create the EGL display")? };
    let render_node = EGLDevice::device_for_display(&egl_display)
        .ok()
        .and_then(|device| device.try_get_render_node().ok().flatten());
    let egl_context = EGLContext::new(&egl_display).context("failed to create the EGL context")?;
    let mut renderer =
        unsafe { GlesRenderer::new(egl_context).context("failed to create the GLES renderer")? };
    if let Err(error) = renderer.bind_wl_display(display_handle) {
        tracing::debug!(%error, "EGL wl_display binding unavailable");
    }
    let renderer_formats = renderer
        .egl_context()
        .display()
        .dmabuf_render_formats()
        .clone();
    let shaders = Shaders::compile(&mut renderer);

    let output_config = state.config.outputs.get(&output_name).cloned();
    let output = Output::new(
        output_name.clone(),
        PhysicalProperties {
            size: (physical.0 as i32, physical.1 as i32).into(),
            subpixel: Subpixel::Unknown,
            make: "Unknown".to_string(),
            model: "DRM".to_string(),
        },
    );
    let location = output_config
        .as_ref()
        .and_then(|config| config.position)
        .unwrap_or([0, 0]);
    let transform = output_config
        .as_ref()
        .and_then(|config| config.parsed_transform().ok().flatten())
        .map(to_smithay_transform)
        .unwrap_or(Transform::Normal);
    let scale = output_config
        .as_ref()
        .and_then(|config| config.scale)
        .map(Scale::Fractional);
    output.change_current_state(
        Some(mode),
        Some(transform),
        scale,
        Some((location[0], location[1]).into()),
    );
    output.set_preferred(mode);
    let _global = output.create_global::<BlairState>(display_handle);
    state.add_output(&output, (location[0], location[1]).into());

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node);
    let mut compositor = DrmCompositor::new(
        &output,
        drm_surface,
        None,
        allocator,
        exporter,
        COLOR_FORMATS,
        renderer_formats,
        drm.cursor_size(),
        Some(gbm),
    )
    .context("failed to create the DRM compositor")?;

    if let Some(vrr) = output_config.as_ref().and_then(|config| config.vrr) {
        match compositor.use_vrr(vrr) {
            Ok(()) => tracing::info!(output = %output_name, vrr, "VRR configured"),
            Err(error) => {
                tracing::warn!(%error, output = %output_name, vrr, "failed to configure VRR")
            }
        }
    }

    init_dmabuf(state, &mut renderer, display_handle, render_node);

    let refresh_interval = refresh_interval(mode.refresh);
    Ok(DrmBackend {
        drm,
        notifier: Some(notifier),
        compositor,
        renderer,
        shaders,
        refresh_interval,
        pacer: super::FramePacer::new(refresh_interval),
        output,
        libinput,
        devices: Vec::new(),
        led_state: Default::default(),
        timer: FrameTimer::new(output_name),
        frames: Arc::new(AtomicU64::new(0)),
        active: true,
    })
}

fn refresh_interval(refresh_millihz: i32) -> Duration {
    let refresh = refresh_millihz.max(1) as u64;
    Duration::from_nanos(1_000_000_000_000 / refresh)
}

fn init_dmabuf(
    state: &mut BlairState,
    renderer: &mut GlesRenderer,
    display_handle: &smithay::reexports::wayland_server::DisplayHandle,
    render_node: Option<DrmNode>,
) {
    let formats: Vec<_> = renderer.dmabuf_formats().iter().copied().collect();
    let global: Option<DmabufGlobal> = match render_node {
        Some(node) => DmabufFeedbackBuilder::new(node.dev_id(), formats.clone())
            .build()
            .map_err(|error| tracing::warn!(%error, "failed to build the dmabuf feedback"))
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

fn configure_input_device(device: &mut LibinputDevice, config: &InputConfig) {
    let profile = match config.mouse.acceleration.as_str() {
        "flat" => AccelProfile::Flat,
        _ => AccelProfile::Adaptive,
    };
    if let Err(error) = device.config_accel_set_profile(profile) {
        tracing::debug!(?error, device = %device.name(), "pointer acceleration profile unsupported");
    }
    if let Err(error) = device.config_accel_set_speed(config.mouse.sensitivity) {
        tracing::debug!(?error, device = %device.name(), "pointer sensitivity unsupported");
    }
    if let Err(error) = device.config_tap_set_enabled(config.touchpad.tap) {
        tracing::debug!(?error, device = %device.name(), "tap-to-click unsupported");
    }
    if let Err(error) =
        device.config_scroll_set_natural_scroll_enabled(config.touchpad.natural_scroll)
    {
        tracing::debug!(?error, device = %device.name(), "natural scrolling unsupported");
    }
    if let Err(error) = device.config_dwt_set_enabled(config.touchpad.disable_while_typing) {
        tracing::debug!(?error, device = %device.name(), "disable-while-typing unsupported");
    }
}

fn log_input_device(message: &'static str, device: &impl InputDevice) {
    tracing::info!(
        name = %device.name(),
        id = %device.id(),
        syspath = ?device.syspath().as_deref(),
        keyboard = device.has_capability(DeviceCapability::Keyboard),
        pointer = device.has_capability(DeviceCapability::Pointer),
        touch = device.has_capability(DeviceCapability::Touch),
        gesture = device.has_capability(DeviceCapability::Gesture),
        switch = device.has_capability(DeviceCapability::Switch),
        "{message}"
    );
}

fn spawn_render_watchdog(counter: Arc<AtomicU64>, timeout: Duration) {
    std::thread::Builder::new()
        .name("blair-watchdog".into())
        .spawn(move || {
            std::thread::sleep(timeout);
            if counter.load(Ordering::Relaxed) == 0 {
                tracing::error!(
                    timeout_secs = timeout.as_secs(),
                    "watchdog: no frame rendered — DRM/GPU appears stuck, forcing exit"
                );
                let _ = std::io::Write::flush(&mut std::io::stderr().lock());
                std::process::exit(124);
            }
        })
        .expect("failed to spawn the watchdog thread");
}

fn spawn_input_watchdog(counter: Arc<AtomicU64>, timeout: Duration) {
    std::thread::Builder::new()
        .name("blair-input-watchdog".into())
        .spawn(move || {
            std::thread::sleep(timeout);
            if counter.load(Ordering::Relaxed) == 0 {
                tracing::warn!(
                    timeout_secs = timeout.as_secs(),
                    "watchdog: libinput produced no events; no keyboard/pointer devices were opened"
                );
            }
        })
        .expect("failed to spawn the input watchdog thread");
}

struct MasterProbeFd(DeviceFd);

impl AsFd for MasterProbeFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for MasterProbeFd {}

fn probe_drm_master(device_path: &std::path::Path, fd: &DeviceFd) {
    let probe = MasterProbeFd(fd.clone());
    match acquire_master_with_retry(&probe, 5, Duration::from_millis(200)) {
        Ok(()) => {
            tracing::info!("DRM master acquired");
            if let Err(error) = drm::Device::release_master_lock(&probe) {
                tracing::debug!(%error, "failed to release the DRM master probe");
            }
        }
        Err(error) => {
            let holders = enumerate_card_holders(device_path);
            if !holders.is_empty() {
                tracing::warn!(?holders, "other holders of the DRM card in our UID");
            }
            tracing::warn!(
                %error,
                diagnostic = %current_master_diagnostic(device_path, &holders),
                "drmSetMaster probe failed; continuing with the libseat-brokered DRM fd"
            );
        }
    }
}

fn acquire_master_with_retry<D: drm::Device>(
    fd: &D,
    attempts: u32,
    backoff: Duration,
) -> std::io::Result<()> {
    let mut last_error = None;
    for attempt in 1..=attempts {
        match drm::Device::acquire_master_lock(fd) {
            Ok(()) => {
                if attempt > 1 {
                    tracing::info!(attempt, "DRM master acquired after a retry");
                }
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(attempt, %error, "drmSetMaster failed, retrying");
                last_error = Some(error);
                if attempt < attempts {
                    std::thread::sleep(backoff);
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("drmSetMaster failed")))
}

fn to_smithay_transform(transform: OutputTransform) -> Transform {
    match transform {
        OutputTransform::Normal => Transform::Normal,
        OutputTransform::Rotate90 => Transform::_90,
        OutputTransform::Rotate180 => Transform::_180,
        OutputTransform::Rotate270 => Transform::_270,
        OutputTransform::Flipped => Transform::Flipped,
        OutputTransform::Flipped90 => Transform::Flipped90,
        OutputTransform::Flipped180 => Transform::Flipped180,
        OutputTransform::Flipped270 => Transform::Flipped270,
    }
}

fn find_output(
    fd: &DrmDeviceFd,
    resources: &smithay::reexports::drm::control::ResourceHandles,
    crtcs: &[crtc::Handle],
    outputs: &std::collections::BTreeMap<String, OutputConfig>,
) -> Option<(
    connector::Handle,
    smithay::reexports::drm::control::Mode,
    crtc::Handle,
    String,
)> {
    // Prefer an enabled configured connector. If every connected output is
    // disabled, retry with one anyway so a bad profile never leaves Blair
    // without a visible output.
    for allow_disabled in [false, true] {
        for &handle in resources.connectors() {
            let Ok(connector) = fd.get_connector(handle, false) else {
                continue;
            };
            if connector.state() != connector::State::Connected {
                continue;
            }
            let name = connector.to_string();
            let config = outputs.get(&name);
            if config.and_then(|config| config.enabled) == Some(false) && !allow_disabled {
                continue;
            }
            let requested = config.and_then(|config| config.parsed_mode().ok().flatten());
            let mode = requested
                .and_then(|requested| {
                    connector.modes().iter().find(|mode| {
                        let size = mode.size();
                        i32::from(size.0) == requested.width
                            && i32::from(size.1) == requested.height
                            && (OutputMode::from(**mode).refresh - requested.refresh_millihz).abs()
                                <= 1_000
                    })
                })
                .or_else(|| {
                    connector
                        .modes()
                        .iter()
                        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                })
                .or_else(|| connector.modes().first())
                .copied()?;
            if requested.is_some_and(|requested| {
                let size = mode.size();
                i32::from(size.0) != requested.width || i32::from(size.1) != requested.height
            }) {
                tracing::warn!(output = %name, ?requested, "the requested mode is unavailable; using the preferred mode");
            }

            for &encoder_handle in connector.encoders() {
                let Ok(encoder) = fd.get_encoder(encoder_handle) else {
                    continue;
                };
                let compatible = resources.filter_crtcs(encoder.possible_crtcs());
                for &crtc in crtcs {
                    if compatible.contains(&crtc) {
                        if allow_disabled {
                            tracing::warn!(output = %name, "all configured outputs were disabled; keeping this one enabled");
                        }
                        return Some((handle, mode, crtc, name));
                    }
                }
            }
        }
    }
    None
}

#[derive(Debug)]
struct DrmCardInfo {
    path: std::path::PathBuf,
    sysfs: Option<std::path::PathBuf>,
    connected_outputs: Vec<String>,
    all_outputs: Vec<(String, String)>,
}

fn enumerate_drm_cards(udev: &UdevBackend) -> Vec<DrmCardInfo> {
    let mut cards = Vec::new();
    for (_id, path) in udev.device_list() {
        let Ok(node) = DrmNode::from_path(path) else {
            continue;
        };
        if node.ty() != NodeType::Primary {
            continue;
        }
        let sysfs = sysfs_path_for_card(path);
        let (all_outputs, connected_outputs) = read_connectors(sysfs.as_deref());
        cards.push(DrmCardInfo {
            path: path.to_owned(),
            sysfs,
            connected_outputs,
            all_outputs,
        });
    }
    cards
}

fn sysfs_path_for_card(dev_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = dev_path.file_name()?.to_str()?;
    let candidate = std::path::PathBuf::from(format!("/sys/class/drm/{name}"));
    candidate.exists().then_some(candidate)
}

fn read_connectors(sysfs: Option<&std::path::Path>) -> (Vec<(String, String)>, Vec<String>) {
    let Some(dir) = sysfs else {
        return (vec![], vec![]);
    };
    let card_prefix = dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{name}-"))
        .unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (vec![], vec![]);
    };

    let mut all = Vec::new();
    let mut connected = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(connector_name) = file_name.strip_prefix(&card_prefix) else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let status = status.trim().to_string();
        let connector_name = connector_name.to_string();
        if status == "connected" {
            connected.push(connector_name.clone());
        }
        all.push((connector_name, status));
    }
    all.sort();
    connected.sort();
    (all, connected)
}

fn log_drm_cards(cards: &[DrmCardInfo]) {
    if cards.is_empty() {
        tracing::warn!("no DRM primary cards found via udev");
        return;
    }
    for card in cards {
        tracing::info!(
            path = %card.path.display(),
            sysfs = ?card.sysfs.as_deref().map(|path| path.display().to_string()),
            connected = ?card.connected_outputs,
            all = ?card.all_outputs,
            "DRM card"
        );
    }
}

fn pick_best_drm_card(cards: &[DrmCardInfo]) -> Option<std::path::PathBuf> {
    let with_outputs: Vec<_> = cards
        .iter()
        .filter(|card| !card.connected_outputs.is_empty())
        .collect();
    let pool = if with_outputs.is_empty() {
        cards.iter().collect()
    } else {
        with_outputs
    };
    pool.into_iter()
        .max_by(|a, b| {
            a.connected_outputs
                .len()
                .cmp(&b.connected_outputs.len())
                .then_with(|| b.path.cmp(&a.path))
        })
        .map(|card| card.path.clone())
}

fn enumerate_card_holders(card: &std::path::Path) -> Vec<(u32, String)> {
    let Ok(card_canon) = std::fs::canonicalize(card) else {
        return vec![];
    };
    let Ok(proc_dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    let self_pid = std::process::id();

    let mut holders = Vec::new();
    for entry in proc_dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd_entry in fds.flatten() {
            if std::fs::read_link(fd_entry.path()).is_ok_and(|target| target == card_canon) {
                let comm = std::fs::read_to_string(entry.path().join("comm"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                holders.push((pid, comm));
                break;
            }
        }
    }
    holders
}

fn diagnose_seat_environment() {
    let xdg_session_id = std::env::var("XDG_SESSION_ID").ok();
    let xdg_vtnr = std::env::var("XDG_VTNR").ok();
    let foreground_vt = std::fs::read_to_string("/sys/class/tty/tty0/active")
        .ok()
        .map(|active| active.trim().to_string());

    tracing::info!(
        session_id = ?xdg_session_id.as_deref(),
        session_type = ?std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        session_class = ?std::env::var("XDG_SESSION_CLASS").ok().as_deref(),
        seat = ?std::env::var("XDG_SEAT").ok().as_deref(),
        vtnr = ?xdg_vtnr.as_deref(),
        foreground_vt = ?foreground_vt.as_deref(),
        "seat/VT environment"
    );

    match (&xdg_vtnr, &foreground_vt) {
        (Some(vtnr), Some(foreground)) if vtnr.trim() != foreground.trim_start_matches("tty") => {
            tracing::warn!(
                vtnr,
                foreground = %foreground,
                "the session VT does not match the foreground VT — logind will refuse to \
                 grant DRM master until the foreground VT switches to ours"
            );
        }
        (None, _) => {
            tracing::warn!(
                "XDG_VTNR is unset — this process likely did not start from a real \
                 login session (su/sudo/machinectl shells don't create one). \
                 logind/seatd will not promote us to the active session."
            );
        }
        _ => {}
    }

    if let Some(session_id) = xdg_session_id.as_deref() {
        log_loginctl_session(session_id);
    }
}

fn log_loginctl_session(session_id: &str) {
    let output = match std::process::Command::new("loginctl")
        .args([
            "show-session",
            session_id,
            "-p",
            "Active",
            "-p",
            "State",
            "-p",
            "Type",
            "-p",
            "Class",
        ])
        .output()
    {
        Ok(output) if output.status.success() => output.stdout,
        Ok(output) => {
            tracing::debug!(stderr = %String::from_utf8_lossy(&output.stderr), "loginctl returned non-zero");
            return;
        }
        Err(error) => {
            tracing::debug!(%error, "could not invoke loginctl");
            return;
        }
    };
    let text = String::from_utf8_lossy(&output);
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::to_owned)
    };
    let active = value("Active=");
    tracing::info!(
        session_id,
        active = ?active.as_deref(),
        state = ?value("State=").as_deref(),
        type_ = ?value("Type=").as_deref(),
        class = ?value("Class=").as_deref(),
        "logind live session state (loginctl)"
    );
    if active.as_deref() == Some("no") {
        tracing::warn!(
            "logind reports Active=no for our session. drmSetMaster will be denied. \
             The session wrapper should run `loginctl activate $XDG_SESSION_ID` to \
             promote the session — required for Type=tty sessions (agetty+login on \
             a TTY) because logind doesn't auto-promote those."
        );
    }
}

fn current_master_diagnostic(card: &std::path::Path, holders: &[(u32, String)]) -> String {
    let foreground_vt = std::fs::read_to_string("/sys/class/tty/tty0/active")
        .ok()
        .map(|active| active.trim().to_string())
        .unwrap_or_else(|| "<unknown>".into());
    let xdg_vtnr = std::env::var("XDG_VTNR").unwrap_or_else(|_| "<unset>".into());
    let xdg_session_id = std::env::var("XDG_SESSION_ID").unwrap_or_else(|_| "<unset>".into());

    let mut logind_active = "<unknown>".to_string();
    let mut logind_state = "<unknown>".to_string();
    if xdg_session_id != "<unset>" {
        if let Ok(output) = std::process::Command::new("loginctl")
            .args([
                "show-session",
                &xdg_session_id,
                "-p",
                "Active",
                "-p",
                "State",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("Active=") {
                    logind_active = value.into();
                }
                if let Some(value) = line.strip_prefix("State=") {
                    logind_state = value.into();
                }
            }
        }
    }

    let holders = if holders.is_empty() {
        "<none in our UID — run `sudo fuser -v` on the card to see other UIDs>".to_string()
    } else {
        holders
            .iter()
            .map(|(pid, comm)| format!("{pid}({comm})"))
            .collect::<Vec<_>>()
            .join(",")
    };

    let hint = match logind_active.as_str() {
        "yes" => "logind says we ARE active but the kernel refused. This is rare: either another \
                  process of a different UID still holds the master (run `sudo lsof <card>` and \
                  `sudo cat /sys/kernel/debug/dri/<minor>/clients`), or there's a kernel/driver \
                  bug. A reboot usually clears it.",
        "no" => "logind reports Active=no for our session — that's why drmSetMaster was denied. \
                 Most common cause on Type=tty sessions (agetty+login): logind doesn't auto-promote \
                 to active. The wrapper should run `loginctl activate $XDG_SESSION_ID` before \
                 starting the compositor. If it does and this still happens, another graphical \
                 session is already claiming Active on seat0 and logind is refusing to transfer.",
        _ => "could not read logind's session state — is systemd-logind running and loginctl \
              on PATH?",
    };

    format!(
        "card={} foreground_vt={} XDG_VTNR={} XDG_SESSION_ID={} \
         logind.ACTIVE={} logind.STATE={} other_holders=[{}]. {}",
        card.display(),
        foreground_vt,
        xdg_vtnr,
        xdg_session_id,
        logind_active,
        logind_state,
        holders,
        hint
    )
}
