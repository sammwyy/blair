use std::{collections::VecDeque, time::Duration};

use smithay::{
    desktop::{
        find_popup_root_surface, PopupGrab, PopupKeyboardGrab, PopupKind, PopupManager,
        PopupPointerGrab,
    },
    input::{
        pointer::{ClickGrab, Focus},
        Seat,
    },
    utils::{Serial, SERIAL_COUNTER},
    wayland::shell::xdg::PopupSurface,
};
use wayland_server::{
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    Resource, Weak,
};

use crate::state::BlairState;

const MAX_INPUTS: usize = 32;
const MAX_INPUT_AGE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Keyboard,
    Pointer,
    Touch,
}

struct InputPress {
    kind: InputKind,
    serial: Serial,
    focus: Weak<WlSurface>,
    time: Duration,
}

#[derive(Default)]
pub struct PopupInput {
    presses: VecDeque<InputPress>,
    keyboard_focus: Option<Weak<WlSurface>>,
    origin: Option<Serial>,
}

impl PopupInput {
    fn record(&mut self, kind: InputKind, serial: Serial, focus: &WlSurface, time: Duration) {
        self.presses.retain(|press| {
            time.saturating_sub(press.time) <= MAX_INPUT_AGE
                && press.focus.id().same_client_as(&focus.id())
                && press.focus.is_alive()
        });
        if self.presses.len() >= MAX_INPUTS {
            self.presses.pop_front();
        }
        self.presses.push_back(InputPress {
            kind,
            serial,
            focus: focus.downgrade(),
            time,
        });
    }

    fn input(&self, serial: Serial, surface: &WlSurface, now: Duration) -> Option<InputKind> {
        self.presses
            .iter()
            .find(|press| {
                press.serial == serial
                    && now.saturating_sub(press.time) <= MAX_INPUT_AGE
                    && press.focus.id().same_client_as(&surface.id())
                    && press.focus.is_alive()
            })
            .map(|press| press.kind)
    }

    pub fn focus_changed(&mut self, focus: Option<&WlSurface>) {
        let same_client = self
            .keyboard_focus
            .as_ref()
            .zip(focus)
            .is_some_and(|(previous, current)| previous.id().same_client_as(&current.id()));
        if !same_client {
            self.clear();
        }
        self.keyboard_focus = focus.map(Resource::downgrade);
    }

    pub fn clear(&mut self) {
        self.presses.clear();
        self.origin = None;
        self.keyboard_focus = None;
    }

    pub fn revoke(&mut self, kind: InputKind) {
        self.presses.retain(|press| press.kind != kind);
    }
}

struct Authorization {
    root: WlSurface,
    nested: bool,
}

fn active_grab(grab: &PopupGrab<BlairState>) -> bool {
    !grab.has_ended()
        && grab.current_grab().as_ref() != grab.keyboard_grab_start_data().focus.as_ref()
}

impl BlairState {
    pub(crate) fn refresh_popup_grab(&mut self) {
        let Some(grab) = self.active_popup_grab.as_ref() else {
            return;
        };
        let matching = |serial| serial == grab.serial() || Some(serial) == grab.previous_serial();
        if active_grab(grab) {
            if let Some(keyboard) = self.seat.get_keyboard() {
                let current = grab.current_grab();
                let ours = keyboard
                    .with_grab(|serial, handler| {
                        matching(serial) && handler.is::<PopupKeyboardGrab<Self>>()
                    })
                    .unwrap_or(false);
                if ours && keyboard.current_focus() != current {
                    keyboard.set_focus(self, current, SERIAL_COUNTER.next_serial());
                }
            }
            return;
        }
        let grab = self.active_popup_grab.take().unwrap();
        self.popup_input.origin = None;
        let matching = |serial| serial == grab.serial() || Some(serial) == grab.previous_serial();
        if let Some(keyboard) = self.seat.get_keyboard() {
            let ours = keyboard
                .with_grab(|serial, handler| {
                    matching(serial) && handler.is::<PopupKeyboardGrab<Self>>()
                })
                .unwrap_or(false);
            if ours {
                keyboard.unset_grab(self);
                self.set_keyboard_focus(
                    grab.keyboard_grab_start_data()
                        .focus
                        .clone()
                        .filter(Resource::is_alive),
                );
            }
        }
        if let Some(pointer) = self.seat.get_pointer() {
            let ours = pointer
                .with_grab(|serial, handler| {
                    matching(serial) && handler.is::<PopupPointerGrab<Self>>()
                })
                .unwrap_or(false);
            if ours {
                pointer.unset_grab(
                    self,
                    SERIAL_COUNTER.next_serial(),
                    self.clock_now().as_millis() as u32,
                );
            }
        }
    }

    pub(crate) fn record_popup_input(
        &mut self,
        kind: InputKind,
        serial: Serial,
        focus: Option<&WlSurface>,
    ) {
        if self.session_locked {
            return;
        }
        if let Some(focus) = focus {
            let now = self.clock_now();
            self.popup_input.record(kind, serial, focus, now);
        }
    }

    fn authorize_popup_grab(
        &self,
        surface: &PopupSurface,
        seat: &Seat<Self>,
        serial: Serial,
    ) -> Result<Authorization, &'static str> {
        if self.session_locked {
            return Err("session is locked");
        }
        if seat != &self.seat {
            return Err("input belongs to another seat");
        }
        let owner = surface.wl_surface();
        if self.popup_manager.find_popup(owner).is_none() {
            return Err("popup is no longer tracked");
        }
        if self
            .exclusive_layer_surface()
            .is_some_and(|layer| !layer.id().same_client_as(&owner.id()))
        {
            return Err("an exclusive layer owns keyboard focus");
        }
        let root = find_popup_root_surface(&PopupKind::Xdg(surface.clone()))
            .map_err(|_| "popup has no live root")?;
        let active = self
            .active_popup_grab
            .as_ref()
            .filter(|grab| active_grab(grab));
        let parent = active
            .and_then(|grab| grab.current_grab())
            .unwrap_or_else(|| root.clone());
        if surface.get_parent_surface().as_ref() != Some(&parent) {
            return Err("popup parent is not the active grab");
        }
        let keyboard = seat.get_keyboard();
        let pointer = seat.get_pointer();
        let keyboard_focused = keyboard
            .as_ref()
            .is_some_and(|keyboard| keyboard.client_of_object_has_focus(&owner.id()));
        let pointer_focused = pointer
            .as_ref()
            .and_then(|pointer| pointer.current_focus())
            .is_some_and(|focus| focus.is_alive() && focus.id().same_client_as(&owner.id()));
        let click = pointer
            .as_ref()
            .and_then(|pointer| {
                pointer.with_grab(|grab_serial, grab| {
                    grab.is::<ClickGrab<Self>>()
                        && grab.start_data().focus.as_ref().is_some_and(|(focus, _)| {
                            focus.is_alive() && focus.id().same_client_as(&owner.id())
                        })
                        && (grab_serial == serial || pointer_focused)
                })
            })
            .unwrap_or(false);
        let chain_serial = active.is_some_and(|grab| {
            serial == grab.serial()
                || Some(serial) == grab.previous_serial()
                || Some(serial) == self.popup_input.origin
        });
        let input = match self.popup_input.input(serial, owner, self.clock_now()) {
            Some(InputKind::Keyboard) => keyboard_focused,
            Some(InputKind::Pointer) => keyboard_focused || pointer_focused,
            Some(InputKind::Touch) => true,
            None => false,
        };
        let live_click = click
            && pointer
                .as_ref()
                .is_some_and(|pointer| pointer.has_grab(serial));
        if !(input || live_click || chain_serial) {
            return Err("serial does not authorize input for this client");
        }
        let previous = active.map(|grab| grab.serial());
        if keyboard.as_ref().is_some_and(|keyboard| {
            keyboard.is_grabbed()
                && !keyboard
                    .with_grab(|grab_serial, grab| {
                        grab.is::<PopupKeyboardGrab<Self>>() && Some(grab_serial) == previous
                    })
                    .unwrap_or(false)
        }) {
            return Err("keyboard has an unrelated grab");
        }
        if pointer.as_ref().is_some_and(|pointer| {
            pointer.is_grabbed()
                && !click
                && !pointer
                    .with_grab(|grab_serial, grab| {
                        grab.is::<PopupPointerGrab<Self>>() && Some(grab_serial) == previous
                    })
                    .unwrap_or(false)
        }) {
            return Err("pointer has an unrelated grab");
        }
        Ok(Authorization {
            root,
            nested: active.is_some(),
        })
    }

    fn reject_popup_grab(&mut self, surface: &PopupSurface, serial: Serial, reason: &str) {
        tracing::debug!(reason, ?serial, surface = ?surface.wl_surface().id(), "popup grab rejected");
        let tracked = self
            .popup_manager
            .find_popup(surface.wl_surface())
            .is_some();
        let dismissed = tracked
            && find_popup_root_surface(&PopupKind::Xdg(surface.clone()))
                .ok()
                .is_some_and(|root| {
                    PopupManager::dismiss_popup(&root, &PopupKind::Xdg(surface.clone())).is_ok()
                });
        if dismissed {
            self.request_redraw();
            self.mark_pointer_focus_dirty();
        } else {
            surface.send_popup_done();
        }
    }

    pub(crate) fn grab_popup(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        self.refresh_popup_grab();
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            self.reject_popup_grab(&surface, serial, "seat is unavailable");
            return;
        };
        let authorization = match self.authorize_popup_grab(&surface, &seat, serial) {
            Ok(authorization) => authorization,
            Err(reason) => {
                self.reject_popup_grab(&surface, serial, reason);
                return;
            }
        };
        let grab = match self.popup_manager.grab_popup(
            authorization.root,
            PopupKind::Xdg(surface),
            &seat,
            serial,
        ) {
            Ok(grab) => grab,
            Err(error) => {
                tracing::debug!(?error, "popup grab rejected");
                return;
            }
        };
        // Replacing a popup pointer grab can unset its paired keyboard grab.
        if let Some(pointer) = seat.get_pointer() {
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
        if let Some(keyboard) = seat.get_keyboard() {
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        self.popup_input
            .presses
            .retain(|press| press.serial != serial);
        if !authorization.nested {
            self.popup_input.origin = Some(serial);
        }
        self.active_popup_grab = Some(grab);
    }
}

#[cfg(test)]
mod tests;
