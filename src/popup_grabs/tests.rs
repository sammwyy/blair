use super::*;
use smithay::{
    backend::input::InputEvent,
    desktop::PopupUngrabStrategy,
    input::pointer::MotionEvent,
    reexports::{calloop::EventLoop, wayland_server::Display},
    utils::{Logical, Point},
    wayland::seat::WaylandFocus,
};
use wayland_client::protocol::{
    wl_compositor, wl_keyboard, wl_pointer, wl_seat, wl_surface, wl_touch,
};
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_surface, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::{
    input::{process_input_event, InputHooks},
    test_support::{
        input::{TestEvent, TestInputBackend},
        TestClient,
    },
};

struct Hooks;
impl InputHooks for Hooks {}

struct Fixture {
    display: Display<BlairState>,
    _event_loop: EventLoop<'static, BlairState>,
    state: BlairState,
}

impl Fixture {
    fn new() -> Self {
        let (display, event_loop, mut state) = crate::state::tests::fixture();
        state.seat.add_touch();
        state.config.decorations.mode = crate::config::DecorationModeConfig::None;
        crate::test_support::add_output(&mut state, "test", (0, 0), (800, 600));
        Self {
            display,
            _event_loop: event_loop,
            state,
        }
    }

    fn event(&mut self, event: InputEvent<TestInputBackend>) {
        process_input_event(&mut self.state, event, &mut Hooks);
    }

    fn focus(&mut self, app: &App) {
        self.state.set_keyboard_focus(Some(app.root()));
    }

    fn assert_focus(&self, surface: &WlSurface) {
        assert_eq!(
            self.state
                .seat
                .get_keyboard()
                .unwrap()
                .current_focus()
                .as_ref(),
            Some(surface)
        );
    }
}

struct App {
    client: TestClient,
    seat: wl_seat::WlSeat,
    surface: wl_surface::WlSurface,
    shell: xdg_surface::XdgSurface,
    origin: Point<i32, Logical>,
    _keyboard: wl_keyboard::WlKeyboard,
    _pointer: wl_pointer::WlPointer,
    _touch: wl_touch::WlTouch,
}

struct Popup {
    popup: xdg_popup::XdgPopup,
    surface: wl_surface::WlSurface,
    shell: xdg_surface::XdgSurface,
}

impl App {
    fn new(f: &mut Fixture, x: i32) -> Self {
        let mut client = TestClient::new(&mut f.display, &mut f.state);
        let seat = client.bind::<wl_seat::WlSeat>(7);
        let keyboard = seat.get_keyboard(&client.handle(), ());
        let pointer = seat.get_pointer(&client.handle(), ());
        let touch = seat.get_touch(&client.handle(), ());
        let surface = client
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&client.handle(), ());
        let wm = client.bind::<xdg_wm_base::XdgWmBase>(6);
        let shell = wm.get_xdg_surface(&surface, &client.handle(), ());
        shell.get_toplevel(&client.handle(), ());
        surface.commit();
        client.dispatch(&mut f.display, &mut f.state);
        client.paint_surface(&surface, 300, 240);
        client.dispatch(&mut f.display, &mut f.state);
        let root = client.surface(&surface);
        let window = f
            .state
            .windows
            .values()
            .find(|managed| managed.window.wl_surface().as_deref() == Some(&root))
            .unwrap()
            .window
            .clone();
        let origin = (x, 100).into();
        f.state.space.map_element(window, origin, false);
        Self {
            client,
            seat,
            surface,
            shell,
            origin,
            _keyboard: keyboard,
            _pointer: pointer,
            _touch: touch,
        }
    }

    fn root(&self) -> WlSurface {
        self.client.surface(&self.surface)
    }
    fn dispatch(&mut self, f: &mut Fixture) {
        self.client.dispatch(&mut f.display, &mut f.state);
    }

    fn popup(&self, parent: &xdg_surface::XdgSurface) -> Popup {
        let wm = self.client.bind::<xdg_wm_base::XdgWmBase>(6);
        let surface = self
            .client
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&self.client.handle(), ());
        let shell = wm.get_xdg_surface(&surface, &self.client.handle(), ());
        let positioner = wm.create_positioner(&self.client.handle(), ());
        positioner.set_size(30, 30);
        positioner.set_anchor_rect(0, 0, 30, 30);
        let popup = shell.get_popup(Some(parent), &positioner, &self.client.handle(), ());
        Popup {
            popup,
            surface,
            shell,
        }
    }

    fn key(&mut self, f: &mut Fixture, pressed: bool) -> Serial {
        crate::input::keyboard_transition(&mut f.state, 38, pressed, 1, &mut Hooks);
        self.dispatch(f);
        (*self.client.events.key_serials.last().unwrap()).into()
    }

    fn pointer(&mut self, f: &mut Fixture, pressed: bool) -> Serial {
        if pressed {
            let pointer = f.state.seat.get_pointer().unwrap();
            pointer.motion(
                &mut f.state,
                Some((self.root(), self.origin.to_f64())),
                &MotionEvent {
                    location: (self.origin + Point::from((20, 20))).to_f64(),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: 1,
                },
            );
        }
        f.event(InputEvent::PointerButton {
            event: TestEvent {
                pressed,
                ..Default::default()
            },
        });
        self.dispatch(f);
        self.client
            .events
            .pointer
            .iter()
            .rev()
            .find_map(|event| {
                if let wl_pointer::Event::Button { serial, .. } = event {
                    Some((*serial).into())
                } else {
                    None
                }
            })
            .unwrap()
    }

    fn request(&mut self, f: &mut Fixture, popup: &Popup, serial: Serial) {
        popup.popup.grab(&self.seat, serial.into());
        self.dispatch(f);
    }

    fn touch_down(&self, f: &mut Fixture, slot: u32) {
        f.event(InputEvent::TouchDown {
            event: TestEvent {
                x: f64::from(self.origin.x + 20) / 800.0,
                y: f64::from(self.origin.y + 20) / 600.0,
                slot,
                ..Default::default()
            },
        });
        f.event(InputEvent::TouchFrame {
            event: TestEvent::default(),
        });
    }

    fn touch_serial(&self) -> Serial {
        self.client
            .events
            .touch
            .iter()
            .rev()
            .find_map(|event| {
                if let wl_touch::Event::Down { serial, .. } = event {
                    Some((*serial).into())
                } else {
                    None
                }
            })
            .unwrap()
    }
}

#[test]
fn forged_and_foreign_serials_do_not_move_focus_or_install_grabs() {
    for kind in [InputKind::Keyboard, InputKind::Pointer] {
        let mut f = Fixture::new();
        let mut owner = App::new(&mut f, 20);
        let mut other = App::new(&mut f, 440);
        f.focus(&owner);
        let serial = match kind {
            InputKind::Keyboard => owner.key(&mut f, true),
            _ => owner.pointer(&mut f, true),
        };
        let rejected = other.popup(&other.shell);
        f.state.take_redraw_request();
        other.request(&mut f, &rejected, serial);
        assert!(f.state.take_redraw_request());
        f.assert_focus(&owner.root());
        assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
        assert_eq!(other.client.events.popups_dismissed, 1);
        assert!(f
            .state
            .popup_manager
            .find_popup(&other.client.surface(&rejected.surface))
            .is_none());
        let forged = owner.popup(&owner.shell);
        owner.request(&mut f, &forged, 0.into());
        f.assert_focus(&owner.root());
        assert_eq!(owner.client.events.popups_dismissed, 1);
        owner.request(&mut f, &forged, serial);
        f.assert_focus(&owner.root());
        assert_eq!(owner.client.events.popups_dismissed, 2);
        let accepted = owner.popup(&owner.shell);
        owner.request(&mut f, &accepted, serial);
        f.assert_focus(&owner.client.surface(&accepted.surface));
        assert!(f.state.seat.get_keyboard().unwrap().is_grabbed());
    }
}

#[test]
fn pointer_and_keyboard_press_serials_survive_release_but_release_serials_are_rejected() {
    for kind in [InputKind::Keyboard, InputKind::Pointer] {
        let mut f = Fixture::new();
        let mut app = App::new(&mut f, 20);
        f.focus(&app);
        let press = match kind {
            InputKind::Keyboard => app.key(&mut f, true),
            _ => app.pointer(&mut f, true),
        };
        let release = match kind {
            InputKind::Keyboard => app.key(&mut f, false),
            _ => app.pointer(&mut f, false),
        };
        let rejected = app.popup(&app.shell);
        app.request(&mut f, &rejected, release);
        assert_eq!(app.client.events.popups_dismissed, 1);
        let accepted = app.popup(&app.shell);
        app.request(&mut f, &accepted, press);
        f.assert_focus(&app.client.surface(&accepted.surface));
    }
}

#[test]
fn invalid_nested_grabs_leave_the_parent_active_and_valid_submenus_can_reuse_the_origin() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    f.focus(&app);
    let serial = app.key(&mut f, true);
    let first = app.popup(&app.shell);
    app.request(&mut f, &first, serial);
    let focus = app.client.surface(&first.surface);
    let invalid = app.popup(&first.shell);
    app.request(&mut f, &invalid, 0.into());
    assert_eq!(app.client.events.popups_dismissed, 1);
    f.assert_focus(&focus);
    assert_eq!(
        f.state.active_popup_grab.as_ref().unwrap().current_grab(),
        Some(focus.clone())
    );
    let sibling = app.popup(&app.shell);
    app.request(&mut f, &sibling, serial);
    assert_eq!(app.client.events.popups_dismissed, 2);
    f.assert_focus(&focus);
    let child = app.popup(&first.shell);
    app.request(&mut f, &child, serial);
    f.assert_focus(&app.client.surface(&child.surface));
    app.key(&mut f, false);
    let next_serial = app.key(&mut f, true);
    let grandchild = app.popup(&child.shell);
    app.request(&mut f, &grandchild, next_serial);
    let deepest = app.popup(&grandchild.shell);
    app.request(&mut f, &deepest, serial);
    f.assert_focus(&app.client.surface(&deepest.surface));
    deepest.popup.destroy();
    app.dispatch(&mut f);
    f.state.refresh();
    f.assert_focus(&app.client.surface(&grandchild.surface));
}

#[test]
fn dismissed_serials_cannot_reopen_popups_and_stale_handles_do_not_block_new_keys() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    f.focus(&app);
    let first_serial = app.key(&mut f, true);
    let first = app.popup(&app.shell);
    app.request(&mut f, &first, first_serial);
    f.state
        .active_popup_grab
        .as_mut()
        .unwrap()
        .ungrab(PopupUngrabStrategy::All);
    let replay = app.popup(&app.shell);
    app.request(&mut f, &replay, first_serial);
    f.assert_focus(&app.root());
    assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
    assert!(!f.state.seat.get_pointer().unwrap().is_grabbed());
    assert_eq!(app.client.events.popups_dismissed, 2);
    app.key(&mut f, false);
    let next_serial = app.key(&mut f, true);
    let accepted = app.popup(&app.shell);
    app.request(&mut f, &accepted, next_serial);
    f.assert_focus(&app.client.surface(&accepted.surface));
    accepted.popup.destroy();
    app.dispatch(&mut f);
    f.state.refresh();
    f.assert_focus(&app.root());
    assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
    assert!(!f.state.seat.get_pointer().unwrap().is_grabbed());
}

#[test]
fn serials_from_an_old_focus_or_a_different_seat_cannot_authorize_a_grab() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    f.focus(&app);
    let serial = app.key(&mut f, true);
    f.state.set_keyboard_focus(None);
    f.focus(&app);
    let stale = app.popup(&app.shell);
    app.request(&mut f, &stale, serial);
    assert_eq!(app.client.events.popups_dismissed, 1);
    app.key(&mut f, false);
    let serial = app.key(&mut f, true);
    let _foreign_seat = f
        .state
        .seat_state
        .new_wl_seat(&f.state.display_handle, "other");
    app.dispatch(&mut f);
    let name = app
        .client
        .events
        .globals
        .iter()
        .rev()
        .find(|(_, interface, _)| interface == "wl_seat")
        .unwrap()
        .0;
    let foreign = app.client.bind_global::<wl_seat::WlSeat>(name, 7);
    let rejected = app.popup(&app.shell);
    rejected.popup.grab(&foreign, serial.into());
    app.dispatch(&mut f);
    assert_eq!(app.client.events.popups_dismissed, 2);
    f.assert_focus(&app.root());
    let accepted = app.popup(&app.shell);
    app.request(&mut f, &accepted, serial);
    f.assert_focus(&app.client.surface(&accepted.surface));
}

#[test]
fn compositor_shortcuts_are_not_recorded_and_keyboard_reset_revokes_input() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    f.focus(&app);
    for raw in [37, 64] {
        crate::input::keyboard_transition(&mut f.state, raw, true, 1, &mut Hooks);
    }
    app.dispatch(&mut f);
    let serial: Serial = (*app.client.events.key_serials.last().unwrap()).into();
    let presses = f.state.popup_input.presses.len();
    let keys = app.client.events.keys.len();
    crate::input::keyboard_transition(&mut f.state, 67, true, 2, &mut Hooks);
    app.dispatch(&mut f);
    assert!(f.state.suppressed_keys.contains(&67));
    assert_eq!(app.client.events.keys.len(), keys);
    assert_eq!(f.state.popup_input.presses.len(), presses);
    assert!(!f.state.popup_input.presses.is_empty());
    crate::input::reset_keyboard_state(&mut f.state);
    assert!(f.state.popup_input.presses.is_empty());
    let rejected = app.popup(&app.shell);
    app.request(&mut f, &rejected, serial);
    assert_eq!(app.client.events.popups_dismissed, 1);
    f.assert_focus(&app.root());
}

#[test]
fn popup_requests_cannot_replace_an_interactive_pointer_grab() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    let window = f
        .state
        .windows
        .values()
        .find(|managed| managed.window.wl_surface().as_deref() == Some(&app.root()))
        .unwrap()
        .window
        .clone();
    let move_serial = SERIAL_COUNTER.next_serial();
    assert!(f.state.begin_move(&window, move_serial, 0x110));
    let rejected = app.popup(&app.shell);
    app.request(&mut f, &rejected, move_serial);
    assert_eq!(app.client.events.popups_dismissed, 1);
    let serial = app.key(&mut f, true);
    let rejected = app.popup(&app.shell);
    app.request(&mut f, &rejected, serial);
    assert_eq!(app.client.events.popups_dismissed, 2);
    assert!(f.state.seat.get_pointer().unwrap().has_grab(move_serial));
    assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
    assert!(f.state.active_popup_grab.is_none());
    f.assert_focus(&app.root());
}

#[test]
fn pointer_input_cannot_take_focus_from_an_exclusive_layer() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    let mut shell = TestClient::new(&mut f.display, &mut f.state);
    let surface = shell
        .bind::<wl_compositor::WlCompositor>(6)
        .create_surface(&shell.handle(), ());
    let layer = shell
        .bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1>(4)
        .get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "test".into(),
            &shell.handle(),
            (),
        );
    layer.set_size(800, 30);
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    for _ in 0..2 {
        surface.commit();
        shell.dispatch(&mut f.display, &mut f.state);
    }
    let focus = shell.surface(&surface);
    f.assert_focus(&focus);
    let serial = app.pointer(&mut f, true);
    f.assert_focus(&focus);
    let rejected = app.popup(&app.shell);
    app.request(&mut f, &rejected, serial);
    assert_eq!(app.client.events.popups_dismissed, 1);
    f.assert_focus(&focus);
    assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
}

#[test]
fn touch_down_authorizes_popups_after_release_but_cancellation_revokes_it() {
    for cancelled in [false, true] {
        let mut f = Fixture::new();
        let mut app = App::new(&mut f, 20);
        app.touch_down(&mut f, 0);
        app.dispatch(&mut f);
        let serial = app.touch_serial();
        f.event(if cancelled {
            InputEvent::TouchCancel {
                event: TestEvent::default(),
            }
        } else {
            InputEvent::TouchUp {
                event: TestEvent::default(),
            }
        });
        f.event(InputEvent::TouchFrame {
            event: TestEvent::default(),
        });
        let popup = app.popup(&app.shell);
        app.request(&mut f, &popup, serial);
        if cancelled {
            f.assert_focus(&app.root());
            assert_eq!(app.client.events.popups_dismissed, 1);
            assert!(f.state.active_popup_grab.is_none());
        } else {
            f.assert_focus(&app.client.surface(&popup.surface));
            assert_eq!(app.client.events.popups_dismissed, 0);
        }
    }
}

#[test]
fn touch_serials_belong_to_the_implicit_grab_recipient() {
    let mut f = Fixture::new();
    let mut owner = App::new(&mut f, 20);
    let mut other = App::new(&mut f, 440);
    owner.touch_down(&mut f, 0);
    owner.dispatch(&mut f);
    other.touch_down(&mut f, 1);
    owner.dispatch(&mut f);
    other.dispatch(&mut f);
    assert!(other.client.events.touch.is_empty());
    let serial = owner.touch_serial();
    assert_eq!(
        owner
            .client
            .events
            .touch
            .iter()
            .filter(|event| { matches!(event, wl_touch::Event::Down { .. }) })
            .count(),
        2
    );
    let rejected = other.popup(&other.shell);
    other.request(&mut f, &rejected, serial);
    assert_eq!(other.client.events.popups_dismissed, 1);
    let accepted = owner.popup(&owner.shell);
    owner.request(&mut f, &accepted, serial);
    f.assert_focus(&owner.client.surface(&accepted.surface));
}

#[test]
fn input_history_is_bounded_expires_and_reuses_storage() {
    let mut f = Fixture::new();
    let app = App::new(&mut f, 20);
    let focus = app.root();
    let mut input = PopupInput::default();
    for index in 0..MAX_INPUTS + 8 {
        input.record(
            InputKind::Keyboard,
            (index as u32).into(),
            &focus,
            Duration::from_secs(1),
        );
    }
    assert_eq!(input.presses.len(), MAX_INPUTS);
    assert!(input
        .input(0.into(), &focus, Duration::from_secs(1))
        .is_none());
    assert_eq!(
        input.input((MAX_INPUTS as u32).into(), &focus, Duration::from_secs(1)),
        Some(InputKind::Keyboard)
    );
    assert!(input
        .input((MAX_INPUTS as u32).into(), &focus, Duration::from_secs(7))
        .is_none());
    assert_eq!(
        crate::test_support::count_allocations(|| {
            for index in 0..1000 {
                input.record(
                    InputKind::Pointer,
                    (index as u32).into(),
                    &focus,
                    Duration::from_secs(1),
                );
            }
        }),
        0
    );
    input.record(
        InputKind::Touch,
        1001.into(),
        &focus,
        Duration::from_secs(7),
    );
    assert_eq!(input.presses.len(), 1);
    input.revoke(InputKind::Touch);
    assert!(input.presses.is_empty());
    let bare = app
        .client
        .bind::<wl_compositor::WlCompositor>(6)
        .create_surface(&app.client.handle(), ());
    let mut app = app;
    app.dispatch(&mut f);
    let server = app.client.surface(&bare);
    input.record(
        InputKind::Keyboard,
        1002.into(),
        &server,
        Duration::from_secs(7),
    );
    bare.destroy();
    app.dispatch(&mut f);
    assert!(input
        .input(1002.into(), &focus, Duration::from_secs(7))
        .is_none());
}
