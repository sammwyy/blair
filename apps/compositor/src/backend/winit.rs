use std::sync::Arc;

use anyhow::{Context, Result};
use smithay::reexports::winit::platform::pump_events::PumpStatus;
use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, ButtonState, Event, InputEvent, KeyState, KeyboardKeyEvent,
            PointerButtonEvent,
        },
        renderer::{gles::GlesRenderer, utils::draw_render_elements, Frame, Renderer},
        winit::{self, WinitEvent, WinitInput},
    },
    input::keyboard::FilterResult,
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::Display,
    utils::{Rectangle, Transform, SERIAL_COUNTER},
};
use wayland_server::ListeningSocket;

use crate::{
    config::{CompositorConfig, ConfigPaths, ConfigWatcher},
    decorations::RoundedCornerShaders,
    input::{
        begin_window_drag, handle_decoration_press, lower_layer_surface_under, move_dragged_window,
        upper_layer_surface_under, window_surface_under, window_under_including_decoration,
        WindowDrag,
    },
    integrations,
    render::{
        bottom_layer_elements, draw_window, ensure_rounded_corner_shader, popup_elements,
        send_frame_callbacks, top_layer_elements, window_content_elements, BACKGROUND_COLOR,
    },
    state::{BlairState, ClientState},
};

const BTN_LEFT: u32 = 0x110;

pub fn run(config: CompositorConfig) -> Result<()> {
    let mut display: Display<BlairState> =
        Display::new().context("failed to create Wayland display")?;
    let dh = display.handle();

    let temp_loop = smithay::reexports::calloop::EventLoop::<'static, ()>::try_new()
        .context("failed to create event loop")?;
    let loop_signal = temp_loop.get_signal();
    drop(temp_loop);

    let mut integrations = integrations::Integrations::start(config.integrations.dbus);
    let mut config_watcher = start_config_watcher(&config);
    let mut state = BlairState::new(
        dh.clone(),
        loop_signal,
        config,
        integrations.event_channel(),
    );

    let (mut backend, mut winit) = winit::init::<GlesRenderer>()
        .map_err(|err| anyhow::anyhow!("failed to init winit backend: {err:?}"))?;

    let listener =
        ListeningSocket::bind_auto("wayland", 1..33).context("failed to bind Wayland socket")?;
    let socket_name = listener
        .socket_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wayland-1".to_string());
    tracing::info!(socket = %socket_name, "Wayland socket ready");
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);

    let mut clients = Vec::new();
    let mut rounded_corner_shader: Option<RoundedCornerShaders> = None;

    let output = Output::new(
        "winit-0".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Blair".to_string(),
            model: "Winit".to_string(),
        },
    );
    let win_size = backend.window_size();
    let output_mode = OutputMode {
        size: win_size,
        refresh: 60_000,
    };
    output.change_current_state(
        Some(output_mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(output_mode);
    output.create_global::<BlairState>(&dh);
    state.add_output(&output, (0, 0).into());

    tracing::info!(
        width = win_size.w,
        height = win_size.h,
        "winit output created"
    );

    state.spawn_primary_client();

    let start_time = std::time::Instant::now();
    let mut running = true;
    let mut drag: Option<WindowDrag> = None;

    tracing::info!("entering main loop");

    while running {
        let status = winit.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                let mode = OutputMode {
                    size,
                    refresh: 60_000,
                };
                output.change_current_state(Some(mode), None, None, None);
                state.output_resized(&output);
                tracing::debug!(w = size.w, h = size.h, "output resized");
            }
            WinitEvent::Input(input_event) => {
                handle_input(&mut state, input_event, &mut drag);
            }
            WinitEvent::CloseRequested => {
                tracing::info!("window close requested — stopping compositor");
                running = false;
            }
            WinitEvent::Focus(_) | WinitEvent::Redraw => {}
        });

        match status {
            PumpStatus::Continue => {}
            PumpStatus::Exit(_) => {
                tracing::info!("winit exited");
                break;
            }
        }

        if !running {
            break;
        }

        if let Some(watcher) = config_watcher.as_mut() {
            watcher.reload_if_due(&mut state);
        }

        if let Ok(Some(stream)) = listener.accept() {
            match display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                Ok(client) => {
                    clients.push(client);
                    tracing::debug!("new Wayland client connected");
                }
                Err(err) => tracing::warn!(%err, "failed to insert Wayland client"),
            }
        }

        let size = backend.window_size();
        let damage = Rectangle::from_size(size);

        {
            let (renderer, mut framebuffer) = match backend.bind() {
                Ok(bound) => bound,
                Err(err) => {
                    tracing::warn!(%err, "failed to bind renderer");
                    continue;
                }
            };

            let bottom_elements = bottom_layer_elements(renderer, &output);
            let window_content = window_content_elements(renderer, &state);
            let top_elements = top_layer_elements(renderer, &output);
            let popups = popup_elements(renderer, &state);
            let corner_shader = ensure_rounded_corner_shader(renderer, &mut rounded_corner_shader);

            // Winit's framebuffer has an inverted Y axis.
            match renderer.render(&mut framebuffer, size, Transform::Flipped180) {
                Ok(mut frame) => {
                    let _ = frame.clear(BACKGROUND_COLOR, &[damage]);
                    let _ = draw_render_elements(&mut frame, 1.0, &bottom_elements, &[damage]);
                    for (window, content) in &window_content {
                        if let Err(err) = draw_window(
                            &mut frame,
                            &state,
                            window,
                            content,
                            &[damage],
                            corner_shader.as_ref(),
                        ) {
                            tracing::warn!(%err, "failed to draw window");
                        }
                    }
                    let _ = draw_render_elements(&mut frame, 1.0, &top_elements, &[damage]);
                    let _ = draw_render_elements(&mut frame, 1.0, &popups, &[damage]);
                    let _ = frame.finish();
                }
                Err(err) => tracing::warn!(%err, "render error"),
            }

            send_frame_callbacks(&state, start_time.elapsed().as_millis() as u32);
        }

        backend.submit(Some(&[damage])).ok();

        display
            .dispatch_clients(&mut state)
            .context("dispatch error")?;
        display.flush_clients().context("flush error")?;
        if let Some(window) = state.take_pending_move_request() {
            if let Some(pointer) = state.seat.get_pointer() {
                let pos = pointer.current_location();
                begin_window_drag(&mut state, &mut drag, window, pos);
            }
        }

        integrations.drain(&mut state);
        if state.exit_requested {
            running = false;
        }

        state.space.refresh();
        state.popup_manager.cleanup();
    }

    tracing::info!("compositor exiting");
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

fn handle_input(
    state: &mut BlairState,
    event: InputEvent<WinitInput>,
    drag: &mut Option<WindowDrag>,
) {
    match event {
        InputEvent::Keyboard { event } => {
            if let Some(keyboard) = state.seat.get_keyboard() {
                let key_state = event.state();
                tracing::debug!(
                    keycode = u32::from(event.key_code()),
                    state = ?key_state,
                    "winit keyboard event"
                );
                keyboard.input::<(), _>(
                    state,
                    event.key_code(),
                    key_state,
                    SERIAL_COUNTER.next_serial(),
                    event.time_msec(),
                    move |state, _mods, _keysym| {
                        let keycode = u32::from(event.key_code());
                        let pressed = key_state == KeyState::Pressed;
                        crate::shortcuts::update_physical_mods(
                            &mut state.physical_mods,
                            keycode,
                            pressed,
                        );
                        state.shortcuts.update_key(keycode, pressed);
                        let activated =
                            state.shortcuts.maybe_activate_physical(state.physical_mods);
                        if pressed && !activated.is_empty() {
                            for shortcut in activated {
                                state.emit(blair_protocol::CompositorEvent::ShortcutActivated {
                                    client: shortcut.client,
                                    id: shortcut.id,
                                });
                            }
                            return FilterResult::Intercept(());
                        }
                        FilterResult::Forward
                    },
                );
            }
        }
        InputEvent::PointerMotionAbsolute { event } => {
            let output = state.space.outputs().next().cloned();
            if let Some(output) = output {
                let output_geo = state.space.output_geometry(&output).unwrap_or_default();
                let pos = event.position_transformed(output_geo.size);
                tracing::trace!(x = pos.x, y = pos.y, "winit pointer motion");
                if let Some(pointer) = state.seat.get_pointer() {
                    move_dragged_window(state, drag.as_ref(), pos);
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
            tracing::debug!(
                button = event.button_code(),
                state = ?event.state(),
                "winit pointer button"
            );
            if let Some(pointer) = state.seat.get_pointer() {
                let serial = SERIAL_COUNTER.next_serial();
                if event.button_code() == BTN_LEFT && event.state() == ButtonState::Released {
                    drag.take();
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
                            && handle_decoration_press(state, drag, window, pos)
                        {
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
