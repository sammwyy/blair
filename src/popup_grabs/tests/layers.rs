use super::*;
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

struct ShellLayer {
    client: TestClient,
    seat: wl_seat::WlSeat,
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    _keyboard: wl_keyboard::WlKeyboard,
}

impl ShellLayer {
    fn new(
        f: &mut Fixture,
        depth: zwlr_layer_shell_v1::Layer,
        interactivity: zwlr_layer_surface_v1::KeyboardInteractivity,
    ) -> Self {
        let mut client = TestClient::new(&mut f.display, &mut f.state);
        let seat = client.bind::<wl_seat::WlSeat>(7);
        let keyboard = seat.get_keyboard(&client.handle(), ());
        let (surface, layer) = Self::create(&client, depth, interactivity);
        surface.commit();
        client.dispatch(&mut f.display, &mut f.state);
        Self {
            client,
            seat,
            surface,
            layer,
            _keyboard: keyboard,
        }
    }

    fn create(
        client: &TestClient,
        depth: zwlr_layer_shell_v1::Layer,
        interactivity: zwlr_layer_surface_v1::KeyboardInteractivity,
    ) -> (
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    ) {
        let surface = client
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&client.handle(), ());
        let layer = client
            .bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1>(4)
            .get_layer_surface(&surface, None, depth, "test".into(), &client.handle(), ());
        layer.set_size(800, 30);
        layer.set_keyboard_interactivity(interactivity);
        (surface, layer)
    }

    fn dispatch(&mut self, f: &mut Fixture) {
        self.client.dispatch(&mut f.display, &mut f.state);
    }

    fn map(&mut self, f: &mut Fixture) {
        self.client.paint_surface(&self.surface, 800, 30);
        self.dispatch(f);
    }

    fn root(&self) -> WlSurface {
        self.client.surface(&self.surface)
    }

    fn popup(&mut self, f: &mut Fixture) -> Popup {
        crate::input::keyboard_transition(&mut f.state, 38, true, 1, &mut Hooks);
        self.dispatch(f);
        let serial = *self.client.events.key_serials.last().unwrap();
        let popup = Popup::new(&self.client, None);
        self.layer.get_popup(&popup.popup);
        popup.popup.grab(&self.seat, serial);
        self.dispatch(f);
        popup
    }
}

#[test]
fn pointer_input_cannot_take_focus_from_an_exclusive_layer() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    let mut shell = ShellLayer::new(
        &mut f,
        zwlr_layer_shell_v1::Layer::Overlay,
        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
    );
    shell.map(&mut f);
    let focus = shell.root();
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
fn exclusive_layer_mapping_dismisses_nested_popups_and_receives_keys() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    f.focus(&app);
    let serial = app.key(&mut f, true);
    let popup = app.popup(&app.shell);
    app.request(&mut f, &popup, serial);
    let child = app.popup(&popup.shell);
    app.request(&mut f, &child, serial);
    let mut shell = ShellLayer::new(
        &mut f,
        zwlr_layer_shell_v1::Layer::Overlay,
        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
    );
    f.assert_focus(&app.client.surface(&child.surface));
    shell.map(&mut f);
    f.assert_focus(&shell.root());
    app.dispatch(&mut f);
    assert_eq!(app.client.events.popups_dismissed, 2);
    assert!(f.state.active_popup_grab.is_none());
    assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
    assert!(!f.state.seat.get_pointer().unwrap().is_grabbed());
    let keys = app.client.events.keys.len();
    crate::input::keyboard_transition(&mut f.state, 38, false, 2, &mut Hooks);
    crate::input::keyboard_transition(&mut f.state, 39, true, 3, &mut Hooks);
    app.dispatch(&mut f);
    shell.dispatch(&mut f);
    assert_eq!(app.client.events.keys.len(), keys);
    assert_eq!(shell.client.events.keys.len(), 2);
    assert!(PopupManager::popups_for_surface(&app.root())
        .next()
        .is_none());
}

#[test]
fn exclusive_layer_popups_survive_commits_and_close_when_the_layer_unmaps() {
    for committed in [false, true] {
        let mut f = Fixture::new();
        let app = App::new(&mut f, 20);
        f.focus(&app);
        let mut shell = ShellLayer::new(
            &mut f,
            zwlr_layer_shell_v1::Layer::Overlay,
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
        );
        shell.map(&mut f);
        let popup = shell.popup(&mut f);
        if committed {
            popup.surface.commit();
            shell.dispatch(&mut f);
        }
        let focus = shell.client.surface(&popup.surface);
        f.assert_focus(&focus);
        shell.surface.commit();
        shell.dispatch(&mut f);
        f.state.refresh();
        f.assert_focus(&focus);
        assert_eq!(shell.client.events.popups_dismissed, 0);
        shell.surface.attach(None, 0, 0);
        shell.surface.commit();
        shell.dispatch(&mut f);
        f.assert_focus(&app.root());
        assert_eq!(shell.client.events.popups_dismissed, 1);
        assert!(f.state.active_popup_grab.is_none());
    }
}

#[test]
fn lower_priority_layers_preserve_the_exclusive_layers_own_popup() {
    let mut f = Fixture::new();
    let mut shell = ShellLayer::new(
        &mut f,
        zwlr_layer_shell_v1::Layer::Overlay,
        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
    );
    shell.map(&mut f);
    let popup = shell.popup(&mut f);
    let focus = shell.client.surface(&popup.surface);
    for (depth, interactivity) in [
        (
            zwlr_layer_shell_v1::Layer::Top,
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
        ),
        (
            zwlr_layer_shell_v1::Layer::Overlay,
            zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
        ),
        (
            zwlr_layer_shell_v1::Layer::Overlay,
            zwlr_layer_surface_v1::KeyboardInteractivity::None,
        ),
    ] {
        let mut other = ShellLayer::new(&mut f, depth, interactivity);
        other.map(&mut f);
        shell.dispatch(&mut f);
        f.assert_focus(&focus);
        assert_eq!(shell.client.events.popups_dismissed, 0);
        assert!(f.state.active_popup_grab.is_some());
    }
}

#[test]
fn exclusive_layer_cannot_redirect_keys_to_an_unrelated_window_of_the_same_client() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    let (surface, _layer) = ShellLayer::create(
        &app.client,
        zwlr_layer_shell_v1::Layer::Overlay,
        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
    );
    surface.commit();
    app.dispatch(&mut f);
    app.client.paint_surface(&surface, 800, 30);
    app.dispatch(&mut f);
    let focus = app.client.surface(&surface);
    f.assert_focus(&focus);
    let serial = app.key(&mut f, true);
    let popup = app.popup(&app.shell);
    app.request(&mut f, &popup, serial);
    f.assert_focus(&focus);
    assert_eq!(app.client.events.popups_dismissed, 1);
    assert!(f.state.active_popup_grab.is_none());
}

#[test]
fn exclusive_layer_priority_follows_depth_and_mapping() {
    let mut f = Fixture::new();
    let app = App::new(&mut f, 20);
    f.focus(&app);
    let mut top = ShellLayer::new(
        &mut f,
        zwlr_layer_shell_v1::Layer::Top,
        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
    );
    top.surface.commit();
    top.dispatch(&mut f);
    f.focus(&app);
    f.assert_focus(&app.root());
    top.map(&mut f);
    f.assert_focus(&top.root());
    let mut overlay = ShellLayer::new(
        &mut f,
        zwlr_layer_shell_v1::Layer::Overlay,
        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
    );
    overlay.map(&mut f);
    f.assert_focus(&overlay.root());
    overlay.surface.attach(None, 0, 0);
    overlay.surface.commit();
    overlay.dispatch(&mut f);
    f.assert_focus(&top.root());
    top.layer
        .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    top.surface.commit();
    top.dispatch(&mut f);
    f.assert_focus(&app.root());
}

#[test]
fn higher_exclusive_layers_dismiss_lower_layer_menus_after_mapping_or_policy_changes() {
    for changed in [false, true] {
        let mut f = Fixture::new();
        let mut top = ShellLayer::new(
            &mut f,
            zwlr_layer_shell_v1::Layer::Top,
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
        );
        top.map(&mut f);
        let popup = top.popup(&mut f);
        let mut overlay = ShellLayer::new(
            &mut f,
            zwlr_layer_shell_v1::Layer::Overlay,
            if changed {
                zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand
            } else {
                zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive
            },
        );
        overlay.map(&mut f);
        if changed {
            f.assert_focus(&top.client.surface(&popup.surface));
            overlay.layer.set_keyboard_interactivity(
                zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
            );
            overlay.surface.commit();
            overlay.dispatch(&mut f);
        }
        top.dispatch(&mut f);
        f.assert_focus(&overlay.root());
        assert_eq!(top.client.events.popups_dismissed, 1);
        assert!(f.state.active_popup_grab.is_none());
        assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
        assert!(!f.state.seat.get_pointer().unwrap().is_grabbed());
    }
}

#[test]
fn disabling_or_destroying_a_layer_dismisses_its_menu_and_restores_window_focus() {
    for destroyed in [false, true] {
        let mut f = Fixture::new();
        let app = App::new(&mut f, 20);
        f.focus(&app);
        let mut shell = ShellLayer::new(
            &mut f,
            zwlr_layer_shell_v1::Layer::Overlay,
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
        );
        shell.map(&mut f);
        shell.popup(&mut f);
        if destroyed {
            shell.layer.destroy();
        } else {
            shell
                .layer
                .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
            shell.surface.commit();
        }
        shell.dispatch(&mut f);
        f.assert_focus(&app.root());
        assert_eq!(shell.client.events.popups_dismissed, 1);
        assert!(f.state.active_popup_grab.is_none());
        assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
        assert!(!f.state.seat.get_pointer().unwrap().is_grabbed());
    }
}

#[test]
fn outside_touch_dismisses_an_exclusive_layer_menu_without_taking_keyboard_focus() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    let mut shell = ShellLayer::new(
        &mut f,
        zwlr_layer_shell_v1::Layer::Overlay,
        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
    );
    shell.map(&mut f);
    shell.popup(&mut f);
    app.touch_down(&mut f, 0);
    shell.dispatch(&mut f);
    app.dispatch(&mut f);
    f.assert_focus(&shell.root());
    assert_eq!(shell.client.events.popups_dismissed, 1);
    assert!(f.state.active_popup_grab.is_none());
    let serial = app.touch_serial();
    let rejected = app.popup(&app.shell);
    app.request(&mut f, &rejected, serial);
    assert_eq!(app.client.events.popups_dismissed, 1);
    assert!(!f.state.seat.get_keyboard().unwrap().is_grabbed());
    f.assert_focus(&shell.root());
}

#[test]
fn outside_touch_dismisses_window_menus_and_focuses_an_on_demand_layer() {
    let mut f = Fixture::new();
    let mut app = App::new(&mut f, 20);
    f.focus(&app);
    let serial = app.key(&mut f, true);
    let menu = app.popup(&app.shell);
    app.request(&mut f, &menu, serial);
    let mut shell = ShellLayer::new(
        &mut f,
        zwlr_layer_shell_v1::Layer::Overlay,
        zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
    );
    let _touch = shell.seat.get_touch(&shell.client.handle(), ());
    shell.map(&mut f);
    let origin = crate::input::surface_origin(&f.state, &shell.root()).unwrap();
    f.event(InputEvent::TouchDown {
        event: TestEvent {
            x: (origin.x + 10.0) / 800.0,
            y: (origin.y + 10.0) / 600.0,
            ..Default::default()
        },
    });
    f.event(InputEvent::TouchFrame {
        event: TestEvent::default(),
    });
    shell.dispatch(&mut f);
    app.dispatch(&mut f);
    assert_eq!(app.client.events.popups_dismissed, 1);
    assert!(f.state.active_popup_grab.is_none());
    f.assert_focus(&shell.root());
    assert!(shell.client.events.touch.iter().any(|event| matches!(event,
        wl_touch::Event::Down { surface, .. } if surface == &shell.surface)));
}
