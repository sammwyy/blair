use std::collections::HashSet;

use smithay::{
    backend::renderer::{element::solid::SolidColorBuffer, Color32F},
    desktop::PopupUngrabStrategy,
    input::pointer::MotionEvent,
    output::Output,
    reexports::wayland_server::{backend::ObjectId, protocol::wl_output::WlOutput, Resource},
    utils::{Logical, Size, SERIAL_COUNTER},
    wayland::session_lock::{LockSurface, SessionLockHandler, SessionLocker},
};

use crate::{input::reset_keyboard_state, state::BlairState};

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
        if let Some(mut grab) = self.active_popup_grab.take() {
            grab.ungrab(PopupUngrabStrategy::All);
        }
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
        if let Some(touch) = self.seat.get_touch() {
            touch.unset_grab(self);
            touch.cancel(self);
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.unset_grab(self);
        }
        self.compositor_cursor = None;
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
mod tests {
    use super::*;
    use smithay::{
        backend::input::ButtonState,
        input::pointer::{ButtonEvent, MotionEvent},
        output::{Mode, PhysicalProperties, Subpixel},
        utils::SERIAL_COUNTER,
    };
    use wayland_client::protocol::{wl_compositor, wl_keyboard, wl_output, wl_seat, wl_surface};
    use wayland_protocols::{
        ext::session_lock::v1::client::{ext_session_lock_manager_v1, ext_session_lock_v1},
        xdg::shell::client::{xdg_popup, xdg_surface, xdg_wm_base},
    };
    use wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1, zwlr_layer_surface_v1,
    };

    use crate::{state::tests::fixture, test_support::TestClient};

    fn add_output(state: &mut BlairState) -> Output {
        add_named_output(state, "test-0", (0, 0))
    }

    fn add_named_output(state: &mut BlairState, name: &str, location: (i32, i32)) -> Output {
        let output = Output::new(
            name.into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Test".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (800, 600).into(),
                refresh: 60_000,
            }),
            None,
            None,
            Some(location.into()),
        );
        output.create_global::<BlairState>(&state.display_handle);
        state.add_output(&output, location.into());
        output
    }

    fn present_lock_frame(state: &mut BlairState, output: &Output) {
        let frame = state.pending_session_lock_frame(output).unwrap();
        state.session_lock_frame_presented(frame);
    }

    fn keyboard(client: &TestClient) -> wl_keyboard::WlKeyboard {
        client
            .bind::<wl_seat::WlSeat>(7)
            .get_keyboard(&client.handle(), ())
    }

    fn lock(client: &TestClient) -> (ext_session_lock_v1::ExtSessionLockV1, wl_surface::WlSurface) {
        let manager = client.bind::<ext_session_lock_manager_v1::ExtSessionLockManagerV1>(1);
        let lock = manager.lock(&client.handle(), ());
        let surface = client
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&client.handle(), ());
        lock.get_lock_surface(
            &surface,
            &client.bind::<wl_output::WlOutput>(4),
            &client.handle(),
            (),
        );
        (lock, surface)
    }

    fn key(state: &mut BlairState) {
        struct Hooks;
        impl crate::input::InputHooks for Hooks {}
        crate::input::keyboard_transition(state, 38, true, 1, &mut Hooks);
        crate::input::keyboard_transition(state, 38, false, 2, &mut Hooks);
    }

    fn popup(
        client: &TestClient,
        parent: &xdg_surface::XdgSurface,
    ) -> (xdg_popup::XdgPopup, wl_surface::WlSurface) {
        let shell = client.bind::<xdg_wm_base::XdgWmBase>(6);
        let surface = client
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&client.handle(), ());
        let xdg = shell.get_xdg_surface(&surface, &client.handle(), ());
        let positioner = shell.create_positioner(&client.handle(), ());
        positioner.set_size(20, 20);
        positioner.set_anchor_rect(0, 0, 20, 20);
        (
            xdg.get_popup(Some(parent), &positioner, &client.handle(), ()),
            surface,
        )
    }

    #[test]
    fn exclusive_layer_commits_cannot_receive_keys_during_a_session_lock() {
        let (mut display, _event_loop, mut state) = fixture();
        let _output = add_output(&mut state);
        let mut shell = TestClient::new(&mut display, &mut state);
        let _keyboard = keyboard(&shell);
        let layer_shell = shell.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1>(4);
        let surface = shell
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&shell.handle(), ());
        let layer = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "test".into(),
            &shell.handle(),
            (),
        );
        layer.set_size(800, 600);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
        surface.commit();
        shell.dispatch(&mut display, &mut state);
        surface.commit();
        shell.dispatch(&mut display, &mut state);
        assert_eq!(
            state.seat.get_keyboard().unwrap().current_focus(),
            Some(shell.surface(&surface))
        );
        key(&mut state);
        shell.dispatch(&mut display, &mut state);
        assert_eq!(shell.events.keys.len(), 2);
        shell.events.keys.clear();

        let mut locker = TestClient::new(&mut display, &mut state);
        let _keyboard = keyboard(&locker);
        let manager = locker.bind::<ext_session_lock_manager_v1::ExtSessionLockManagerV1>(1);
        let lock = manager.lock(&locker.handle(), ());
        locker.dispatch(&mut display, &mut state);
        assert!(state.session_locked);
        surface.commit();
        shell.dispatch(&mut display, &mut state);
        key(&mut state);
        shell.dispatch(&mut display, &mut state);
        assert!(shell.events.keys.is_empty());
        assert!(state.seat.get_keyboard().unwrap().current_focus().is_none());
        let lock_surface = locker
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&locker.handle(), ());
        lock.get_lock_surface(
            &lock_surface,
            &locker.bind::<wl_output::WlOutput>(4),
            &locker.handle(),
            (),
        );
        locker.dispatch(&mut display, &mut state);
        surface.commit();
        shell.dispatch(&mut display, &mut state);
        key(&mut state);
        shell.dispatch(&mut display, &mut state);
        locker.dispatch(&mut display, &mut state);
        assert!(shell.events.keys.is_empty());
        assert_eq!(locker.events.keys.len(), 2);
        assert_eq!(
            state.seat.get_keyboard().unwrap().current_focus(),
            Some(locker.surface(&lock_surface))
        );
        present_lock_frame(&mut state, &_output);
        locker.dispatch(&mut display, &mut state);
        lock.unlock_and_destroy();
        locker.dispatch(&mut display, &mut state);
        key(&mut state);
        shell.dispatch(&mut display, &mut state);
        assert!(!state.session_locked);
        assert_eq!(shell.events.keys.len(), 2);
    }

    #[test]
    fn locking_cancels_popup_grabs_and_rejects_new_normal_client_grabs() {
        let (mut display, _event_loop, mut state) = fixture();
        let _output = add_output(&mut state);
        let mut app = TestClient::new(&mut display, &mut state);
        let _keyboard = keyboard(&app);
        let seat = app.bind::<wl_seat::WlSeat>(7);
        let shell = app.bind::<xdg_wm_base::XdgWmBase>(6);
        let surface = app
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&app.handle(), ());
        let parent = shell.get_xdg_surface(&surface, &app.handle(), ());
        let _toplevel = parent.get_toplevel(&app.handle(), ());
        app.dispatch(&mut display, &mut state);
        let root = app.surface(&surface);
        state.set_keyboard_focus(Some(root.clone()));
        let pointer = state.seat.get_pointer().unwrap();
        let serial = SERIAL_COUNTER.next_serial();
        pointer.motion(
            &mut state,
            Some((root, (0.0, 0.0).into())),
            &MotionEvent {
                location: (10.0, 10.0).into(),
                serial,
                time: 0,
            },
        );
        pointer.button(
            &mut state,
            &ButtonEvent {
                serial,
                time: 0,
                button: 0x110,
                state: ButtonState::Pressed,
            },
        );
        let (first, _) = popup(&app, &parent);
        first.grab(&seat, serial.into());
        app.dispatch(&mut display, &mut state);
        assert!(pointer.is_grabbed());
        assert!(state.seat.get_keyboard().unwrap().is_grabbed());

        let mut locker = TestClient::new(&mut display, &mut state);
        let _keyboard = keyboard(&locker);
        let (_lock, lock_surface) = lock(&locker);
        locker.dispatch(&mut display, &mut state);
        assert!(state.session_locked);
        assert!(!pointer.is_grabbed());
        assert!(!state.seat.get_keyboard().unwrap().is_grabbed());
        assert!(pointer.current_focus().is_none());
        app.dispatch(&mut display, &mut state);
        assert_eq!(app.events.popups_dismissed, 1);
        let (second, _) = popup(&app, &parent);
        second.grab(&seat, serial.into());
        app.dispatch(&mut display, &mut state);
        assert!(!pointer.is_grabbed());
        assert!(!state.seat.get_keyboard().unwrap().is_grabbed());
        key(&mut state);
        app.dispatch(&mut display, &mut state);
        locker.dispatch(&mut display, &mut state);
        assert!(app.events.keys.is_empty());
        assert_eq!(locker.events.keys.len(), 2);
        assert_eq!(
            state.seat.get_keyboard().unwrap().current_focus(),
            Some(locker.surface(&lock_surface))
        );
    }

    #[test]
    fn locking_finishes_an_interactive_resize_and_prevents_new_window_grabs() {
        let (mut display, _event_loop, mut state) = fixture();
        let _output = add_output(&mut state);
        state.config.window.layout = crate::config::WindowLayout::Floating;
        let mut app = TestClient::new(&mut display, &mut state);
        let shell = app.bind::<xdg_wm_base::XdgWmBase>(6);
        let surface = app
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&app.handle(), ());
        let parent = shell.get_xdg_surface(&surface, &app.handle(), ());
        let _toplevel = parent.get_toplevel(&app.handle(), ());
        app.dispatch(&mut display, &mut state);
        let window = state.windows.values().next().unwrap().window.clone();
        let id = state.window_id(&window).unwrap();
        state.space.map_element(window.clone(), (20, 20), true);
        let edges = crate::grabs::ResizeEdges {
            left: false,
            top: false,
            right: true,
            bottom: true,
        };
        let serial = SERIAL_COUNTER.next_serial();
        assert!(state.begin_resize(&window, edges, serial, 0x110));
        let pointer = state.seat.get_pointer().unwrap();
        assert!(pointer.is_grabbed());
        assert!(state.windows[&id].resize.unwrap().1);
        let mut locker = TestClient::new(&mut display, &mut state);
        let (_lock, _) = lock(&locker);
        locker.dispatch(&mut display, &mut state);
        assert!(!pointer.is_grabbed());
        assert!(!state.windows[&id].resize.unwrap().1);
        assert!(state.compositor_cursor.is_none());
        assert!(!state.begin_move(&window, serial, 0x110));
        assert!(!state.begin_resize(&window, edges, serial, 0x110));
    }

    #[test]
    fn lock_confirmation_waits_for_a_presented_frame_on_every_output() {
        let (mut display, _event_loop, mut state) = fixture();
        let first = add_output(&mut state);
        let second = add_named_output(&mut state, "test-1", (800, 0));
        let mut locker = TestClient::new(&mut display, &mut state);
        let (_lock, _) = lock(&locker);
        locker.dispatch(&mut display, &mut state);
        let discarded = state.pending_session_lock_frame(&first).unwrap();
        drop(discarded);
        locker.dispatch(&mut display, &mut state);
        assert_eq!(locker.events.locks_confirmed, 0);
        let duplicate = state.pending_session_lock_frame(&first).unwrap();
        present_lock_frame(&mut state, &first);
        state.session_lock_frame_presented(duplicate);
        locker.dispatch(&mut display, &mut state);
        assert_eq!(locker.events.locks_confirmed, 0);
        assert!(state.pending_session_lock_frame(&first).is_none());
        present_lock_frame(&mut state, &second);
        locker.dispatch(&mut display, &mut state);
        assert_eq!(locker.events.locks_confirmed, 1);
        assert!(state.pending_session_lock.is_none());
    }

    #[test]
    fn presentation_from_a_previous_lock_cannot_confirm_a_new_lock() {
        use smithay::backend::renderer::element::{solid::SolidColorRenderElement, Element, Kind};
        let (mut display, _event_loop, mut state) = fixture();
        let output = add_output(&mut state);
        let mut locker = TestClient::new(&mut display, &mut state);
        let (first, _) = lock(&locker);
        locker.dispatch(&mut display, &mut state);
        let stale = state.pending_session_lock_frame(&output).unwrap();
        let background = SolidColorRenderElement::from_buffer(
            &state.session_lock_buffer,
            (0, 0),
            1.0,
            1.0,
            Kind::Unspecified,
        )
        .id()
        .clone();
        present_lock_frame(&mut state, &output);
        locker.dispatch(&mut display, &mut state);
        first.unlock_and_destroy();
        locker.dispatch(&mut display, &mut state);
        let (_second, _) = lock(&locker);
        locker.dispatch(&mut display, &mut state);
        assert_ne!(
            SolidColorRenderElement::from_buffer(
                &state.session_lock_buffer,
                (0, 0),
                1.0,
                1.0,
                Kind::Unspecified
            )
            .id(),
            &background
        );
        state.session_lock_frame_presented(stale);
        locker.dispatch(&mut display, &mut state);
        assert_eq!(locker.events.locks_confirmed, 1);
        assert!(state.pending_session_lock.is_some());
        present_lock_frame(&mut state, &output);
        locker.dispatch(&mut display, &mut state);
        assert_eq!(locker.events.locks_confirmed, 2);
    }

    #[test]
    fn a_session_without_outputs_can_confirm_its_lock_immediately() {
        let (mut display, _event_loop, mut state) = fixture();
        let mut locker = TestClient::new(&mut display, &mut state);
        let manager = locker.bind::<ext_session_lock_manager_v1::ExtSessionLockManagerV1>(1);
        let _lock = manager.lock(&locker.handle(), ());
        locker.dispatch(&mut display, &mut state);
        assert!(state.session_locked);
        assert_eq!(locker.events.locks_confirmed, 1);
        assert!(state.pending_session_lock.is_none());
    }

    #[test]
    fn rejected_lockers_cannot_add_surfaces_to_another_clients_lock() {
        let (mut display, _event_loop, mut state) = fixture();
        let _output = add_output(&mut state);
        let mut locker = TestClient::new(&mut display, &mut state);
        let _keyboard = keyboard(&locker);
        let (_lock, surface) = lock(&locker);
        locker.dispatch(&mut display, &mut state);
        present_lock_frame(&mut state, &_output);
        locker.dispatch(&mut display, &mut state);
        assert_eq!(locker.events.locks_confirmed, 1);
        let mut rejected = TestClient::new(&mut display, &mut state);
        let (_lock, _) = lock(&rejected);
        rejected.dispatch(&mut display, &mut state);
        assert_eq!(rejected.events.locks_rejected, 1);
        assert_eq!(state.session_lock_surfaces.len(), 1);
        assert_eq!(
            state.seat.get_keyboard().unwrap().current_focus(),
            Some(locker.surface(&surface))
        );
    }
}
