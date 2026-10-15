use smithay::{
    output::Output,
    reexports::wayland_server::protocol::wl_output::WlOutput,
    utils::{Logical, Size},
    wayland::session_lock::{LockSurface, SessionLockHandler, SessionLocker},
};

use crate::{input::reset_keyboard_state, state::BlairState};

pub struct LockSurfaceEntry {
    pub surface: LockSurface,
    pub output: Output,
}

impl BlairState {
    pub fn confirm_session_lock(&mut self) {
        if let Some(confirmation) = self.pending_session_lock.take() {
            confirmation.lock();
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
        self.session_lock_surfaces.clear();
        self.set_keyboard_focus(None);
        self.pending_session_lock = Some(confirmation);
        self.request_redraw();
        tracing::info!("session lock engaged");
    }

    fn unlock(&mut self) {
        self.session_lock_surfaces.clear();
        self.pending_session_lock = None;
        self.session_locked = false;
        self.set_keyboard_focus(None);
        self.request_redraw();
        tracing::info!("session lock released");
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
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
