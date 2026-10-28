use super::*;
use wayland_client::protocol::wl_subcompositor;

fn down(f: &mut Fixture, slot: u32, x: f64, y: f64) {
    f.event(InputEvent::TouchDown {
        event: TestEvent {
            slot,
            x: x / 800.0,
            y: y / 600.0,
            ..Default::default()
        },
    });
    frame(f);
}

fn motion(f: &mut Fixture, slot: u32, x: f64, y: f64) {
    f.event(InputEvent::TouchMotion {
        event: TestEvent {
            slot,
            x: x / 800.0,
            y: y / 600.0,
            ..Default::default()
        },
    });
    frame(f);
}

fn up(f: &mut Fixture, slot: u32) {
    f.event(InputEvent::TouchUp {
        event: TestEvent {
            slot,
            ..Default::default()
        },
    });
    frame(f);
}

fn frame(f: &mut Fixture) {
    f.event(InputEvent::TouchFrame {
        event: TestEvent::default(),
    });
}

fn assert_down(
    client: &TestClient,
    surface: &wl_surface::WlSurface,
    id: i32,
    expected: (f64, f64),
) {
    let (receiver, x, y) = client
        .events
        .touch
        .iter()
        .rev()
        .find_map(|event| {
            if let wl_touch::Event::Down {
                surface,
                id: actual,
                x,
                y,
                ..
            } = event
            {
                (*actual == id).then_some((surface, *x, *y))
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(receiver, surface);
    assert!((x - expected.0).abs() <= 1.0 / 256.0, "x: {x}");
    assert!((y - expected.1).abs() <= 1.0 / 256.0, "y: {y}");
}

fn popup(f: &mut Fixture, app: &mut App) -> Popup {
    f.focus(app);
    let serial = app.key(f, true);
    let popup = app.popup(&app.shell);
    app.request(f, &popup, serial);
    f.assert_focus(&app.client.surface(&popup.surface));
    popup
}

#[test]
fn outside_touch_dismisses_nested_popups_and_authorizes_the_new_recipient() {
    for dismissed in [false, true] {
        let mut f = Fixture::new();
        let mut owner = App::new(&mut f, 20);
        let mut other = App::new(&mut f, 440);
        let parent = popup(&mut f, &mut owner);
        let serial = f.state.active_popup_grab.as_ref().unwrap().serial();
        let child = owner.popup(&parent.shell);
        owner.request(&mut f, &child, serial);
        if dismissed {
            f.state
                .active_popup_grab
                .as_mut()
                .unwrap()
                .ungrab(PopupUngrabStrategy::All);
        }
        other.touch_down(&mut f, 0);
        owner.dispatch(&mut f);
        other.dispatch(&mut f);
        assert_eq!(owner.client.events.popups_dismissed, 2);
        assert!(f.state.active_popup_grab.is_none());
        assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
        assert!(!f.state.seat.get_pointer().unwrap().is_grabbed());
        f.assert_focus(&other.root());
        assert!(owner.client.events.touch.is_empty());
        assert_down(&other.client, &other.surface, 0, (20.0, 20.0));
        let serial = other.touch_serial();
        up(&mut f, 0);
        let menu = other.popup(&other.shell);
        other.request(&mut f, &menu, serial);
        f.assert_focus(&other.client.surface(&menu.surface));
    }
}

#[test]
fn background_touches_dismiss_popups_without_delivering_or_authorizing_a_contact() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    popup(&mut f, &mut app);
    down(&mut f, 0, 790.0, 590.0);
    motion(&mut f, 0, 100.0, 150.0);
    up(&mut f, 0);
    app.dispatch(&mut f);
    assert_eq!(app.client.events.popups_dismissed, 1);
    assert!(app.client.events.touch.is_empty());
    assert!(f.state.active_popup_grab.is_none());
    assert!(f
        .state
        .popup_input
        .presses
        .iter()
        .all(|press| press.kind != InputKind::Touch));
    f.assert_focus(&app.root());
}

#[test]
fn owner_events_reach_sibling_windows_and_popups_without_moving_keyboard_focus() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    let (sibling, _sibling_shell) = App::toplevel(&mut f, &mut app.client, 440);
    let menu = popup(&mut f, &mut app);
    menu.surface.commit();
    app.dispatch(&mut f);
    app.client.paint_surface(&menu.surface, 30, 30);
    app.dispatch(&mut f);
    let focus = app.client.surface(&menu.surface);
    let kind = f.state.popup_manager.find_popup(&focus).unwrap();
    let offset = smithay::desktop::get_popup_toplevel_coords(&kind);
    for (surface, (x, y)) in [
        (sibling, (460.0, 120.0)),
        (app.surface.clone(), (170.0, 250.0)),
        (
            menu.surface.clone(),
            (
                f64::from(app.origin.x + offset.x + 5),
                f64::from(app.origin.y + offset.y + 5),
            ),
        ),
    ] {
        assert_eq!(
            crate::input::surface_under(&f.state, (x, y).into())
                .unwrap()
                .0,
            app.client.surface(&surface)
        );
        down(&mut f, 0, x, y);
        up(&mut f, 0);
        app.dispatch(&mut f);
        assert!(app.client.events.touch.iter().any(|event| matches!(event,
            wl_touch::Event::Down { surface: receiver, .. } if receiver == &surface)));
        assert_eq!(app.client.events.popups_dismissed, 0);
        f.assert_focus(&focus);
    }
}

#[test]
fn implicit_contacts_keep_the_first_receiver_until_every_contact_is_released() {
    let mut f = Fixture::new();
    let mut owner = App::new(&mut f, 20);
    let mut other = App::new(&mut f, 440);
    owner.touch_down(&mut f, 0);
    owner.dispatch(&mut f);
    let serial = owner.touch_serial();
    let menu = owner.popup(&owner.shell);
    owner.request(&mut f, &menu, serial);
    let focus = owner.client.surface(&menu.surface);
    other.touch_down(&mut f, 1);
    motion(&mut f, 0, 500.0, 150.0);
    up(&mut f, 0);
    other.touch_down(&mut f, 2);
    owner.dispatch(&mut f);
    other.dispatch(&mut f);
    assert!(other.client.events.touch.is_empty());
    assert_eq!(owner.client.events.popups_dismissed, 0);
    f.assert_focus(&focus);
    assert_eq!(
        f.state
            .space
            .elements()
            .last()
            .unwrap()
            .wl_surface()
            .as_deref(),
        Some(&owner.root())
    );
    assert_down(&owner.client, &owner.surface, 2, (440.0, 20.0));
    assert!(owner.client.events.touch.iter().any(|event| matches!(event,
        wl_touch::Event::Motion { id: 0, x, y, .. }
        if (*x - 480.0).abs() <= 1.0 / 256.0 && (*y - 50.0).abs() <= 1.0 / 256.0)));
    up(&mut f, 1);
    up(&mut f, 2);
    other.touch_down(&mut f, 3);
    owner.dispatch(&mut f);
    other.dispatch(&mut f);
    assert_eq!(owner.client.events.popups_dismissed, 1);
    assert!(other
        .client
        .events
        .touch
        .iter()
        .any(|event| matches!(event, wl_touch::Event::Down { id: 3, .. })));
    f.assert_focus(&other.root());
}

#[test]
fn only_accepted_popup_grabs_cancel_foreign_touch_streams_after_completed_frames() {
    for accepted in [false, true] {
        let mut f = Fixture::new();
        let mut owner = App::new(&mut f, 20);
        let mut other = App::new(&mut f, 440);
        other.touch_down(&mut f, 0);
        other.touch_down(&mut f, 1);
        other.dispatch(&mut f);
        let menu = if accepted {
            popup(&mut f, &mut owner)
        } else {
            f.focus(&owner);
            let menu = owner.popup(&owner.shell);
            owner.request(&mut f, &menu, 0.into());
            menu
        };
        other.dispatch(&mut f);
        assert_eq!(
            other
                .client
                .events
                .touch
                .iter()
                .filter(|event| matches!(event, wl_touch::Event::Cancel))
                .count(),
            usize::from(accepted)
        );
        if accepted {
            assert!(f.state.touch_points.is_empty());
            assert!(!f.state.seat.get_touch().unwrap().is_grabbed());
        } else {
            assert_eq!(f.state.touch_points.len(), 2);
            assert!(f.state.seat.get_touch().unwrap().is_grabbed());
        }
        other.client.events.touch.clear();
        for slot in [0, 1] {
            motion(&mut f, slot, 500.0, 150.0);
            up(&mut f, slot);
        }
        other.dispatch(&mut f);
        assert_eq!(other.client.events.touch.is_empty(), accepted);
        if accepted {
            owner.touch_down(&mut f, 2);
            owner.dispatch(&mut f);
            f.assert_focus(&owner.client.surface(&menu.surface));
            assert!(owner
                .client
                .events
                .touch
                .iter()
                .any(|event| matches!(event, wl_touch::Event::Down { id: 2, .. })));
        }
    }
}

#[test]
fn popup_grabs_cannot_replace_explicit_touch_drags() {
    use smithay::wayland::selection::data_device::{start_dnd, ServerDnDGrab, SourceMetadata};
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    app.touch_down(&mut f, 0);
    app.dispatch(&mut f);
    let serial = app.touch_serial();
    let touch = f.state.seat.get_touch().unwrap();
    let display = f.state.display_handle.clone();
    let seat = f.state.seat.clone();
    let start = touch.grab_start_data().unwrap();
    start_dnd(
        &display,
        &seat,
        &mut f.state,
        serial,
        None,
        Some(start),
        SourceMetadata::default(),
    );
    let key = app.key(&mut f, true);
    for serial in [serial, key] {
        let menu = app.popup(&app.shell);
        app.request(&mut f, &menu, serial);
        assert_eq!(
            touch.with_grab(|_, grab| grab.is::<ServerDnDGrab<BlairState>>()),
            Some(true)
        );
        assert!(f.state.active_popup_grab.is_none());
        f.assert_focus(&app.root());
    }
    assert_eq!(app.client.events.popups_dismissed, 2);
    assert!(!app
        .client
        .events
        .touch
        .iter()
        .any(|event| matches!(event, wl_touch::Event::Cancel)));
}

#[test]
fn popup_grabs_clear_touch_sequences_that_started_without_a_surface() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    down(&mut f, 0, 790.0, 590.0);
    assert!(f.state.seat.get_touch().unwrap().is_grabbed());
    let menu = popup(&mut f, &mut app);
    assert!(f.state.touch_points.is_empty());
    assert!(!f.state.seat.get_touch().unwrap().is_grabbed());
    app.touch_down(&mut f, 1);
    app.dispatch(&mut f);
    assert_down(&app.client, &app.surface, 1, (20.0, 20.0));
    assert_eq!(app.client.events.popups_dismissed, 0);
    f.assert_focus(&app.client.surface(&menu.surface));
}

#[test]
fn duplicate_touch_downs_do_not_leave_an_implicit_grab_active() {
    let mut f = Fixture::new();
    let mut owner = App::new(&mut f, 20);
    let mut other = App::new(&mut f, 440);
    popup(&mut f, &mut owner);
    for _ in 0..2 {
        owner.touch_down(&mut f, 0);
    }
    owner.dispatch(&mut f);
    assert_eq!(
        owner
            .client
            .events
            .touch
            .iter()
            .filter(|event| matches!(event, wl_touch::Event::Down { .. }))
            .count(),
        1
    );
    up(&mut f, 0);
    assert!(!f.state.seat.get_touch().unwrap().is_grabbed());
    other.touch_down(&mut f, 1);
    owner.dispatch(&mut f);
    other.dispatch(&mut f);
    assert_eq!(owner.client.events.popups_dismissed, 1);
    assert_down(&other.client, &other.surface, 1, (20.0, 20.0));
    f.assert_focus(&other.root());
}

#[test]
fn dead_implicit_receivers_cannot_bypass_outside_touch_dismissal() {
    let mut f = Fixture::new();
    let mut owner = App::new(&mut f, 20);
    let mut other = App::new(&mut f, 440);
    let child = owner
        .client
        .bind::<wl_compositor::WlCompositor>(6)
        .create_surface(&owner.client.handle(), ());
    let _subsurface = owner
        .client
        .bind::<wl_subcompositor::WlSubcompositor>(1)
        .get_subsurface(&child, &owner.surface, &owner.client.handle(), ());
    owner.client.paint_surface(&child, 30, 30);
    owner.surface.commit();
    owner.dispatch(&mut f);
    popup(&mut f, &mut owner);
    owner.touch_down(&mut f, 0);
    owner.dispatch(&mut f);
    assert_eq!(
        f.state
            .seat
            .get_touch()
            .unwrap()
            .grab_start_data()
            .unwrap()
            .focus
            .unwrap()
            .0,
        owner.client.surface(&child)
    );
    child.destroy();
    owner.dispatch(&mut f);
    other.touch_down(&mut f, 1);
    owner.dispatch(&mut f);
    other.dispatch(&mut f);
    assert_eq!(owner.client.events.popups_dismissed, 1);
    assert_eq!(f.state.touch_points.len(), 1);
    assert!(f.state.active_popup_grab.is_none());
    f.assert_focus(&other.root());
    assert!(other
        .client
        .events
        .touch
        .iter()
        .any(|event| matches!(event, wl_touch::Event::Down { id: 1, .. })));
}
