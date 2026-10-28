use std::collections::HashSet;

use smithay::{
    backend::renderer::{element::solid::SolidColorBuffer, Color32F},
    input::pointer::{CursorImageStatus, MotionEvent},
    output::Output,
    reexports::wayland_server::{
        backend::ObjectId,
        protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        Resource,
    },
    utils::{Logical, Size, SERIAL_COUNTER},
    wayland::{
        compositor::get_parent,
        session_lock::{LockSurface, SessionLockHandler, SessionLocker},
    },
};

use crate::{
    input::{cancel_touch_input, reset_keyboard_state},
    state::BlairState,
};

pub struct LockSurfaceEntry {
    pub surface: LockSurface,
    pub output: Output,
}

pub struct PendingSessionLock {
    confirmation: SessionLocker,
    outputs: HashSet<Output>,
}

#[derive(Debug)]
pub struct SessionLockFrame {
    lock: ObjectId,
    output: Output,
}

pub fn background_buffer() -> SolidColorBuffer {
    SolidColorBuffer::new((1, 1), Color32F::new(0.0, 0.0, 0.0, 1.0))
}

impl BlairState {
    fn cancel_client_input(&mut self) {
        self.dismiss_popup_grab();
        if let Some(pointer) = self.seat.get_pointer() {
            let serial = SERIAL_COUNTER.next_serial();
            let time = self.clock_now().as_millis() as u32;
            pointer.unset_grab(self, serial, time);
            pointer.motion(
                self,
                None,
                &MotionEvent {
                    location: pointer.current_location(),
                    serial,
                    time,
                },
            );
            pointer.frame(self);
        }
        cancel_touch_input(self);
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.unset_grab(self);
        }
        self.compositor_cursor = None;
        self.client_cursor = CursorImageStatus::default_named();
        self.hovered_button = None;
        self.mark_pointer_focus_dirty();
    }

    pub fn session_lock_focus(&self, requested: Option<WlSurface>) -> Option<WlSurface> {
        let find = |mut surface| {
            while let Some(parent) = get_parent(&surface) {
                surface = parent;
            }
            self.session_lock_surfaces
                .iter()
                .find(|entry| entry.surface.alive() && entry.surface.wl_surface() == &surface)
                .map(|entry| entry.surface.wl_surface().clone())
        };
        requested
            .and_then(find)
            .or_else(|| {
                self.seat
                    .get_keyboard()
                    .and_then(|keyboard| keyboard.current_focus())
                    .and_then(find)
            })
            .or_else(|| {
                self.session_lock_surfaces
                    .iter()
                    .find(|entry| entry.surface.alive())
                    .map(|entry| entry.surface.wl_surface().clone())
            })
    }

    pub fn pending_session_lock_frame(&self, output: &Output) -> Option<SessionLockFrame> {
        let pending = self.pending_session_lock.as_ref()?;
        pending.outputs.contains(output).then(|| SessionLockFrame {
            lock: pending.confirmation.ext_session_lock().id(),
            output: output.clone(),
        })
    }

    pub fn session_lock_frame_presented(&mut self, frame: SessionLockFrame) {
        let Some(pending) = self.pending_session_lock.as_mut() else {
            return;
        };
        if pending.confirmation.ext_session_lock().id() != frame.lock
            || !pending.outputs.remove(&frame.output)
        {
            return;
        }
        self.confirm_session_lock_if_ready();
    }

    fn confirm_session_lock_if_ready(&mut self) {
        if self
            .pending_session_lock
            .as_ref()
            .is_some_and(|pending| pending.outputs.is_empty())
        {
            self.pending_session_lock
                .take()
                .unwrap()
                .confirmation
                .lock();
            tracing::debug!("session lock confirmed on all outputs");
        }
    }

    pub fn prepare_for_sleep(&mut self) {
        if !self.config.session.lock_after_sleep || self.session_locked {
            return;
        }
        let Some(locker) = self.config.session.locker.clone() else {
            tracing::error!("refusing to claim a sleep lock without a configured locker");
            return;
        };
        match self.spawn_executable(&locker) {
            Ok(()) => {
                tracing::info!(locker, "started session locker before sleep");
            }
            Err(error) => {
                tracing::error!(%error, locker, "could not start session locker before sleep")
            }
        }
    }
}

impl SessionLockHandler for BlairState {
    fn lock_state(&mut self) -> &mut smithay::wayland::session_lock::SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if self.session_locked {
            tracing::warn!("rejected a second concurrent session lock");
            return;
        }
        reset_keyboard_state(self);
        self.session_locked = true;
        self.session_lock_client = confirmation
            .ext_session_lock()
            .client()
            .map(|client| client.id());
        self.session_lock_surfaces.clear();
        self.session_lock_buffer = background_buffer();
        self.cancel_client_input();
        self.set_keyboard_focus(None);
        self.pending_session_lock = Some(PendingSessionLock {
            confirmation,
            outputs: self.space.outputs().cloned().collect(),
        });
        self.confirm_session_lock_if_ready();
        self.request_redraw();
        tracing::info!("session lock engaged");
    }

    fn unlock(&mut self) {
        reset_keyboard_state(self);
        self.cancel_client_input();
        self.session_lock_surfaces.clear();
        self.pending_session_lock = None;
        self.session_locked = false;
        self.session_lock_client = None;
        self.set_keyboard_focus(None);
        self.request_redraw();
        tracing::info!("session lock released");
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        if !self.session_locked
            || surface.wl_surface().client().map(|client| client.id()) != self.session_lock_client
        {
            tracing::warn!("ignored a lock surface from an inactive session locker");
            return;
        }
        let Some(output) = Output::from_resource(&output) else {
            tracing::warn!("session lock requested an unknown output");
            return;
        };
        let Some(geometry) = self.space.output_geometry(&output) else {
            tracing::warn!(output = %output.name(), "session lock output has no geometry");
            return;
        };
        let size: Size<u32, Logical> =
            (geometry.size.w.max(1) as u32, geometry.size.h.max(1) as u32).into();
        surface.with_pending_state(|state| state.size = Some(size));
        surface.send_configure();
        self.session_lock_surfaces.push(LockSurfaceEntry {
            surface: surface.clone(),
            output,
        });
        self.set_keyboard_focus(Some(surface.wl_surface().clone()));
        self.request_redraw();
    }
}

#[cfg(test)]
mod tests;
