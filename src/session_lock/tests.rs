use super::*;
use smithay::{
    backend::input::ButtonState,
    input::pointer::{ButtonEvent, MotionEvent},
    output::Mode,
    utils::SERIAL_COUNTER,
};
use wayland_client::protocol::{wl_compositor, wl_keyboard, wl_output, wl_seat, wl_surface};
use wayland_protocols::{
    ext::session_lock::v1::client::{ext_session_lock_manager_v1, ext_session_lock_v1},
    xdg::shell::client::{xdg_popup, xdg_surface, xdg_wm_base},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::{state::tests::fixture, test_support::TestClient};

fn add_output(state: &mut BlairState) -> Output {
    add_named_output(state, "test-0", (0, 0))
}

fn add_named_output(state: &mut BlairState, name: &str, location: (i32, i32)) -> Output {
    crate::test_support::add_output(state, name, location, (800, 600))
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

struct InputFixture {
    display: smithay::reexports::wayland_server::Display<BlairState>,
    event_loop: smithay::reexports::calloop::EventLoop<'static, BlairState>,
    state: BlairState,
    output: Output,
    app: TestClient,
    app_surface: wl_surface::WlSurface,
    locker: TestClient,
    lock: ext_session_lock_v1::ExtSessionLockV1,
    surface: wl_surface::WlSurface,
}

impl InputFixture {
    fn new() -> Self {
        use crate::test_support::input::{position, TestEvent};
        use smithay::{backend::input::InputEvent, output::Scale};
        let (mut display, event_loop, mut state) = fixture();
        state.seat.add_touch();
        let output = add_named_output(&mut state, "test-0", (100, 80));
        output.change_current_state(
            Some(Mode {
                size: (1600, 1200).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Integer(2)),
            None,
        );
        let mut app = TestClient::new(&mut display, &mut state);
        let seat = app.bind::<wl_seat::WlSeat>(7);
        seat.get_pointer(&app.handle(), ());
        seat.get_touch(&app.handle(), ());
        let compositor = app.bind::<wl_compositor::WlCompositor>(6);
        let app_surface = compositor.create_surface(&app.handle(), ());
        let layer_shell = app.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1>(4);
        let layer = layer_shell.get_layer_surface(
            &app_surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "normal-input".into(),
            &app.handle(),
            (),
        );
        layer.set_size(800, 600);
        app_surface.commit();
        app.dispatch(&mut display, &mut state);
        app.paint_surface(&app_surface, 800, 600);
        app.dispatch(&mut display, &mut state);
        feed(
            &mut state,
            InputEvent::PointerMotionAbsolute {
                event: position(0.25, 0.25),
            },
        );
        feed(
            &mut state,
            InputEvent::TouchDown {
                event: TestEvent {
                    slot: 6,
                    ..position(0.25, 0.25)
                },
            },
        );
        feed(
            &mut state,
            InputEvent::TouchFrame {
                event: TestEvent::default(),
            },
        );
        app.dispatch(&mut display, &mut state);
        assert_eq!(
            state.seat.get_pointer().unwrap().current_focus(),
            Some(app.surface(&app_surface))
        );
        let mut locker = TestClient::new(&mut display, &mut state);
        let seat = locker.bind::<wl_seat::WlSeat>(7);
        seat.get_keyboard(&locker.handle(), ());
        seat.get_pointer(&locker.handle(), ());
        seat.get_touch(&locker.handle(), ());
        let (lock, surface) = lock(&locker);
        locker.dispatch(&mut display, &mut state);
        locker.paint_surface(&surface, 800, 600);
        locker.dispatch(&mut display, &mut state);
        state.refresh();
        locker.dispatch(&mut display, &mut state);
        app.dispatch(&mut display, &mut state);
        assert!(app
            .events
            .touch
            .iter()
            .any(|event| matches!(event, wayland_client::protocol::wl_touch::Event::Cancel)));
        app.events.pointer.clear();
        app.events.touch.clear();
        Self {
            display,
            event_loop,
            state,
            output,
            app,
            app_surface,
            locker,
            lock,
            surface,
        }
    }

    fn dispatch(&mut self) {
        self.state.refresh();
        self.app.dispatch(&mut self.display, &mut self.state);
        self.locker.dispatch(&mut self.display, &mut self.state);
    }

    fn unlock(&mut self) {
        present_lock_frame(&mut self.state, &self.output);
        self.locker.dispatch(&mut self.display, &mut self.state);
        self.lock.unlock_and_destroy();
        self.locker.dispatch(&mut self.display, &mut self.state);
        self.dispatch();
    }
}

fn feed(
    state: &mut BlairState,
    event: smithay::backend::input::InputEvent<crate::test_support::input::TestInputBackend>,
) {
    struct Hooks;
    impl crate::input::InputHooks for Hooks {}
    crate::input::process_input_event(state, event, &mut Hooks);
}

#[test]
fn locked_pointer_events_reach_only_the_locker_and_do_not_trigger_hot_corners() {
    use crate::test_support::input::{position, TestEvent};
    use smithay::backend::input::InputEvent;
    use wayland_client::protocol::wl_pointer;
    let mut f = InputFixture::new();
    let pointer = f.state.seat.get_pointer().unwrap();
    assert_eq!(pointer.current_focus(), Some(f.locker.surface(&f.surface)));
    feed(
        &mut f.state,
        InputEvent::PointerMotionAbsolute {
            event: position(0.5, 0.25),
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerMotion {
            event: position(25.0, -10.0),
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent {
                pressed: true,
                ..TestEvent::default()
            },
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent::default(),
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerAxis {
            event: position(0.0, 12.0),
        },
    );
    f.dispatch();
    assert!(f.app.events.pointer.is_empty());
    assert!(f.locker.events.pointer.iter().any(|event| matches!(event, wl_pointer::Event::Motion { surface_x, surface_y, .. } if *surface_x == 400.0 && *surface_y == 150.0)));
    assert!(f.locker.events.pointer.iter().any(|event| matches!(event, wl_pointer::Event::Motion { surface_x, surface_y, .. } if *surface_x == 425.0 && *surface_y == 140.0)));
    assert_eq!(
        f.locker
            .events
            .pointer
            .iter()
            .filter(|event| matches!(event, wl_pointer::Event::Button { .. }))
            .count(),
        2
    );
    assert!(f
        .locker
        .events
        .pointer
        .iter()
        .any(|event| matches!(event, wl_pointer::Event::Axis { value, .. } if *value == 12.0)));
    f.state.config.hot_corners.top_left.action = crate::hot_corners::CornerAction::NextWorkspace;
    f.state.config.hot_corners.dwell_ms = 0;
    let workspace = f.state.focused_workspace_id();
    feed(
        &mut f.state,
        InputEvent::PointerMotionAbsolute {
            event: position(0.001, 0.001),
        },
    );
    f.event_loop
        .dispatch(std::time::Duration::ZERO, &mut f.state)
        .unwrap();
    assert_eq!(f.state.focused_workspace_id(), workspace);
    f.unlock();
    assert_eq!(pointer.current_focus(), Some(f.app.surface(&f.app_surface)));
}

#[test]
fn locked_touch_events_keep_surface_coordinates_and_cancel_before_unlock() {
    use crate::test_support::input::{position, TestEvent};
    use smithay::backend::input::InputEvent;
    use wayland_client::protocol::wl_touch;
    let mut f = InputFixture::new();
    feed(
        &mut f.state,
        InputEvent::TouchDown {
            event: position(0.25, 0.25),
        },
    );
    feed(
        &mut f.state,
        InputEvent::TouchMotion {
            event: position(0.5, 0.5),
        },
    );
    feed(
        &mut f.state,
        InputEvent::TouchUp {
            event: TestEvent::default(),
        },
    );
    feed(
        &mut f.state,
        InputEvent::TouchFrame {
            event: TestEvent::default(),
        },
    );
    f.dispatch();
    assert!(f.app.events.touch.is_empty());
    assert!(f.locker.events.touch.iter().any(
        |event| matches!(event, wl_touch::Event::Down { x, y, .. } if *x == 200.0 && *y == 150.0)
    ));
    assert!(f.locker.events.touch.iter().any(
        |event| matches!(event, wl_touch::Event::Motion { x, y, .. } if *x == 400.0 && *y == 300.0)
    ));
    assert!(f
        .locker
        .events
        .touch
        .iter()
        .any(|event| matches!(event, wl_touch::Event::Up { .. })));
    feed(
        &mut f.state,
        InputEvent::TouchDown {
            event: TestEvent {
                slot: 1,
                ..position(0.5, 0.5)
            },
        },
    );
    f.dispatch();
    f.locker.events.touch.clear();
    f.unlock();
    assert!(f
        .locker
        .events
        .touch
        .iter()
        .any(|event| matches!(event, wl_touch::Event::Cancel)));
    f.locker.events.touch.clear();
    feed(
        &mut f.state,
        InputEvent::TouchMotion {
            event: TestEvent {
                slot: 1,
                ..position(0.75, 0.75)
            },
        },
    );
    feed(
        &mut f.state,
        InputEvent::TouchUp {
            event: TestEvent {
                slot: 1,
                ..TestEvent::default()
            },
        },
    );
    feed(
        &mut f.state,
        InputEvent::TouchDown {
            event: position(0.25, 0.25),
        },
    );
    feed(
        &mut f.state,
        InputEvent::TouchFrame {
            event: TestEvent::default(),
        },
    );
    f.dispatch();
    assert!(f.locker.events.touch.is_empty());
    assert!(f
        .app
        .events
        .touch
        .iter()
        .any(|event| matches!(event, wl_touch::Event::Down { .. })));
}

#[test]
fn lock_hit_testing_respects_regions_and_subsurfaces_without_falling_through() {
    use crate::test_support::input::{position, TestEvent};
    use smithay::backend::input::InputEvent;
    use wayland_client::protocol::{wl_pointer, wl_subcompositor};
    let mut f = InputFixture::new();
    let compositor = f.locker.bind::<wl_compositor::WlCompositor>(6);
    let region = compositor.create_region(&f.locker.handle(), ());
    region.add(0, 0, 100, 100);
    f.surface.set_input_region(Some(&region));
    f.surface.commit();
    f.dispatch();
    f.locker.events.pointer.clear();
    feed(
        &mut f.state,
        InputEvent::PointerMotionAbsolute {
            event: position(0.5, 0.5),
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent {
                pressed: true,
                ..TestEvent::default()
            },
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent::default(),
        },
    );
    feed(
        &mut f.state,
        InputEvent::TouchDown {
            event: position(0.5, 0.5),
        },
    );
    feed(
        &mut f.state,
        InputEvent::TouchUp {
            event: TestEvent::default(),
        },
    );
    f.dispatch();
    assert!(f.app.events.pointer.is_empty());
    assert!(f.app.events.touch.is_empty());
    assert!(f
        .state
        .seat
        .get_pointer()
        .unwrap()
        .current_focus()
        .is_none());
    assert!(!f
        .locker
        .events
        .pointer
        .iter()
        .any(|event| matches!(event, wl_pointer::Event::Button { .. })));
    assert!(f.locker.events.touch.is_empty());

    let child = compositor.create_surface(&f.locker.handle(), ());
    let subsurface = f
        .locker
        .bind::<wl_subcompositor::WlSubcompositor>(1)
        .get_subsurface(&child, &f.surface, &f.locker.handle(), ());
    subsurface.set_position(200, 100);
    f.locker.paint_surface(&child, 200, 100);
    f.surface.commit();
    f.dispatch();
    feed(
        &mut f.state,
        InputEvent::PointerMotionAbsolute {
            event: position(0.3125, 0.25),
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent {
                pressed: true,
                ..TestEvent::default()
            },
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent::default(),
        },
    );
    f.dispatch();
    assert_eq!(
        f.state.seat.get_pointer().unwrap().current_focus(),
        Some(f.locker.surface(&child))
    );
    assert!(f.locker.events.pointer.iter().any(|event| matches!(event, wl_pointer::Event::Enter { surface_x, surface_y, .. } | wl_pointer::Event::Motion { surface_x, surface_y, .. } if *surface_x == 50.0 && *surface_y == 50.0)));
    assert_eq!(
        f.state.seat.get_keyboard().unwrap().current_focus(),
        Some(f.locker.surface(&f.surface))
    );
    assert!(f.app.events.pointer.is_empty());
}

#[test]
fn cancelling_a_completed_touch_frame_releases_every_contact() {
    use crate::test_support::input::{position, TestEvent};
    use smithay::backend::input::InputEvent;
    use wayland_client::protocol::wl_touch;
    let mut f = InputFixture::new();
    for slot in [0, 1] {
        feed(
            &mut f.state,
            InputEvent::TouchDown {
                event: TestEvent {
                    slot,
                    ..position(0.25, 0.25)
                },
            },
        );
    }
    feed(
        &mut f.state,
        InputEvent::TouchFrame {
            event: TestEvent::default(),
        },
    );
    f.dispatch();
    f.locker.events.touch.clear();
    feed(
        &mut f.state,
        InputEvent::TouchCancel {
            event: TestEvent::default(),
        },
    );
    f.dispatch();
    assert!(f.state.touch_points.is_empty());
    assert!(!f.state.seat.get_touch().unwrap().is_grabbed());
    assert_eq!(
        f.locker
            .events
            .touch
            .iter()
            .filter(|event| matches!(event, wl_touch::Event::Cancel))
            .count(),
        1
    );
    f.locker.events.touch.clear();
    for slot in [0, 1] {
        feed(
            &mut f.state,
            InputEvent::TouchMotion {
                event: TestEvent {
                    slot,
                    ..position(0.75, 0.75)
                },
            },
        );
        feed(
            &mut f.state,
            InputEvent::TouchUp {
                event: TestEvent {
                    slot,
                    ..TestEvent::default()
                },
            },
        );
    }
    feed(
        &mut f.state,
        InputEvent::TouchFrame {
            event: TestEvent::default(),
        },
    );
    f.dispatch();
    assert!(f.locker.events.touch.is_empty());
    assert!(f.app.events.touch.is_empty());
}

#[test]
fn clicking_a_lock_surface_selects_its_output_and_preserves_that_keyboard_focus() {
    use crate::test_support::input::{position, TestEvent};
    use smithay::backend::input::InputEvent;
    let mut f = InputFixture::new();
    let _second = add_named_output(&mut f.state, "test-1", (900, 80));
    f.locker.dispatch(&mut f.display, &mut f.state);
    let name = f
        .locker
        .events
        .globals
        .iter()
        .filter(|(_, interface, _)| interface == "wl_output")
        .nth(1)
        .unwrap()
        .0;
    let output = f.locker.bind_global::<wl_output::WlOutput>(name, 4);
    let surface = f
        .locker
        .bind::<wl_compositor::WlCompositor>(6)
        .create_surface(&f.locker.handle(), ());
    f.lock
        .get_lock_surface(&surface, &output, &f.locker.handle(), ());
    f.locker.dispatch(&mut f.display, &mut f.state);
    f.locker.paint_surface(&surface, 800, 600);
    f.dispatch();
    let keyboard = f.state.seat.get_keyboard().unwrap();
    feed(
        &mut f.state,
        InputEvent::PointerMotionAbsolute {
            event: position(0.25, 0.25),
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent {
                pressed: true,
                ..TestEvent::default()
            },
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent::default(),
        },
    );
    assert_eq!(keyboard.current_focus(), Some(f.locker.surface(&f.surface)));
    feed(
        &mut f.state,
        InputEvent::PointerMotion {
            event: position(800.0, 0.0),
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent {
                pressed: true,
                ..TestEvent::default()
            },
        },
    );
    feed(
        &mut f.state,
        InputEvent::PointerButton {
            event: TestEvent::default(),
        },
    );
    assert_eq!(keyboard.current_focus(), Some(f.locker.surface(&surface)));
    f.state
        .set_keyboard_focus(Some(f.app.surface(&f.app_surface)));
    assert_eq!(keyboard.current_focus(), Some(f.locker.surface(&surface)));
    f.state.set_keyboard_focus(None);
    assert_eq!(keyboard.current_focus(), Some(f.locker.surface(&surface)));
    f.dispatch();
    assert!(f.app.events.pointer.is_empty());
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn the_locked_scene_keeps_the_composited_pointer_cursor_visible() {
    use smithay::backend::renderer::element::{Element, Kind};
    let egl = crate::test_support::EglTest::new();
    let mut f = InputFixture::new();
    let mut renderer = egl.renderer();
    let elements = crate::render::output_elements(
        &mut renderer,
        &mut f.state,
        &f.output,
        None,
        crate::render::CursorMode::Composited,
    );
    assert!(elements
        .iter()
        .any(|element| element.kind() == Kind::Cursor));
    let elements = crate::render::output_elements(
        &mut renderer,
        &mut f.state,
        &f.output,
        None,
        crate::render::CursorMode::Hidden,
    );
    assert!(!elements
        .iter()
        .any(|element| element.kind() == Kind::Cursor));
}
