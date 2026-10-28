use smithay::{
    backend::input::TouchSlot,
    input::touch::{self, TouchDownGrab, TouchHandle},
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::shell::wlr_layer::Layer,
};
use wayland_server::Resource;

use super::{pointer_target, surface_under, PointerTarget};
use crate::{popup_grabs::InputKind, state::BlairState};

#[derive(PartialEq, Eq)]
enum Grab {
    None,
    Implicit,
    Explicit,
    Dead,
}

fn grab_kind(touch: &TouchHandle<BlairState>) -> Grab {
    touch
        .with_grab(|_, grab| {
            if grab
                .start_data()
                .focus
                .as_ref()
                .is_some_and(|(surface, _)| !surface.is_alive())
            {
                Grab::Dead
            } else if grab.is::<TouchDownGrab<BlairState>>() {
                Grab::Implicit
            } else {
                Grab::Explicit
            }
        })
        .unwrap_or(Grab::None)
}

pub(super) fn down(
    state: &mut BlairState,
    slot: TouchSlot,
    location: Point<f64, Logical>,
    time: u32,
) {
    let Some(touch) = state.seat.get_touch() else {
        return;
    };
    let mut grab = grab_kind(&touch);
    if grab == Grab::Dead {
        cancel_touch_input(state);
        grab = Grab::None;
    }
    if state.touch_points.contains_key(&slot) {
        return;
    }
    // Implicit grabs supply the first contact's target for every contact.
    let under = if grab == Grab::Implicit {
        None
    } else {
        let target = pointer_target(state, location);
        if grab == Grab::None {
            let focus = match &target {
                Some(
                    PointerTarget::Lock { surface, .. } | PointerTarget::Layer { surface, .. },
                ) => Some(surface),
                Some(PointerTarget::Window { surface, .. }) => {
                    surface.as_ref().map(|(surface, _)| surface)
                }
                None => None,
            };
            state.popup_touch_down(focus);
        }
        match target {
            Some(PointerTarget::Lock { surface, origin }) => {
                if grab == Grab::None {
                    state.set_keyboard_focus(Some(surface.clone()));
                }
                Some((surface, origin))
            }
            Some(PointerTarget::Window {
                window, surface, ..
            }) => {
                if grab == Grab::None {
                    state.focus_window(&window);
                }
                surface
            }
            Some(PointerTarget::Layer {
                surface,
                origin,
                layer,
                can_focus,
            }) => {
                if grab == Grab::None {
                    if can_focus {
                        state.set_keyboard_focus(Some(surface.clone()));
                    } else if matches!(layer, Layer::Bottom | Layer::Background) {
                        state.set_keyboard_focus(None);
                    }
                }
                Some((surface, origin))
            }
            None => None,
        }
    };
    state.touch_points.insert(slot, location);
    let serial = SERIAL_COUNTER.next_serial();
    touch.down(
        state,
        under,
        &touch::DownEvent {
            slot,
            location,
            serial,
            time,
        },
    );
    if !state.session_locked && matches!(grab, Grab::None | Grab::Implicit) {
        let focus = touch
            .grab_start_data()
            .and_then(|start| start.focus)
            .map(|(surface, _)| surface);
        state.record_popup_input(InputKind::Touch, serial, focus.as_ref());
    }
}

pub(super) fn motion(
    state: &mut BlairState,
    slot: TouchSlot,
    location: Point<f64, Logical>,
    time: u32,
) {
    let Some(point) = state.touch_points.get_mut(&slot) else {
        return;
    };
    *point = location;
    let Some(touch) = state.seat.get_touch() else {
        return;
    };
    let under = if grab_kind(&touch) == Grab::Explicit {
        surface_under(state, location)
    } else {
        None
    };
    touch.motion(
        state,
        under,
        &touch::MotionEvent {
            slot,
            location,
            time,
        },
    );
}

pub(super) fn up(state: &mut BlairState, slot: TouchSlot, time: u32) {
    if state.touch_points.remove(&slot).is_none() {
        return;
    }
    if let Some(touch) = state.seat.get_touch() {
        touch.up(
            state,
            &touch::UpEvent {
                slot,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
    }
}

pub(crate) fn cancel_touch_input(state: &mut BlairState) {
    state.popup_input.revoke(InputKind::Touch);
    let points = std::mem::take(&mut state.touch_points);
    if let Some(touch) = state.seat.get_touch() {
        touch.unset_grab(state);
        // Smithay cancels only slots with events pending since the last touch frame.
        for (slot, location) in points {
            touch.motion(
                state,
                None,
                &touch::MotionEvent {
                    slot,
                    location,
                    time: state.clock_now().as_millis() as u32,
                },
            );
        }
        touch.cancel(state);
    }
}
