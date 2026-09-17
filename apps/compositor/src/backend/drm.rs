//! DRM/KMS backend — used when launching from a TTY (no parent compositor).

use std::{
    os::fd::{AsFd, BorrowedFd},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use smithay::reexports::input::{AccelProfile, Device as LibinputDevice, Libinput};
use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, GbmBufferedSurface, NodeType},
        egl::{EGLContext, EGLDisplay},
        input::{
            AbsolutePositionEvent, ButtonState, Device as InputDevice, DeviceCapability, Event,
            InputEvent, KeyState, KeyboardKeyEvent, PointerButtonEvent, PointerMotionEvent,
        },
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            gles::GlesRenderer, utils::draw_render_elements, Bind, Color32F, Frame, Renderer,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::UdevBackend,
    },
    input::keyboard::FilterResult,
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop,
        },
        drm::{
            self,
            control::{connector, crtc, Device as ControlDevice, ModeTypeFlags},
        },
        rustix::fs::OFlags,
        wayland_server::Display,
    },
    utils::{DeviceFd, Logical, Physical, Point, Rectangle, Transform, SERIAL_COUNTER},
};
use wayland_server::ListeningSocket;

use crate::{
    config::{
        CompositorConfig, ConfigPaths, ConfigWatcher, InputConfig, OutputConfig, OutputTransform,
    },
    decorations::RoundedCornerShaders,
    input::{
        begin_window_drag, handle_decoration_press, lower_layer_surface_under, move_dragged_window,
        upper_layer_surface_under, window_surface_under, window_under_including_decoration,
        WindowDrag,
    },
    integrations,
    render::{
        bottom_layer_elements, draw_window, ensure_rounded_corner_shader, popup_elements,
        top_layer_elements, window_content_elements, BACKGROUND_COLOR,
    },
    shortcuts::{physical_vt_from_keycode, update_physical_mods, vt_from_keysym, PhysicalMods},
    state::{BlairState, ClientState},
};

const BTN_LEFT: u32 = 0x110;

struct LoopData {
    state: BlairState,
    display: Display<BlairState>,
    session: LibSeatSession,
    start_time: std::time::Instant,
    need_frame: bool,
    session_active: bool,
    running: bool,
    frame_counter: Arc<AtomicU64>,

    // libseat activation must precede DRM master acquisition.
    drm: Option<DrmDevice>,
    renderer: Option<GlesRenderer>,
    rounded_corner_shader: Option<RoundedCornerShaders>,
    surface: Option<GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>>,
    output: Option<Output>,
    physical_mods: PhysicalMods,
    libinput: Option<Libinput>,
    input_events_seen: Arc<AtomicU64>,
    debug_overlay: DebugOverlay,
    drag: Option<WindowDrag>,
    left_button_down: bool,
}

struct MasterProbeFd(DeviceFd);

#[derive(Debug, Default)]
struct DebugOverlay {
    device_count: usize,
    key_events: u64,
    pointer_events: u64,
    last_key: String,
    last_shortcut: String,
    last_device: String,
    last_pointer: (i32, i32),
}

#[derive(Debug, Clone)]
struct DebugOverlaySnapshot {
    session_active: bool,
    input_events_seen: u64,
    shortcut_count: usize,
    device_count: usize,
    key_events: u64,
    pointer_events: u64,
    last_key: String,
    last_shortcut: String,
    last_device: String,
    last_pointer: (i32, i32),
}

impl DebugOverlay {
    fn snapshot(
        &self,
        session_active: bool,
        input_events_seen: u64,
        shortcut_count: usize,
    ) -> DebugOverlaySnapshot {
        DebugOverlaySnapshot {
            session_active,
            input_events_seen,
            shortcut_count,
            device_count: self.device_count,
            key_events: self.key_events,
            pointer_events: self.pointer_events,
            last_key: self.last_key.clone(),
            last_shortcut: self.last_shortcut.clone(),
            last_device: self.last_device.clone(),
            last_pointer: self.last_pointer,
        }
    }
}

impl AsFd for MasterProbeFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for MasterProbeFd {}

pub fn run(config: CompositorConfig) -> Result<()> {
    diagnose_seat_environment();

    let (session, session_notifier) = LibSeatSession::new()
        .context("failed to open libseat session — is seatd/logind running?")?;
    let mut session_for_open = session.clone();
    let session_for_libinput = session.clone();
    let seat_name = session.seat();
    tracing::info!(
        seat = %seat_name,
        already_active = session.is_active(),
        "libseat session opened"
    );

    let udev = UdevBackend::new(&seat_name).context("failed to init udev")?;
    let cards = enumerate_drm_cards(&udev);
    log_drm_cards(&cards);
    let device_path =
        pick_best_drm_card(&cards).context("no DRM card with a connected output was found")?;
    tracing::info!(device = %device_path.display(), "selected DRM device");

    let display: Display<BlairState> =
        Display::new().context("failed to create Wayland display")?;
    let dh = display.handle();

    let mut event_loop: EventLoop<'static, LoopData> =
        EventLoop::try_new().context("failed to create calloop event loop")?;
    let handle = event_loop.handle();
    let loop_signal = event_loop.get_signal();

    let mut integrations = integrations::Integrations::start(config.integrations.dbus);
    let mut config_watcher = start_config_watcher(&config);
    let state = BlairState::new(
        dh.clone(),
        loop_signal,
        config,
        integrations.event_channel(),
    );

    let frame_counter = Arc::new(AtomicU64::new(0));
    let input_events_seen = Arc::new(AtomicU64::new(0));
    let initial_active = session.is_active();

    let mut loop_data = LoopData {
        state,
        display,
        session,
        start_time: std::time::Instant::now(),
        need_frame: false,
        session_active: initial_active,
        running: true,
        frame_counter: Arc::clone(&frame_counter),
        drm: None,
        renderer: None,
        rounded_corner_shader: None,
        surface: None,
        output: None,
        physical_mods: PhysicalMods::default(),
        libinput: None,
        input_events_seen: Arc::clone(&input_events_seen),
        debug_overlay: DebugOverlay::default(),
        drag: None,
        left_button_down: false,
    };

    handle
        .insert_source(
            session_notifier,
            |event, _, data: &mut LoopData| match event {
                SessionEvent::PauseSession => {
                    tracing::info!("session paused (VT switch out)");
                    data.session_active = false;
                    data.need_frame = false;
                    if let Some(libinput) = data.libinput.as_mut() {
                        libinput.suspend();
                        tracing::debug!("libinput suspended");
                    }
                    if let Some(drm) = data.drm.as_mut() {
                        drm.pause();
                    }
                }
                SessionEvent::ActivateSession => {
                    tracing::info!("session activated");
                    data.session_active = true;
                    data.physical_mods = PhysicalMods::default();
                    data.need_frame = true;
                    if let Some(libinput) = data.libinput.as_mut() {
                        if libinput.resume().is_ok() {
                            tracing::info!("libinput resumed");
                        } else {
                            tracing::warn!("failed to resume libinput after VT switch");
                        }
                    }
                    if let Some(drm) = data.drm.as_mut() {
                        if let Err(err) = drm.activate(false) {
                            tracing::error!(%err, "failed to reactivate DRM after VT switch");
                            data.running = false;
                        }
                    }
                }
            },
        )
        .map_err(|err| anyhow::anyhow!("session notifier: {err:?}"))?;

    if !loop_data.session_active {
        tracing::info!("waiting for libseat ActivateSession event");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !loop_data.session_active {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
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
            event_loop.dispatch(
                Some(remaining.min(Duration::from_millis(200))),
                &mut loop_data,
            )?;
        }
        tracing::info!("libseat session is now active");
    }

    let raw_device_fd = session_for_open
        .open(
            &device_path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .context("failed to open DRM device")?;
    let raw_device_fd = DeviceFd::from(raw_device_fd);
    let master_probe = MasterProbeFd(raw_device_fd.clone());
    let master_acquired =
        match acquire_master_with_retry(&master_probe, 5, Duration::from_millis(200)) {
            Ok(()) => {
                tracing::info!("DRM master acquired");
                true
            }
            Err(err) => {
                let holders = enumerate_card_holders(&device_path);
                if !holders.is_empty() {
                    tracing::warn!(?holders, "other holders of the DRM card in our UID");
                }
                let diagnostic = current_master_diagnostic(&device_path, &holders);
                tracing::warn!(
                    error = %err,
                    diagnostic = %diagnostic,
                    "drmSetMaster probe failed; continuing with libseat/logind brokered DRM fd"
                );
                false
            }
        };
    if master_acquired {
        if let Err(err) = drm::Device::release_master_lock(&master_probe) {
            tracing::debug!(%err, "failed to release DRM master probe before Smithay wrap");
        }
    }
    drop(master_probe);

    let device_fd = DrmDeviceFd::new(raw_device_fd);

    let (mut drm, drm_notifier) =
        DrmDevice::new(device_fd.clone(), false).context("failed to create DRM device")?;

    let gbm = GbmDevice::new(device_fd.clone()).context("failed to create GBM device")?;

    let resources = device_fd
        .resource_handles()
        .context("failed to get DRM resource handles")?;
    let (conn_handle, drm_mode, crtc_handle, output_name) = find_output(
        &device_fd,
        &resources,
        drm.crtcs(),
        &loop_data.state.config.outputs,
    )
    .context("no connected display found")?;
    let conn_info = device_fd
        .get_connector(conn_handle, false)
        .context("failed to read connector info")?;
    let phys_mm = conn_info.size().unwrap_or((0, 0));
    let (w, h) = (drm_mode.size().0 as i32, drm_mode.size().1 as i32);
    tracing::info!(
        width = w,
        height = h,
        refresh = drm_mode.vrefresh(),
        "DRM output ready"
    );

    let drm_surface = drm
        .create_surface(crtc_handle, drm_mode, &[conn_handle])
        .context("failed to create DRM surface")?;

    let egl_display =
        unsafe { EGLDisplay::new(gbm.clone()).context("failed to create EGL display")? };
    let egl_context = EGLContext::new(&egl_display).context("failed to create EGL context")?;
    let renderer =
        unsafe { GlesRenderer::new(egl_context).context("failed to create GLES renderer")? };

    let renderer_formats = renderer
        .egl_context()
        .display()
        .dmabuf_render_formats()
        .clone();
    let gbm_alloc = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let surface = GbmBufferedSurface::<_, ()>::new(
        drm_surface,
        gbm_alloc,
        &[Fourcc::Xrgb8888, Fourcc::Argb8888],
        renderer_formats,
    )
    .context("failed to create GBM buffered surface")?;
    let output_config = loop_data.state.config.outputs.get(&output_name);
    if let Some(vrr) = output_config.and_then(|config| config.vrr) {
        if let Err(error) = surface.use_vrr(vrr) {
            tracing::warn!(%error, output = %output_name, vrr, "failed to configure VRR");
        }
    }

    let listener =
        ListeningSocket::bind_auto("wayland", 1..33).context("failed to bind Wayland socket")?;
    let socket_name = listener
        .socket_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wayland-1".to_string());
    tracing::info!(socket = %socket_name, "Wayland socket ready");
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);

    let mut clients: Vec<wayland_server::Client> = Vec::new();

    let output = Output::new(
        output_name.clone(),
        PhysicalProperties {
            size: (phys_mm.0 as i32, phys_mm.1 as i32).into(),
            subpixel: Subpixel::Unknown,
            make: "Unknown".to_string(),
            model: "DRM".to_string(),
        },
    );
    let refresh = (drm_mode.vrefresh() * 1000) as i32;
    let out_mode = OutputMode {
        size: (w, h).into(),
        refresh,
    };
    let location = output_config
        .and_then(|config| config.position)
        .unwrap_or([0, 0]);
    let transform = output_config
        .and_then(|config| config.parsed_transform().ok().flatten())
        .map(to_smithay_transform)
        .unwrap_or(Transform::Normal);
    let scale = output_config
        .and_then(|config| config.scale)
        .map(Scale::Fractional);
    output.change_current_state(
        Some(out_mode),
        Some(transform),
        scale,
        Some((location[0], location[1]).into()),
    );
    output.set_preferred(out_mode);
    output.create_global::<BlairState>(&dh);
    loop_data
        .state
        .add_output(&output, (location[0], location[1]).into());

    loop_data.drm = Some(drm);
    loop_data.renderer = Some(renderer);
    loop_data.surface = Some(surface);
    loop_data.output = Some(output);

    loop_data.state.spawn_primary_client();

    handle
        .insert_source(drm_notifier, |event, _, data: &mut LoopData| match event {
            DrmEvent::VBlank(_crtc) => {
                if let Some(surface) = data.surface.as_mut() {
                    if let Err(err) = surface.frame_submitted() {
                        tracing::warn!("frame_submitted error: {err}");
                    }
                }
                if data.session_active {
                    data.need_frame = true;
                }
            }
            DrmEvent::Error(err) => tracing::warn!("DRM error: {err}"),
        })
        .map_err(|err| anyhow::anyhow!("DRM notifier: {err:?}"))?;

    let mut libinput_ctx =
        Libinput::new_with_udev(LibinputSessionInterface::from(session_for_libinput));
    libinput_ctx
        .udev_assign_seat(&seat_name)
        .map_err(|_| anyhow::anyhow!("failed to assign libinput seat"))?;
    loop_data.libinput = Some(libinput_ctx.clone());
    tracing::info!(seat = %seat_name, "libinput seat assigned");
    handle
        .insert_source(
            LibinputInputBackend::new(libinput_ctx),
            |event, _, data: &mut LoopData| {
                data.display.dispatch_clients(&mut data.state).ok();
                data.display.flush_clients().ok();
                drain_pending_move_request(data);

                let index = data.input_events_seen.fetch_add(1, Ordering::Relaxed) + 1;
                if index <= 20 {
                    log_input_event_summary(index, &event);
                }
                handle_input(event, data);
                data.display.flush_clients().ok();
            },
        )
        .map_err(|err| anyhow::anyhow!("libinput: {err:?}"))?;

    handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(50)),
            |_, _, data: &mut LoopData| {
                data.need_frame = true;
                TimeoutAction::Drop
            },
        )
        .map_err(|err| anyhow::anyhow!("timer: {err:?}"))?;

    spawn_render_watchdog(Arc::clone(&frame_counter), Duration::from_secs(5));
    spawn_input_watchdog(Arc::clone(&input_events_seen), Duration::from_secs(3));

    tracing::info!("entering DRM event loop");

    while loop_data.running {
        event_loop.dispatch(Some(Duration::from_millis(16)), &mut loop_data)?;

        if let Some(watcher) = config_watcher.as_mut() {
            watcher.reload_if_due(&mut loop_data.state);
        }

        if let Ok(Some(stream)) = listener.accept() {
            match loop_data
                .display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                Ok(client) => {
                    clients.push(client);
                    tracing::debug!("Wayland client connected");
                }
                Err(err) => tracing::warn!(%err, "failed to insert Wayland client"),
            }
        }

        loop_data
            .display
            .dispatch_clients(&mut loop_data.state)
            .ok();
        loop_data.display.flush_clients().ok();
        drain_pending_move_request(&mut loop_data);
        integrations.drain(&mut loop_data.state);
        if loop_data.state.take_redraw_request() {
            loop_data.need_frame = true;
        }
        if loop_data.state.exit_requested {
            loop_data.running = false;
        }
        loop_data.state.space.refresh();
        loop_data.state.popup_manager.cleanup();

        if loop_data.need_frame && loop_data.session_active {
            loop_data.need_frame = false;
            let debug_overlay = loop_data.debug_overlay.snapshot(
                loop_data.session_active,
                loop_data.input_events_seen.load(Ordering::Relaxed),
                loop_data.state.shortcuts.binding_count(),
            );
            let LoopData {
                renderer: Some(renderer),
                rounded_corner_shader,
                surface: Some(surface),
                state,
                output: Some(output),
                start_time,
                frame_counter,
                ..
            } = &mut loop_data
            else {
                tracing::warn!("DRM stack not initialised — skipping frame");
                continue;
            };
            if render_frame(
                renderer,
                rounded_corner_shader,
                surface,
                state,
                output,
                *start_time,
                &debug_overlay,
            ) {
                frame_counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    tracing::info!(
        frames = loop_data.frame_counter.load(Ordering::Relaxed),
        "DRM compositor exiting"
    );
    Ok(())
}

fn start_config_watcher(config: &CompositorConfig) -> Option<ConfigWatcher> {
    if !config.general.hot_reload {
        tracing::info!("configuration hot reload disabled");
        return None;
    }

    match ConfigWatcher::new(&ConfigPaths::default()) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            tracing::error!(%error, "failed to start config watcher; continuing without hot reload");
            None
        }
    }
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
                let _ = std::io::Write::flush(&mut std::io::stdout().lock());
                std::process::exit(124);
            }
        })
        .expect("failed to spawn watchdog thread");
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
        .expect("failed to spawn input watchdog thread");
}

fn drain_pending_move_request(data: &mut LoopData) {
    let Some(window) = data.state.take_pending_move_request() else {
        return;
    };
    if data.drag.is_some() {
        tracing::trace!("ignoring duplicate xdg move request during active drag");
        return;
    }
    if !data.left_button_down {
        tracing::debug!("discarding stale xdg move request after button release");
        return;
    }
    let Some(pointer) = data.state.seat.get_pointer() else {
        tracing::warn!("xdg move requested before pointer was available");
        return;
    };
    let pos = pointer.current_location();
    if begin_window_drag(&mut data.state, &mut data.drag, window, pos) {
        tracing::debug!(
            x = pos.x,
            y = pos.y,
            "started DRM window drag from xdg move request"
        );
        data.need_frame = true;
    }
}

fn render_frame(
    renderer: &mut GlesRenderer,
    rounded_corner_shader: &mut Option<RoundedCornerShaders>,
    surface: &mut GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    state: &mut BlairState,
    output: &Output,
    start_time: std::time::Instant,
    debug_overlay: &DebugOverlaySnapshot,
) -> bool {
    tracing::trace!(
        session_active = debug_overlay.session_active,
        input_events_seen = debug_overlay.input_events_seen,
        shortcut_count = debug_overlay.shortcut_count,
        device_count = debug_overlay.device_count,
        key_events = debug_overlay.key_events,
        pointer_events = debug_overlay.pointer_events,
        last_key = %debug_overlay.last_key,
        last_shortcut = %debug_overlay.last_shortcut,
        last_device = %debug_overlay.last_device,
        last_pointer = ?debug_overlay.last_pointer,
        "frame debug snapshot"
    );

    let (mut dmabuf, _age) = match surface.next_buffer() {
        Ok(buffer) => buffer,
        Err(err) => {
            tracing::warn!("next_buffer: {err}");
            return false;
        }
    };

    let bottom_elements = bottom_layer_elements(renderer, output);
    let window_content = window_content_elements(renderer, state, output);
    let top_elements = top_layer_elements(renderer, output);
    let popups = popup_elements(renderer, state, output);
    let corner_shader = ensure_rounded_corner_shader(renderer, rounded_corner_shader);

    let mut framebuffer = match renderer.bind(&mut dmabuf) {
        Ok(framebuffer) => framebuffer,
        Err(err) => {
            tracing::warn!("renderer.bind: {err}");
            return false;
        }
    };

    let size = state
        .space
        .output_geometry(output)
        .map(|geometry| geometry.size)
        .unwrap_or_default();

    if size.w == 0 || size.h == 0 {
        tracing::warn!("output has zero size, skipping render");
        return false;
    }

    let phys: smithay::utils::Size<i32, Physical> = (size.w, size.h).into();
    let damage = Rectangle::from_size(phys);

    match renderer.render(&mut framebuffer, phys, output.current_transform()) {
        Ok(mut frame) => {
            if let Err(err) = frame.clear(BACKGROUND_COLOR, &[damage]) {
                tracing::warn!("frame.clear: {err}");
            }
            if let Err(err) = draw_render_elements(&mut frame, 1.0, &bottom_elements, &[damage]) {
                tracing::warn!("draw_render_elements: {err}");
            }
            for (window, content) in &window_content {
                if let Err(err) = draw_window(
                    &mut frame,
                    state,
                    window,
                    content,
                    &[damage],
                    corner_shader.as_ref(),
                ) {
                    tracing::warn!(%err, "failed to draw window");
                }
            }
            if let Err(err) = draw_render_elements(&mut frame, 1.0, &top_elements, &[damage]) {
                tracing::warn!("draw_render_elements: {err}");
            }
            if let Err(err) = draw_render_elements(&mut frame, 1.0, &popups, &[damage]) {
                tracing::warn!("draw_render_elements: {err}");
            }
            #[cfg(debug_assertions)]
            if let Err(err) = draw_debug_overlay(&mut frame, &[damage], debug_overlay) {
                tracing::warn!("debug overlay: {err}");
            }
            if let Err(err) = draw_software_cursor(&mut frame, &[damage], state) {
                tracing::warn!("software cursor: {err}");
            }
            if let Err(err) = frame.finish() {
                tracing::warn!("frame.finish: {err}");
                return false;
            }
        }
        Err(err) => {
            tracing::warn!("renderer.render: {err}");
            return false;
        }
    }

    crate::render::send_frame_callbacks(state, start_time.elapsed().as_millis() as u32);

    drop(framebuffer);

    if let Err(err) = surface.queue_buffer(None, None, ()) {
        tracing::warn!("queue_buffer: {err}");
        return false;
    }

    true
}

fn handle_input(event: InputEvent<LibinputInputBackend>, data: &mut LoopData) {
    let LoopData {
        state,
        session,
        physical_mods,
        debug_overlay,
        drag,
        left_button_down,
        need_frame,
        ..
    } = data;
    match event {
        InputEvent::DeviceAdded { mut device } => {
            configure_input_device(&mut device, &state.config.input);
            debug_overlay.device_count += 1;
            debug_overlay.last_device = device.name().to_string();
            log_input_device("input device added", &device);
        }
        InputEvent::DeviceRemoved { device } => {
            debug_overlay.device_count = debug_overlay.device_count.saturating_sub(1);
            debug_overlay.last_device = format!("removed {}", device.name());
            log_input_device("input device removed", &device);
        }
        InputEvent::Keyboard { event } => {
            let keycode = event.key_code();
            let keycode_u32 = u32::from(keycode);
            let key_state = event.state();
            let time = event.time_msec();
            let pressed = key_state == KeyState::Pressed;
            debug_overlay.key_events += 1;
            debug_overlay.last_key = format!("{keycode_u32}:{key_state:?}");

            update_physical_mods(physical_mods, keycode_u32, pressed);
            state.shortcuts.update_key(keycode_u32, pressed);
            #[cfg(debug_assertions)]
            if pressed && physical_mods.ctrl && physical_mods.logo && keycode_u32 == 9 {
                tracing::warn!("debug emergency exit requested by Ctrl+Super+Esc");
                debug_overlay.last_shortcut = "debug exit".to_string();
                state.request_exit();
                return;
            }
            if pressed && physical_mods.ctrl && physical_mods.alt {
                if let Some(vt) = physical_vt_from_keycode(keycode_u32) {
                    tracing::info!(vt, keycode = keycode_u32, "physical VT switch requested");
                    debug_overlay.last_shortcut = format!("vt {vt}");
                    if let Err(err) = session.change_vt(vt) {
                        tracing::warn!(%err, vt, "VT switch failed");
                    }
                    return;
                }
            }
            let activated = state.shortcuts.maybe_activate_physical(*physical_mods);
            if pressed && !activated.is_empty() {
                debug_overlay.last_shortcut = activated[0].id.clone();
                state.activate_shortcuts(activated);
                return;
            }

            if let Some(keyboard) = state.seat.get_keyboard() {
                let serial = SERIAL_COUNTER.next_serial();
                keyboard.input::<(), _>(
                    state,
                    keycode,
                    key_state,
                    serial,
                    time,
                    |_state, mods, keysym| {
                        let raw_sym = keysym
                            .raw_latin_sym_or_raw_current_sym()
                            .unwrap_or_else(|| keysym.modified_sym());
                        if key_state == KeyState::Pressed && mods.ctrl && mods.alt {
                            let vt = vt_from_keysym(raw_sym)
                                .or_else(|| vt_from_keysym(keysym.modified_sym()));
                            if let Some(vt) = vt {
                                tracing::info!(vt, "VT switch requested");
                                if let Err(err) = session.change_vt(vt) {
                                    tracing::warn!(%err, vt, "VT switch failed");
                                }
                                return FilterResult::Intercept(());
                            }
                        }
                        FilterResult::Forward
                    },
                );
            }
        }
        InputEvent::PointerMotion { event } => {
            debug_overlay.pointer_events += 1;
            if let Some(pointer) = state.seat.get_pointer() {
                let serial = SERIAL_COUNTER.next_serial();
                let pos = clamp_pointer_position(state, pointer.current_location() + event.delta());
                state.set_focused_output_at(pos);
                debug_overlay.last_pointer = (pos.x.round() as i32, pos.y.round() as i32);
                if move_dragged_window(state, drag.as_ref(), pos) {
                    *need_frame = true;
                }
                let focus = upper_layer_surface_under(state, pos)
                    .map(|(surface, loc, _)| (surface, loc))
                    .or_else(|| window_surface_under(state, pos))
                    .or_else(|| {
                        lower_layer_surface_under(state, pos)
                            .map(|(surface, loc, _)| (surface, loc))
                    });
                pointer.motion(
                    state,
                    focus,
                    &smithay::input::pointer::MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(state);
            }
        }
        InputEvent::PointerMotionAbsolute { event } => {
            debug_overlay.pointer_events += 1;
            let output = state.space.outputs().next().cloned();
            if let Some(output) = output {
                let output_geo = state.space.output_geometry(&output).unwrap_or_default();
                let pos =
                    clamp_pointer_position(state, event.position_transformed(output_geo.size));
                state.set_focused_output_at(pos);
                debug_overlay.last_pointer = (pos.x.round() as i32, pos.y.round() as i32);
                if let Some(pointer) = state.seat.get_pointer() {
                    if move_dragged_window(state, drag.as_ref(), pos) {
                        *need_frame = true;
                    }
                    let serial = SERIAL_COUNTER.next_serial();
                    let focus = upper_layer_surface_under(state, pos)
                        .map(|(surface, loc, _)| (surface, loc))
                        .or_else(|| window_surface_under(state, pos))
                        .or_else(|| {
                            lower_layer_surface_under(state, pos)
                                .map(|(surface, loc, _)| (surface, loc))
                        });
                    pointer.motion(
                        state,
                        focus,
                        &smithay::input::pointer::MotionEvent {
                            location: pos,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    pointer.frame(state);
                }
            }
        }
        InputEvent::PointerButton { event } => {
            use smithay::input::pointer::ButtonEvent;
            debug_overlay.pointer_events += 1;
            if let Some(pointer) = state.seat.get_pointer() {
                let serial = SERIAL_COUNTER.next_serial();
                if event.button_code() == BTN_LEFT {
                    match event.state() {
                        ButtonState::Pressed => *left_button_down = true,
                        ButtonState::Released => {
                            *left_button_down = false;
                            if drag.take().is_some() {
                                tracing::debug!("finished DRM window drag on button release");
                                *need_frame = true;
                            }
                        }
                    }
                }
                if event.state() == ButtonState::Pressed {
                    let pos = pointer.current_location();
                    if let Some((surface, _, can_focus)) = upper_layer_surface_under(state, pos) {
                        tracing::debug!(
                            x = pos.x,
                            y = pos.y,
                            can_focus,
                            "click hit a layer surface"
                        );
                        if can_focus {
                            if let Some(keyboard) = state.seat.get_keyboard() {
                                keyboard.set_focus(state, Some(surface), serial);
                            }
                        } else if let Some(keyboard) = state.seat.get_keyboard() {
                            keyboard.set_focus(state, None, serial);
                        }
                    } else if let Some(window) = window_under_including_decoration(state, pos) {
                        tracing::debug!(x = pos.x, y = pos.y, "click hit a window");
                        state.focus_window(&window);
                        if event.button_code() == BTN_LEFT
                            && handle_decoration_press(state, drag, window.clone(), pos)
                        {
                            *need_frame = true;
                            return;
                        }
                        if event.button_code() == BTN_LEFT && physical_mods.logo {
                            *need_frame = begin_window_drag(state, drag, window, pos);
                            return;
                        }
                    } else if let Some((surface, _, can_focus)) =
                        lower_layer_surface_under(state, pos)
                    {
                        tracing::debug!(
                            x = pos.x,
                            y = pos.y,
                            can_focus,
                            "click hit a lower layer surface"
                        );
                        if can_focus {
                            if let Some(keyboard) = state.seat.get_keyboard() {
                                keyboard.set_focus(state, Some(surface), serial);
                            }
                        } else if let Some(keyboard) = state.seat.get_keyboard() {
                            keyboard.set_focus(state, None, serial);
                        }
                    } else {
                        tracing::debug!(x = pos.x, y = pos.y, "click hit nothing — clearing focus");
                        if let Some(keyboard) = state.seat.get_keyboard() {
                            keyboard.set_focus(state, None, serial);
                        }
                    }
                }
                let pos = pointer.current_location();
                let focus = upper_layer_surface_under(state, pos)
                    .map(|(surface, loc, _)| (surface, loc))
                    .or_else(|| window_surface_under(state, pos))
                    .or_else(|| {
                        lower_layer_surface_under(state, pos)
                            .map(|(surface, loc, _)| (surface, loc))
                    });
                pointer.motion(
                    state,
                    focus,
                    &smithay::input::pointer::MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.button(
                    state,
                    &ButtonEvent {
                        button: event.button_code(),
                        state: event.state(),
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(state);
            }
        }
        _ => {}
    }
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

fn log_input_event_summary(index: u64, event: &InputEvent<LibinputInputBackend>) {
    match event {
        InputEvent::DeviceAdded { device } => {
            tracing::info!(index, name = %device.name(), id = %device.id(), "libinput event: device added");
        }
        InputEvent::DeviceRemoved { device } => {
            tracing::info!(index, name = %device.name(), id = %device.id(), "libinput event: device removed");
        }
        InputEvent::Keyboard { event } => {
            tracing::info!(index, keycode = u32::from(event.key_code()), state = ?event.state(), "libinput event: keyboard");
        }
        InputEvent::PointerMotion { event } => {
            tracing::info!(
                index,
                dx = event.delta_x(),
                dy = event.delta_y(),
                "libinput event: pointer motion"
            );
        }
        InputEvent::PointerMotionAbsolute { event } => {
            tracing::info!(
                index,
                x = event.x(),
                y = event.y(),
                "libinput event: absolute pointer motion"
            );
        }
        InputEvent::PointerButton { event } => {
            tracing::info!(index, button = event.button_code(), state = ?event.state(), "libinput event: pointer button");
        }
        _ => {
            tracing::info!(index, "libinput event: other");
        }
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

fn clamp_pointer_position(state: &BlairState, pos: Point<f64, Logical>) -> Point<f64, Logical> {
    let Some(output) = state.space.outputs().next() else {
        return pos;
    };
    let Some(geo) = state.space.output_geometry(output) else {
        return pos;
    };

    let min_x = geo.loc.x as f64;
    let min_y = geo.loc.y as f64;
    let max_x = min_x + geo.size.w.saturating_sub(1) as f64;
    let max_y = min_y + geo.size.h.saturating_sub(1) as f64;
    (pos.x.clamp(min_x, max_x), pos.y.clamp(min_y, max_y)).into()
}

fn draw_software_cursor<F: Frame>(
    frame: &mut F,
    damage: &[Rectangle<i32, Physical>],
    state: &BlairState,
) -> Result<(), F::Error> {
    let Some(pointer) = state.seat.get_pointer() else {
        return Ok(());
    };
    let loc = pointer.current_location();
    let x = loc.x.round() as i32;
    let y = loc.y.round() as i32;
    let white = Color32F::new(0.92, 0.96, 1.0, 1.0);
    let black = Color32F::new(0.02, 0.03, 0.04, 1.0);

    let shadow = [
        phys_rect(x + 1, y + 1, 3, 18),
        phys_rect(x + 1, y + 1, 14, 3),
        phys_rect(x + 5, y + 6, 9, 3),
        phys_rect(x + 8, y + 9, 6, 3),
        phys_rect(x + 11, y + 12, 4, 3),
    ];
    frame.clear(black, &shadow)?;

    let cursor = [
        phys_rect(x, y, 2, 17),
        phys_rect(x, y, 13, 2),
        phys_rect(x + 4, y + 5, 8, 2),
        phys_rect(x + 7, y + 8, 5, 2),
        phys_rect(x + 10, y + 11, 3, 2),
    ];
    frame.clear(white, &cursor)?;
    let _ = damage;
    Ok(())
}

#[cfg(debug_assertions)]
fn draw_debug_overlay<F: Frame>(
    frame: &mut F,
    damage: &[Rectangle<i32, Physical>],
    snapshot: &DebugOverlaySnapshot,
) -> Result<(), F::Error> {
    let lines = [
        "BLAIR DEBUG".to_string(),
        format!("SESSION ACTIVE: {}", yn(snapshot.session_active)),
        format!(
            "INPUT EVENTS: {} DEVICES: {}",
            snapshot.input_events_seen, snapshot.device_count
        ),
        format!(
            "KEY EVENTS: {} LAST: {}",
            snapshot.key_events,
            empty_dash(&snapshot.last_key)
        ),
        format!(
            "POINTER EVENTS: {} POS: {},{}",
            snapshot.pointer_events, snapshot.last_pointer.0, snapshot.last_pointer.1
        ),
        format!("SHORTCUTS: {}", snapshot.shortcut_count),
        format!("LAST SHORTCUT: {}", empty_dash(&snapshot.last_shortcut)),
        format!("LAST DEVICE: {}", empty_dash(&snapshot.last_device)),
    ];
    let scale = 2;
    let x = 18;
    let y = 18;
    let line_h = 20;
    let text_w = lines
        .iter()
        .map(|line| line.chars().count() as i32)
        .max()
        .unwrap_or(1)
        * 6
        * scale;
    let panel = phys_rect(x - 8, y - 8, text_w + 18, line_h * lines.len() as i32 + 12);
    frame.clear(Color32F::new(0.0, 0.0, 0.0, 0.82), &[panel])?;

    for (idx, line) in lines.iter().enumerate() {
        draw_debug_text(
            frame,
            x,
            y + idx as i32 * line_h,
            line,
            scale,
            damage,
            Color32F::new(0.6, 1.0, 0.78, 1.0),
        )?;
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn yn(value: bool) -> &'static str {
    if value {
        "YES"
    } else {
        "NO"
    }
}

#[cfg(debug_assertions)]
fn empty_dash(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

#[cfg(debug_assertions)]
fn draw_debug_text<F: Frame>(
    frame: &mut F,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    damage: &[Rectangle<i32, Physical>],
    color: Color32F,
) -> Result<(), F::Error> {
    let mut rects = Vec::new();
    let mut cursor_x = x;
    for ch in text.chars() {
        if ch == ' ' {
            cursor_x += 4 * scale;
            continue;
        }
        let glyph = glyph_rows(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) == 0 {
                    continue;
                }
                rects.push(phys_rect(
                    cursor_x + col * scale,
                    y + row as i32 * scale,
                    scale,
                    scale,
                ));
            }
        }
        cursor_x += 6 * scale;
    }
    if !rects.is_empty() {
        frame.clear(color, &rects)?;
    }
    let _ = damage;
    Ok(())
}

fn phys_rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
    Rectangle::new((x, y).into(), (w.max(1), h.max(1)).into())
}

#[cfg(debug_assertions)]
fn glyph_rows(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        ':' => [
            0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '_' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
        ',' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00100, 0b01000,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '+' => [
            0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000,
        ],
        '=' => [
            0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000,
        ],
        '!' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
        '>' => [
            0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000,
        ],
        _ => [
            0b11111, 0b10001, 0b00010, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
    }
}

fn diagnose_seat_environment() {
    let xdg_session_id = std::env::var("XDG_SESSION_ID").ok();
    let xdg_session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let xdg_session_class = std::env::var("XDG_SESSION_CLASS").ok();
    let xdg_seat = std::env::var("XDG_SEAT").ok();
    let xdg_vtnr = std::env::var("XDG_VTNR").ok();
    let foreground_vt = std::fs::read_to_string("/sys/class/tty/tty0/active")
        .ok()
        .map(|s| s.trim().to_string());

    tracing::info!(
        session_id = ?xdg_session_id.as_deref(),
        session_type = ?xdg_session_type.as_deref(),
        session_class = ?xdg_session_class.as_deref(),
        seat = ?xdg_seat.as_deref(),
        vtnr = ?xdg_vtnr.as_deref(),
        foreground_vt = ?foreground_vt.as_deref(),
        "seat/VT environment"
    );

    match (&xdg_vtnr, &foreground_vt) {
        (Some(vtnr), Some(fg)) if vtnr.trim() != fg.trim_start_matches("tty") => {
            tracing::warn!(
                vtnr,
                foreground = %fg,
                "session VT does not match foreground VT — logind will refuse to \
                 grant DRM master until the foreground VT switches to ours"
            );
        }
        (None, _) => {
            tracing::warn!(
                "XDG_VTNR is unset — this process likely did not start from a real \
                 login session (su/sudo/machinectl shell don't create one). \
                 logind/seatd will not promote us to active session."
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
        Err(err) => {
            tracing::debug!(%err, "could not invoke loginctl");
            return;
        }
    };
    let text = String::from_utf8_lossy(&output);
    let mut active = None;
    let mut state = None;
    let mut kind = None;
    let mut class = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "Active" => active = Some(value.to_string()),
            "State" => state = Some(value.to_string()),
            "Type" => kind = Some(value.to_string()),
            "Class" => class = Some(value.to_string()),
            _ => {}
        }
    }
    tracing::info!(
        session_id,
        active = ?active.as_deref(),
        state = ?state.as_deref(),
        type_ = ?kind.as_deref(),
        class = ?class.as_deref(),
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
        let Some(pid_str) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }

        let fd_dir = entry.path().join("fd");
        let Ok(fds) = std::fs::read_dir(&fd_dir) else {
            continue;
        };

        for fd_entry in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd_entry.path()) else {
                continue;
            };
            if target == card_canon {
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
            sysfs = ?card.sysfs.as_deref().map(|p| p.display().to_string()),
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

fn acquire_master_with_retry<D: drm::Device>(
    fd: &D,
    attempts: u32,
    backoff: Duration,
) -> std::io::Result<()> {
    let mut last_err = None;
    for attempt in 1..=attempts {
        match drm::Device::acquire_master_lock(fd) {
            Ok(()) => {
                if attempt > 1 {
                    tracing::info!(attempt, "DRM master acquired after retry");
                }
                return Ok(());
            }
            Err(err) => {
                tracing::warn!(attempt, error = %err, "drmSetMaster failed, retrying");
                last_err = Some(err);
                if attempt < attempts {
                    std::thread::sleep(backoff);
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("drmSetMaster failed")))
}

fn current_master_diagnostic(card: &std::path::Path, holders: &[(u32, String)]) -> String {
    let foreground_vt = std::fs::read_to_string("/sys/class/tty/tty0/active")
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "<unknown>".into());
    let xdg_vtnr = std::env::var("XDG_VTNR").unwrap_or_else(|_| "<unset>".into());
    let xdg_session_id = std::env::var("XDG_SESSION_ID").unwrap_or_else(|_| "<unset>".into());

    let mut logind_active = "<unknown>".to_string();
    let mut logind_state = "<unknown>".to_string();
    if xdg_session_id != "<unset>" {
        if let Ok(out) = std::process::Command::new("loginctl")
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
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("Active=") {
                    logind_active = v.into();
                }
                if let Some(v) = line.strip_prefix("State=") {
                    logind_state = v.into();
                }
            }
        }
    }

    let holders_str = if holders.is_empty() {
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
        holders_str,
        hint
    )
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
        for &conn_handle in resources.connectors() {
            let conn = fd.get_connector(conn_handle, false).ok()?;
            if conn.state() != connector::State::Connected {
                continue;
            }
            let name = conn.to_string();
            let config = outputs.get(&name);
            if config.and_then(|config| config.enabled) == Some(false) && !allow_disabled {
                continue;
            }
            let configured_mode = config.and_then(|config| config.parsed_mode().ok().flatten());
            let mode = configured_mode
                .and_then(|requested| {
                    conn.modes().iter().find(|mode| {
                        let size = mode.size();
                        size.0 as i32 == requested.width
                            && size.1 as i32 == requested.height
                            && ((mode.vrefresh() * 1000) as i32 - requested.refresh_millihz).abs()
                                <= 1
                    })
                })
                .or_else(|| {
                    conn.modes()
                        .iter()
                        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                })
                .or_else(|| conn.modes().first())
                .copied()?;
            if configured_mode.is_some()
                && configured_mode.is_some_and(|requested| {
                    let size = mode.size();
                    size.0 as i32 != requested.width
                        || size.1 as i32 != requested.height
                        || ((mode.vrefresh() * 1000) as i32 - requested.refresh_millihz).abs() > 1
                })
            {
                tracing::warn!(output = %name, requested = ?configured_mode, "requested mode is unavailable; using preferred mode");
            }

            for &enc_handle in conn.encoders() {
                let Ok(enc) = fd.get_encoder(enc_handle) else {
                    continue;
                };
                let filter = enc.possible_crtcs();
                let compatible = resources.filter_crtcs(filter);
                for &crtc_handle in crtcs {
                    if compatible.contains(&crtc_handle) {
                        if allow_disabled {
                            tracing::warn!(output = %name, "all configured outputs were disabled; keeping this output enabled");
                        }
                        return Some((conn_handle, mode, crtc_handle, name));
                    }
                }
            }
        }
    }
    None
}
