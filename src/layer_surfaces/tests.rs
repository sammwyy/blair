use super::*;
use smithay::{
    backend::input::InputEvent,
    reexports::{calloop::EventLoop, wayland_server::Display},
    wayland::shell::wlr_layer::{Anchor, ExclusiveZone},
};
use wayland_client::{
    protocol::{wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_seat, wl_surface, wl_touch},
    Proxy,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::test_support::{
    input::{TestEvent, TestInputBackend},
    TestClient, TestPopup,
};

mod layout;

struct Hooks;
impl crate::input::InputHooks for Hooks {}

struct Fixture {
    display: Display<BlairState>,
    _event_loop: EventLoop<'static, BlairState>,
    state: BlairState,
    client: TestClient,
    output: Output,
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    buffer: wl_buffer::WlBuffer,
    viewport: wp_viewport::WpViewport,
    _keyboard: wl_keyboard::WlKeyboard,
    _pointer: wl_pointer::WlPointer,
    _touch: wl_touch::WlTouch,
}

impl Fixture {
    fn new() -> Self {
        let (mut display, event_loop, mut state) = crate::state::tests::fixture();
        state.seat.add_touch();
        let output = crate::test_support::add_output(&mut state, "test", (0, 0), (800, 600));
        let mut client = TestClient::new(&mut display, &mut state);
        let seat = client.bind::<wl_seat::WlSeat>(7);
        let keyboard = seat.get_keyboard(&client.handle(), ());
        let pointer = seat.get_pointer(&client.handle(), ());
        let touch = seat.get_touch(&client.handle(), ());
        let surface = client
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&client.handle(), ());
        let layer = client
            .bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1>(4)
            .get_layer_surface(
                &surface,
                None,
                zwlr_layer_shell_v1::Layer::Top,
                "panel".into(),
                &client.handle(),
                (),
            );
        let viewport = client.bind::<wp_viewporter::WpViewporter>(1).get_viewport(
            &surface,
            &client.handle(),
            (),
        );
        let buffer = client.solid_buffer();
        client.dispatch(&mut display, &mut state);
        Self {
            display,
            _event_loop: event_loop,
            state,
            client,
            output,
            surface,
            layer,
            buffer,
            viewport,
            _keyboard: keyboard,
            _pointer: pointer,
            _touch: touch,
        }
    }

    fn dispatch(&mut self) {
        self.client.dispatch(&mut self.display, &mut self.state);
    }

    fn settings(&self) {
        self.layer.set_size(0, 30);
        self.layer.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        self.layer.set_exclusive_zone(30);
        self.layer
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    }

    fn configure(&mut self) {
        self.settings();
        self.surface.commit();
        self.dispatch();
    }

    fn attach(&self) {
        let (_, width, height) = self
            .client
            .events
            .layer_configures
            .last()
            .copied()
            .unwrap_or((0, 800, 30));
        self.viewport
            .set_destination(width.max(1) as i32, height.max(1) as i32);
        self.surface.attach(Some(&self.buffer), 0, 0);
        self.surface.damage_buffer(0, 0, 1, 1);
        self.surface.commit();
    }

    fn map(&mut self) {
        self.attach();
        self.dispatch();
    }

    fn unmap(&mut self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        self.dispatch();
    }

    fn root(&self) -> WlSurface {
        self.client.surface(&self.surface)
    }

    fn area(&self) -> Rectangle<i32, Logical> {
        layer_map_for_output(&self.output).non_exclusive_zone()
    }

    fn event(&mut self, event: InputEvent<TestInputBackend>) {
        crate::input::process_input_event(&mut self.state, event, &mut Hooks);
    }
}

#[test]
fn layers_reserve_space_and_receive_input_only_after_mapping() {
    let mut f = Fixture::new();
    assert!(f.client.events.layer_configures.is_empty());
    f.configure();
    assert_eq!(f.client.events.layer_configures.len(), 1);
    assert_eq!(f.area(), Rectangle::from_size((800, 600).into()));
    assert_eq!(layer_map_for_output(&f.output).len(), 0);
    assert!(crate::input::surface_under(&f.state, (10.0, 10.0).into()).is_none());
    f.map();
    assert_eq!(f.area(), Rectangle::new((0, 30).into(), (800, 570).into()));
    assert_eq!(
        crate::input::surface_under(&f.state, (10.0, 10.0).into())
            .unwrap()
            .0,
        f.root()
    );
    let configures = f.client.events.layer_configures.len();
    f.unmap();
    assert_eq!(f.area(), Rectangle::from_size((800, 600).into()));
    assert_eq!(layer_map_for_output(&f.output).len(), 0);
    assert!(crate::input::surface_under(&f.state, (10.0, 10.0).into()).is_none());
    assert_eq!(f.client.events.layer_configures.len(), configures);
}

#[test]
fn duplicate_layer_roles_are_rejected_before_initial_mapping() {
    let mut f = Fixture::new();
    f.client
        .bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1>(4)
        .get_layer_surface(
            &f.surface,
            None,
            zwlr_layer_shell_v1::Layer::Top,
            "duplicate".into(),
            &f.client.handle(),
            (),
        );
    let error = f.client.expect_protocol_error(&mut f.display, &mut f.state);
    assert_eq!(error.object_interface, "zwlr_layer_surface_v1");
    assert_eq!(
        error.code,
        zwlr_layer_surface_v1::Error::InvalidSurfaceState as u32
    );
    assert_eq!(layer_map_for_output(&f.output).len(), 0);
}

#[test]
fn remapping_resets_role_state_and_requires_a_fresh_configure() {
    let mut f = Fixture::new();
    f.configure();
    f.map();
    f.layer.set_layer(zwlr_layer_shell_v1::Layer::Overlay);
    f.layer.set_margin(2, 3, 4, 5);
    f.surface.commit();
    f.dispatch();
    f.unmap();
    let layer = &f.state.layer_surfaces[0];
    let data = layer.cached_state();
    assert_eq!(data.layer, Layer::Top);
    assert_eq!(data.size, (0, 0).into());
    assert_eq!(data.anchor, Anchor::empty());
    assert_eq!(data.exclusive_zone, ExclusiveZone::Neutral);
    assert_eq!(
        data.keyboard_interactivity,
        smithay::wayland::shell::wlr_layer::KeyboardInteractivity::None
    );
    assert_eq!(
        (
            data.margin.top,
            data.margin.right,
            data.margin.bottom,
            data.margin.left
        ),
        (0, 0, 0, 0)
    );
    with_states(&f.root(), |states| {
        let data = states
            .data_map
            .get::<LayerSurfaceData>()
            .unwrap()
            .lock()
            .unwrap();
        assert!(!data.configured);
        assert!(!data.initial_configure_sent);
        assert!(data.configure_serial.is_none());
        assert!(data.last_acked.is_none());
    });
    assert_eq!(f.state.output_for_layer(&f.root()), Some(f.output.clone()));
    let old = f.client.events.layer_configures.last().unwrap().0;
    f.configure();
    assert_ne!(f.client.events.layer_configures.last().unwrap().0, old);
    f.map();
    assert_eq!(f.area(), Rectangle::new((0, 30).into(), (800, 570).into()));
    assert!(is_mapped(&f.state.layer_surfaces[0]));
}

#[test]
fn attaching_buffers_before_initial_or_remap_configure_is_a_protocol_error() {
    for remap in [false, true] {
        let mut f = Fixture::new();
        if remap {
            f.configure();
            f.map();
            f.unmap();
        }
        f.settings();
        f.attach();
        let error = f.client.expect_protocol_error(&mut f.display, &mut f.state);
        assert_eq!(error.object_interface, "zwlr_layer_surface_v1");
        assert_eq!(
            error.code,
            zwlr_layer_surface_v1::Error::InvalidSurfaceState as u32
        );
        assert_eq!(layer_map_for_output(&f.output).len(), 0);
        assert_eq!(f.area(), Rectangle::from_size((800, 600).into()));
    }
}

#[test]
fn a_configure_must_be_acknowledged_before_mapping() {
    for remap in [false, true] {
        let mut f = Fixture::new();
        if remap {
            f.configure();
            f.map();
            f.unmap();
        }
        f.client.events.hold_layer_configures = true;
        f.configure();
        f.attach();
        let error = f.client.expect_protocol_error(&mut f.display, &mut f.state);
        assert_eq!(
            error.code,
            zwlr_layer_surface_v1::Error::InvalidSurfaceState as u32
        );
        assert_eq!(layer_map_for_output(&f.output).len(), 0);
    }
}

#[test]
fn stale_configures_cannot_authorize_another_mapping_cycle() {
    for configured in [false, true] {
        let mut f = Fixture::new();
        f.configure();
        f.map();
        f.client.events.hold_layer_configures = true;
        f.layer.set_size(0, 40);
        f.surface.commit();
        f.dispatch();
        let stale = f.client.events.layer_configures.last().unwrap().0;
        f.unmap();
        if configured {
            f.configure();
        }
        f.layer.ack_configure(stale);
        let error = f.client.expect_protocol_error(&mut f.display, &mut f.state);
        assert_eq!(error.object_interface, "zwlr_layer_surface_v1");
        assert_eq!(
            error.code,
            zwlr_layer_surface_v1::Error::InvalidSurfaceState as u32
        );
        assert_eq!(layer_map_for_output(&f.output).len(), 0);
    }
}

#[test]
fn acknowledged_cycles_accept_later_validated_configures_across_serial_wrap() {
    for mapped in [false, true] {
        let mut f = Fixture::new();
        f.configure();
        f.dispatch();
        if mapped {
            f.map();
        }
        let first = f.client.events.layer_configures[0].0;
        f.state.layer_configure_acknowledged(
            &f.root(),
            LayerSurfaceConfigure {
                serial: first.wrapping_add(0x8000_0001).into(),
                state: smithay::wayland::shell::wlr_layer::LayerSurfaceState {
                    size: Some((800, 30).into()),
                },
            },
        );
        f.dispatch();
        if !mapped {
            f.map();
        }
        assert!(is_mapped(&f.state.layer_surfaces[0]));
        assert_eq!(f.area(), Rectangle::new((0, 30).into(), (800, 570).into()));
    }
}

#[test]
fn non_grabbing_child_popups_are_dismissed_in_reverse_order_and_do_not_reappear() {
    let mut f = Fixture::new();
    f.configure();
    f.map();
    let parent = TestPopup::new(&f.client, None);
    f.layer.get_popup(&parent.popup);
    parent.surface.commit();
    f.dispatch();
    f.client.paint_surface(&parent.surface, 30, 30);
    f.dispatch();
    let child = TestPopup::new(&f.client, Some(&parent.shell));
    child.surface.commit();
    f.dispatch();
    f.client.paint_surface(&child.surface, 30, 30);
    f.dispatch();
    f.unmap();
    assert_eq!(
        f.client.events.popup_done,
        [
            child.popup.id().protocol_id(),
            parent.popup.id().protocol_id()
        ]
    );
    assert!(PopupManager::popups_for_surface(&f.root()).next().is_none());
    f.configure();
    f.map();
    assert!(f
        .state
        .popup_manager
        .find_popup(&f.client.surface(&parent.surface))
        .is_none());
    assert!(f
        .state
        .popup_manager
        .find_popup(&f.client.surface(&child.surface))
        .is_none());
}

#[test]
fn unmapping_cancels_contacts_and_releases_pointer_and_keyboard_focus() {
    let mut f = Fixture::new();
    f.configure();
    f.map();
    let event = TestEvent {
        x: 0.0125,
        y: 1.0 / 60.0,
        ..Default::default()
    };
    f.event(InputEvent::TouchDown { event });
    f.event(InputEvent::TouchFrame { event });
    f.event(InputEvent::PointerMotionAbsolute { event });
    f.event(InputEvent::PointerButton {
        event: TestEvent {
            pressed: true,
            ..event
        },
    });
    f.dispatch();
    assert!(f.state.seat.get_touch().unwrap().is_grabbed());
    assert!(f.state.seat.get_pointer().unwrap().is_grabbed());
    f.unmap();
    assert!(f
        .state
        .seat
        .get_keyboard()
        .unwrap()
        .current_focus()
        .is_none());
    assert!(f
        .state
        .seat
        .get_pointer()
        .unwrap()
        .current_focus()
        .is_none());
    assert!(!f.state.seat.get_pointer().unwrap().is_grabbed());
    assert!(!f.state.seat.get_touch().unwrap().is_grabbed());
    assert!(f.state.touch_points.is_empty());
    assert_eq!(
        f.client
            .events
            .touch
            .iter()
            .filter(|event| matches!(event, wl_touch::Event::Cancel))
            .count(),
        1
    );
}

#[test]
fn unmapped_configures_follow_output_changes_without_reserving_space() {
    let mut f = Fixture::new();
    f.client.events.hold_layer_configures = true;
    f.configure();
    f.output.change_current_state(
        Some(smithay::output::Mode {
            size: (640, 480).into(),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    f.state.output_changed(&f.output);
    f.dispatch();
    let (serial, width, height) = *f.client.events.layer_configures.last().unwrap();
    assert_eq!((width, height), (640, 30));
    assert_eq!(f.area(), Rectangle::from_size((640, 480).into()));
    f.layer.ack_configure(serial);
    f.map();
    assert_eq!(f.area(), Rectangle::new((0, 30).into(), (640, 450).into()));
}

#[test]
fn repeated_mapping_cycles_reuse_the_output_and_do_not_duplicate_configures() {
    let mut f = Fixture::new();
    let other = crate::test_support::add_output(&mut f.state, "other", (800, 0), (800, 600));
    f.state.set_focused_output_at((900.0, 100.0).into());
    assert_eq!(f.state.focused_output(), Some(other.clone()));
    for cycle in 1..=16 {
        f.configure();
        f.map();
        assert_eq!(f.client.events.layer_configures.len(), cycle);
        assert_eq!(f.state.output_for_layer(&f.root()), Some(f.output.clone()));
        assert_eq!(f.area(), Rectangle::new((0, 30).into(), (800, 570).into()));
        f.unmap();
        assert_eq!(f.client.events.layer_configures.len(), cycle);
        assert_eq!(f.area(), Rectangle::from_size((800, 600).into()));
        assert_eq!(
            layer_map_for_output(&other).non_exclusive_zone(),
            Rectangle::from_size((800, 600).into())
        );
    }
    f.layer.destroy();
    f.dispatch();
    assert!(f.state.layer_surfaces.is_empty());
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn layer_mapping_cycles_change_only_the_mapped_scene() {
    use smithay::{
        backend::{
            allocator::Fourcc,
            renderer::{
                damage::OutputDamageTracker,
                gles::{GlesRenderer, GlesTexture},
                Bind, Color32F, ExportMem, Offscreen,
            },
        },
        utils::Transform,
    };
    fn pixels(f: &mut Fixture, renderer: &mut GlesRenderer) -> Vec<u8> {
        let size = (800, 600).into();
        let elements = crate::render::output_elements(
            renderer,
            &mut f.state,
            &f.output,
            None,
            crate::render::CursorMode::Hidden,
        );
        let mut texture =
            Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size).unwrap();
        let mut target = renderer.bind(&mut texture).unwrap();
        OutputDamageTracker::new((800, 600), 1.0, Transform::Normal)
            .render_output(renderer, &mut target, 0, &elements, Color32F::BLACK)
            .unwrap();
        let mapping = renderer
            .copy_framebuffer(&target, Rectangle::from_size(size), Fourcc::Abgr8888)
            .unwrap();
        renderer.map_texture(&mapping).unwrap().to_vec()
    }
    let egl = crate::test_support::EglTest::new();
    let mut f = Fixture::new();
    let mut renderer = egl.renderer();
    let empty = pixels(&mut f, &mut renderer);
    f.configure();
    assert_eq!(pixels(&mut f, &mut renderer), empty);
    f.map();
    let mapped = pixels(&mut f, &mut renderer);
    assert_ne!(mapped, empty);
    f.unmap();
    assert_eq!(pixels(&mut f, &mut renderer), empty);
    f.configure();
    f.map();
    assert_eq!(pixels(&mut f, &mut renderer), mapped);
}

#[test]
fn work_area_notifications_follow_mapping_unmapping_and_empty_output_resizes() {
    use std::sync::{Arc, Mutex};
    let mut f = Fixture::new();
    let events = Arc::new(crate::test_support::TestEvents(Mutex::new(Vec::new())));
    f.state.events = events.clone();
    f.configure();
    assert!(!events.0.lock().unwrap().iter().any(|event| matches!(
        event,
        blair_protocol::CompositorEvent::WorkAreaChanged { .. }
    )));
    f.map();
    f.unmap();
    f.output.change_current_state(
        Some(smithay::output::Mode {
            size: (640, 480).into(),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    f.state.output_changed(&f.output);
    let areas: Vec<_> = events
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| {
            if let blair_protocol::CompositorEvent::WorkAreaChanged { area, .. } = event {
                Some((area.x, area.y, area.width, area.height))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        areas,
        [(0, 30, 800, 570), (0, 0, 800, 600), (0, 0, 640, 480)]
    );
}
