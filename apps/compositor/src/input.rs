use blair_protocol::Point as CorePoint;
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, ButtonState, Event, GestureBeginEvent, GestureEndEvent,
        GesturePinchUpdateEvent as _, GestureSwipeUpdateEvent as _, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        TouchEvent,
    },
    desktop::{layer_map_for_output, Window, WindowSurfaceType},
    input::{
        keyboard::{keysyms, FilterResult},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            RelativeMotionEvent,
        },
        touch,
    },
    output::Output,
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::get_parent,
        pointer_constraints::{with_pointer_constraint, PointerConstraint},
        seat::WaylandFocus,
        shell::wlr_layer::Layer as WlrLayer,
    },
};
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    config::WindowLayout,
    decorations::{hit_test_frame, DecorationPart},
    grabs::ResizeEdges,
    render::{to_rect, window_frame, z_ordered_windows},
    shortcuts::{physical_vt_from_keycode, update_physical_mods},
    state::BlairState,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// Pixel-equivalent of one wheel click (120 v120 units), for backends that
/// only report discrete steps. Matches the GTK/wlroots convention.
const PIXELS_PER_WHEEL_CLICK: f64 = 15.0;

/// Backend operations that input handling may need to trigger.
pub trait InputHooks {
    fn change_vt(&mut self, _vt: i32) {}
}

pub enum PointerTarget {
    Layer {
        surface: WlSurface,
        origin: Point<f64, Logical>,
        layer: WlrLayer,
        can_focus: bool,
    },
    Window {
        window: Window,
        surface: Option<(WlSurface, Point<f64, Logical>)>,
        part: DecorationPart,
    },
}

pub fn pointer_target(state: &BlairState, pos: Point<f64, Logical>) -> Option<PointerTarget> {
    let output = state.output_at(pos)?;
    let windows = z_ordered_windows(state, &output);
    let fullscreen = windows
        .last()
        .is_some_and(|window| state.window_is_fullscreen(window));
    let upper: &[WlrLayer] = if fullscreen {
        &[WlrLayer::Overlay]
    } else {
        &[WlrLayer::Overlay, WlrLayer::Top]
    };
    if let Some(target) = layer_target(state, &output, pos, upper) {
        return Some(target);
    }
    if let Some(target) = windows
        .iter()
        .rev()
        .find_map(|window| window_target(state, window, pos))
    {
        return Some(target);
    }
    if fullscreen {
        return None;
    }
    layer_target(
        state,
        &output,
        pos,
        &[WlrLayer::Bottom, WlrLayer::Background],
    )
}

fn layer_target(
    state: &BlairState,
    output: &Output,
    pos: Point<f64, Logical>,
    layers: &[WlrLayer],
) -> Option<PointerTarget> {
    let output_loc = state.space.output_geometry(output)?.loc;
    let map = layer_map_for_output(output);
    for &layer in layers {
        for layer_surface in map.layers_on(layer).rev() {
            let Some(geometry) = map.layer_geometry(layer_surface) else {
                continue;
            };
            let origin = output_loc + geometry.loc;
            let Some((surface, offset)) =
                layer_surface.surface_under(pos - origin.to_f64(), WindowSurfaceType::ALL)
            else {
                continue;
            };
            return Some(PointerTarget::Layer {
                surface,
                origin: (origin + offset).to_f64(),
                layer,
                can_focus: layer_surface.can_receive_keyboard_focus(),
            });
        }
    }
    None
}

fn window_target(
    state: &BlairState,
    window: &Window,
    pos: Point<f64, Logical>,
) -> Option<PointerTarget> {
    let frame = window_frame(state, window)?;
    let render_loc = frame.client.loc - window.geometry().loc;
    if let Some((surface, offset)) =
        window.surface_under(pos - render_loc.to_f64(), WindowSurfaceType::ALL)
    {
        return Some(PointerTarget::Window {
            window: window.clone(),
            surface: Some((surface, (render_loc + offset).to_f64())),
            part: DecorationPart::Client,
        });
    }
    if !frame.has_border || !frame.frame.to_f64().contains(pos) {
        return None;
    }
    let part = hit_test_frame(
        to_rect(frame.client),
        CorePoint { x: pos.x, y: pos.y },
        state.decoration_theme(),
        frame.has_titlebar,
    );
    Some(PointerTarget::Window {
        window: window.clone(),
        surface: None,
        part,
    })
}

pub fn surface_under(
    state: &BlairState,
    pos: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    match pointer_target(state, pos)? {
        PointerTarget::Layer {
            surface, origin, ..
        } => Some((surface, origin)),
        PointerTarget::Window { surface, .. } => surface,
    }
}

/// Global origin of a surface's root, used for surface-local hints.
pub fn surface_origin(state: &BlairState, surface: &WlSurface) -> Option<Point<f64, Logical>> {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    if let Some(window) = state
        .space
        .elements()
        .find(|window| window.wl_surface().as_deref() == Some(&root))
    {
        let geometry = state.space.element_geometry(window)?;
        return Some((geometry.loc - window.geometry().loc).to_f64());
    }
    let output = state.output_for_layer(&root)?;
    let output_loc = state.space.output_geometry(&output)?.loc;
    let map = layer_map_for_output(&output);
    let layer = map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)?;
    Some((output_loc + map.layer_geometry(layer)?.loc).to_f64())
}

pub fn process_input_event<B: InputBackend>(
    state: &mut BlairState,
    event: InputEvent<B>,
    hooks: &mut dyn InputHooks,
) {
    let seat = state.seat.clone();
    state.idle_notifier_state.notify_activity(&seat);
    match event {
        InputEvent::Keyboard { event } => keyboard_key::<B>(state, event, hooks),
        InputEvent::PointerMotion { event } => {
            pointer_motion_relative(
                state,
                event.delta(),
                event.delta_unaccel(),
                event.time(),
                event.time_msec(),
            );
        }
        InputEvent::PointerMotionAbsolute { event } => {
            let Some(output) = state.focused_output() else {
                return;
            };
            let Some(geometry) = state.space.output_geometry(&output) else {
                return;
            };
            let location = event.position_transformed(geometry.size) + geometry.loc.to_f64();
            pointer_motion_absolute(state, location, event.time_msec());
        }
        InputEvent::PointerButton { event } => {
            pointer_button(state, event.button_code(), event.state(), event.time_msec());
        }
        InputEvent::PointerAxis { event } => pointer_axis::<B, _>(state, &event),
        InputEvent::GestureSwipeBegin { event } => with_pointer(state, |state, pointer| {
            pointer.gesture_swipe_begin(
                state,
                &GestureSwipeBeginEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time_msec(),
                    fingers: event.fingers(),
                },
            );
        }),
        InputEvent::GestureSwipeUpdate { event } => with_pointer(state, |state, pointer| {
            pointer.gesture_swipe_update(
                state,
                &GestureSwipeUpdateEvent {
                    time: event.time_msec(),
                    delta: event.delta(),
                },
            );
        }),
        InputEvent::GestureSwipeEnd { event } => with_pointer(state, |state, pointer| {
            pointer.gesture_swipe_end(
                state,
                &GestureSwipeEndEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time_msec(),
                    cancelled: event.cancelled(),
                },
            );
        }),
        InputEvent::GesturePinchBegin { event } => with_pointer(state, |state, pointer| {
            pointer.gesture_pinch_begin(
                state,
                &GesturePinchBeginEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time_msec(),
                    fingers: event.fingers(),
                },
            );
        }),
        InputEvent::GesturePinchUpdate { event } => with_pointer(state, |state, pointer| {
            pointer.gesture_pinch_update(
                state,
                &GesturePinchUpdateEvent {
                    time: event.time_msec(),
                    delta: event.delta(),
                    scale: event.scale(),
                    rotation: event.rotation(),
                },
            );
        }),
        InputEvent::GesturePinchEnd { event } => with_pointer(state, |state, pointer| {
            pointer.gesture_pinch_end(
                state,
                &GesturePinchEndEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time_msec(),
                    cancelled: event.cancelled(),
                },
            );
        }),
        InputEvent::GestureHoldBegin { event } => with_pointer(state, |state, pointer| {
            pointer.gesture_hold_begin(
                state,
                &GestureHoldBeginEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time_msec(),
                    fingers: event.fingers(),
                },
            );
        }),
        InputEvent::GestureHoldEnd { event } => with_pointer(state, |state, pointer| {
            pointer.gesture_hold_end(
                state,
                &GestureHoldEndEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time_msec(),
                    cancelled: event.cancelled(),
                },
            );
        }),
        InputEvent::TouchDown { event } => {
            let Some(location) = touch_location::<B, _>(state, &event) else {
                return;
            };
            let under = surface_under(state, location);
            if let Some(PointerTarget::Window { window, .. }) = pointer_target(state, location) {
                state.focus_window(&window);
            }
            if let Some(touch) = state.seat.get_touch() {
                touch.down(
                    state,
                    under,
                    &touch::DownEvent {
                        slot: event.slot(),
                        location,
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                    },
                );
            }
        }
        InputEvent::TouchMotion { event } => {
            let Some(location) = touch_location::<B, _>(state, &event) else {
                return;
            };
            let under = surface_under(state, location);
            if let Some(touch) = state.seat.get_touch() {
                touch.motion(
                    state,
                    under,
                    &touch::MotionEvent {
                        slot: event.slot(),
                        location,
                        time: event.time_msec(),
                    },
                );
            }
        }
        InputEvent::TouchUp { event } => {
            if let Some(touch) = state.seat.get_touch() {
                touch.up(
                    state,
                    &touch::UpEvent {
                        slot: event.slot(),
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                    },
                );
            }
        }
        InputEvent::TouchCancel { .. } => {
            if let Some(touch) = state.seat.get_touch() {
                touch.cancel(state);
            }
        }
        InputEvent::TouchFrame { .. } => {
            if let Some(touch) = state.seat.get_touch() {
                touch.frame(state);
            }
        }
        _ => {}
    }
}

fn with_pointer(
    state: &mut BlairState,
    f: impl FnOnce(&mut BlairState, &smithay::input::pointer::PointerHandle<BlairState>),
) {
    if let Some(pointer) = state.seat.get_pointer() {
        f(state, &pointer);
    }
}

fn touch_location<B: InputBackend, E: AbsolutePositionEvent<B>>(
    state: &BlairState,
    event: &E,
) -> Option<Point<f64, Logical>> {
    let output = state.focused_output()?;
    let geometry = state.space.output_geometry(&output)?;
    Some(event.position_transformed(geometry.size) + geometry.loc.to_f64())
}

enum KeyAction {
    Shortcuts(Vec<crate::shortcuts::ActivatedShortcut>),
    ChangeVt(i32),
    Exit,
    Suppress,
}

fn keyboard_key<B: InputBackend>(
    state: &mut BlairState,
    event: B::KeyboardKeyEvent,
    hooks: &mut dyn InputHooks,
) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    let keycode = event.key_code();
    let raw = u32::from(keycode);
    let key_state = event.state();
    let pressed = key_state == KeyState::Pressed;
    update_physical_mods(&mut state.physical_mods, raw, pressed);
    state.shortcuts.update_key(raw, pressed);
    let activated = state.shortcuts.maybe_activate_physical(state.physical_mods);

    let action = keyboard.input(
        state,
        keycode,
        key_state,
        SERIAL_COUNTER.next_serial(),
        event.time_msec(),
        |state, _mods, keysym| {
            if !pressed {
                return if state.suppressed_keys.remove(&raw) {
                    FilterResult::Intercept(KeyAction::Suppress)
                } else {
                    FilterResult::Forward
                };
            }
            let mods = state.physical_mods;
            let sym = u32::from(keysym.modified_sym());
            let vt = (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12)
                .contains(&sym)
                .then(|| (sym - keysyms::KEY_XF86Switch_VT_1 + 1) as i32)
                .or_else(|| {
                    (mods.ctrl && mods.alt)
                        .then(|| physical_vt_from_keycode(raw))
                        .flatten()
                });
            let action = if let Some(vt) = vt {
                Some(KeyAction::ChangeVt(vt))
            } else if cfg!(debug_assertions) && mods.ctrl && mods.logo && sym == keysyms::KEY_Escape
            {
                Some(KeyAction::Exit)
            } else if !activated.is_empty() && !shortcuts_inhibited(state) {
                Some(KeyAction::Shortcuts(activated))
            } else {
                None
            };
            match action {
                Some(action) => {
                    state.suppressed_keys.insert(raw);
                    FilterResult::Intercept(action)
                }
                None => FilterResult::Forward,
            }
        },
    );

    match action {
        Some(KeyAction::Shortcuts(shortcuts)) => {
            tracing::debug!(count = shortcuts.len(), "shortcut activated");
            state.activate_shortcuts(shortcuts);
        }
        Some(KeyAction::ChangeVt(vt)) => {
            tracing::info!(vt, "VT switch requested");
            hooks.change_vt(vt);
        }
        Some(KeyAction::Exit) => {
            tracing::warn!("debug emergency exit requested by Ctrl+Super+Esc");
            state.request_exit();
        }
        Some(KeyAction::Suppress) | None => {}
    }
}

/// Clients such as remote desktop viewers may take over the compositor's
/// shortcuts while they are focused.
fn shortcuts_inhibited(state: &BlairState) -> bool {
    use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat;
    state.seat.keyboard_shortcuts_inhibited()
}

pub fn reset_keyboard_state(state: &mut BlairState) {
    state.physical_mods = Default::default();
    state.shortcuts.clear_pressed();
    state.suppressed_keys.clear();
}

fn clamp_to_outputs(state: &BlairState, pos: Point<f64, Logical>) -> Point<f64, Logical> {
    if state.output_at(pos).is_some() {
        return pos;
    }
    let current = state.pointer_location();
    let Some(geometry) = state
        .output_at(current)
        .or_else(|| state.focused_output())
        .and_then(|output| state.space.output_geometry(&output))
    else {
        return pos;
    };
    let max_x = f64::from(geometry.loc.x + geometry.size.w) - 1.0;
    let max_y = f64::from(geometry.loc.y + geometry.size.h) - 1.0;
    (
        pos.x.clamp(f64::from(geometry.loc.x), max_x),
        pos.y.clamp(f64::from(geometry.loc.y), max_y),
    )
        .into()
}

fn pointer_motion_relative(
    state: &mut BlairState,
    delta: Point<f64, Logical>,
    delta_unaccel: Point<f64, Logical>,
    time_usec: u64,
    time_msec: u32,
) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let current = pointer.current_location();
    let under = surface_under(state, current);

    let mut locked = false;
    let mut confine_region = None;
    let mut confined = false;
    if let Some((surface, origin)) = under.as_ref() {
        with_pointer_constraint(surface, &pointer, |constraint| {
            let Some(constraint) = constraint.filter(|constraint| constraint.is_active()) else {
                return;
            };
            let local = (current - *origin).to_i32_round();
            if !constraint
                .region()
                .is_none_or(|region| region.contains(local))
            {
                return;
            }
            match &*constraint {
                PointerConstraint::Locked(_) => locked = true,
                PointerConstraint::Confined(confine) => {
                    confined = true;
                    confine_region = confine.region().cloned();
                }
            }
        });
    }

    pointer.relative_motion(
        state,
        under.clone(),
        &RelativeMotionEvent {
            delta,
            delta_unaccel,
            utime: time_usec,
        },
    );
    if locked {
        pointer.frame(state);
        return;
    }

    let location = clamp_to_outputs(state, current + delta);
    let new_under = surface_under(state, location);
    if confined {
        if let Some((surface, origin)) = under.as_ref() {
            let left_surface = new_under.as_ref().map(|(target, _)| target) != Some(surface);
            let outside_region = confine_region
                .as_ref()
                .is_some_and(|region| !region.contains((location - *origin).to_i32_round()));
            if left_surface || outside_region {
                pointer.frame(state);
                return;
            }
        }
    }

    move_pointer(state, location, new_under.clone(), time_msec);

    if let Some((surface, origin)) = new_under {
        with_pointer_constraint(&surface, &pointer, |constraint| {
            if let Some(constraint) = constraint.filter(|constraint| !constraint.is_active()) {
                let local = (location - origin).to_i32_round();
                if constraint
                    .region()
                    .is_none_or(|region| region.contains(local))
                {
                    constraint.activate();
                }
            }
        });
    }
}

fn pointer_motion_absolute(state: &mut BlairState, location: Point<f64, Logical>, time: u32) {
    let location = clamp_to_outputs(state, location);
    let under = surface_under(state, location);
    move_pointer(state, location, under, time);
}

fn move_pointer(
    state: &mut BlairState,
    location: Point<f64, Logical>,
    under: Option<(WlSurface, Point<f64, Logical>)>,
    time: u32,
) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    state.set_focused_output_at(location);
    pointer.motion(
        state,
        under,
        &MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    pointer.frame(state);
    state.request_redraw();
}

fn pointer_button(state: &mut BlairState, button: u32, button_state: ButtonState, time: u32) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
        let location = pointer.current_location();
        let consumed = press_target(state, location, button, serial);
        if consumed {
            pointer.frame(state);
            state.request_redraw();
            return;
        }
    }
    pointer.button(
        state,
        &ButtonEvent {
            button,
            state: button_state,
            serial,
            time,
        },
    );
    pointer.frame(state);
}

/// Applies click-to-focus and compositor-side window actions. Returns true
/// when the press must not reach the client.
fn press_target(
    state: &mut BlairState,
    location: Point<f64, Logical>,
    button: u32,
    serial: smithay::utils::Serial,
) -> bool {
    match pointer_target(state, location) {
        Some(PointerTarget::Layer {
            surface,
            layer,
            can_focus,
            ..
        }) => {
            tracing::trace!(?layer, can_focus, "click hit a layer surface");
            if can_focus {
                if let Some(keyboard) = state.seat.get_keyboard() {
                    keyboard.set_focus(state, Some(surface), serial);
                }
            } else if matches!(layer, WlrLayer::Bottom | WlrLayer::Background) {
                state.set_keyboard_focus(None);
            }
            false
        }
        Some(PointerTarget::Window {
            window,
            surface,
            part,
        }) => {
            state.focus_window(&window);
            let floating = state.config.window.layout != WindowLayout::Tiling;
            if state.physical_mods.logo && surface.is_some() {
                return match button {
                    BTN_LEFT => state.begin_move(&window, serial, button),
                    BTN_RIGHT => {
                        let edges = quadrant_edges(state, &window, location);
                        state.begin_resize(&window, edges, serial, button)
                    }
                    _ => false,
                };
            }
            if surface.is_some() || button != BTN_LEFT || !floating {
                return false;
            }
            match part {
                DecorationPart::CloseButton => state.close_window(&window),
                DecorationPart::MinimizeButton => state.minimize_window(&window),
                DecorationPart::MaximizeButton => state.toggle_maximize_window(&window),
                DecorationPart::Titlebar => state.begin_move(&window, serial, button),
                part => match resize_edges(part) {
                    Some(edges) => state.begin_resize(&window, edges, serial, button),
                    None => true,
                },
            }
        }
        None => {
            state.set_keyboard_focus(None);
            false
        }
    }
}

fn quadrant_edges(
    state: &BlairState,
    window: &Window,
    location: Point<f64, Logical>,
) -> ResizeEdges {
    let geometry = state.space.element_geometry(window).unwrap_or_default();
    let center = geometry.loc.to_f64() + geometry.size.to_f64().downscale(2.0).to_point();
    ResizeEdges {
        top: location.y < center.y,
        bottom: location.y >= center.y,
        left: location.x < center.x,
        right: location.x >= center.x,
    }
}

fn resize_edges(part: DecorationPart) -> Option<ResizeEdges> {
    let (top, bottom, left, right) = match part {
        DecorationPart::ResizeTop => (true, false, false, false),
        DecorationPart::ResizeBottom => (false, true, false, false),
        DecorationPart::ResizeLeft => (false, false, true, false),
        DecorationPart::ResizeRight => (false, false, false, true),
        DecorationPart::ResizeTopLeft => (true, false, true, false),
        DecorationPart::ResizeTopRight => (true, false, false, true),
        DecorationPart::ResizeBottomLeft => (false, true, true, false),
        DecorationPart::ResizeBottomRight => (false, true, false, true),
        _ => return None,
    };
    Some(ResizeEdges {
        top,
        bottom,
        left,
        right,
    })
}

fn pointer_axis<B, E>(state: &mut BlairState, event: &E)
where
    B: InputBackend,
    E: PointerAxisEvent<B> + Event<B>,
{
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
    let mut has_value = false;

    for axis in [Axis::Horizontal, Axis::Vertical] {
        let discrete = event.amount_v120(axis);
        if let Some(v120) = discrete {
            frame = frame.v120(axis, v120.round() as i32);
        }
        let amount = event
            .amount(axis)
            .or_else(|| discrete.map(|v120| v120 / 120.0 * PIXELS_PER_WHEEL_CLICK));
        match amount {
            Some(amount) if amount != 0.0 => {
                frame = frame
                    .relative_direction(axis, event.relative_direction(axis))
                    .value(axis, amount);
                has_value = true;
            }
            Some(_) => {
                frame = frame.stop(axis);
                has_value = true;
            }
            None => {}
        }
    }

    if !has_value {
        return;
    }
    pointer.axis(state, frame);
    pointer.frame(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoration_parts_map_to_resize_edges() {
        let edges = resize_edges(DecorationPart::ResizeTopRight).unwrap();
        assert!(edges.top && edges.right && !edges.bottom && !edges.left);
        assert!(resize_edges(DecorationPart::Titlebar).is_none());
        assert!(resize_edges(DecorationPart::Client).is_none());
    }
}
