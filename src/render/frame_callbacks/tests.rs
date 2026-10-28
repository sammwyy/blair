use super::*;

use smithay::{
    backend::renderer::element::{
        default_primary_scanout_output_compare, Id, RenderElementPresentationState,
        RenderElementState, RenderElementStates,
    },
    desktop::utils::update_surface_primary_scanout_output,
    reexports::{calloop::EventLoop, wayland_server::Display},
    wayland::compositor::with_states,
};
use wayland_client::protocol::{wl_compositor, wl_subcompositor, wl_surface};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::test_support::{TestClient, TestPopup};

struct Fixture {
    display: Display<BlairState>,
    event_loop: EventLoop<'static, BlairState>,
    state: BlairState,
    client: TestClient,
    surface: wl_surface::WlSurface,
    shell: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    output: Output,
}

impl Fixture {
    fn new() -> Self {
        let (mut display, event_loop, mut state) = crate::state::tests::fixture();
        let output = crate::test_support::add_output(&mut state, "test", (0, 0), (800, 600));
        let mut client = TestClient::new(&mut display, &mut state);
        let surface = client
            .bind::<wl_compositor::WlCompositor>(6)
            .create_surface(&client.handle(), ());
        let shell = client.bind::<xdg_wm_base::XdgWmBase>(6).get_xdg_surface(
            &surface,
            &client.handle(),
            (),
        );
        let toplevel = shell.get_toplevel(&client.handle(), ());
        surface.commit();
        client.dispatch(&mut display, &mut state);
        Self {
            display,
            event_loop,
            state,
            client,
            surface,
            shell,
            toplevel,
            output,
        }
    }

    fn dispatch(&mut self) {
        self.client.dispatch(&mut self.display, &mut self.state);
    }

    fn request(&mut self, label: u32) {
        self.surface.frame(&self.client.handle(), label);
        self.surface.commit();
        self.dispatch();
    }

    fn visible(&self, surface: &wl_surface::WlSurface, output: &Output, visible: bool) {
        let surface = self.client.surface(surface);
        let mut rendered = RenderElementStates::default();
        if visible {
            rendered.states.insert(
                Id::from_wayland_resource(&surface),
                RenderElementState {
                    visible_area: 100,
                    presentation_state: RenderElementPresentationState::Rendering { reason: None },
                },
            );
        }
        with_states(&surface, |states| {
            update_surface_primary_scanout_output(
                &surface,
                output,
                states,
                &rendered,
                default_primary_scanout_output_compare,
            );
        });
    }

    fn expire(&mut self, surfaces: &[wl_surface::WlSurface]) {
        for surface in surfaces {
            with_states(&self.client.surface(surface), |states| {
                states
                    .data_map
                    .get::<CallbackDeadline>()
                    .unwrap()
                    .0
                    .set(Instant::now());
            });
        }
        self.state.schedule_hidden_frames();
        self.event_loop
            .dispatch(Duration::ZERO, &mut self.state)
            .unwrap();
        self.dispatch();
    }
}

#[test]
fn hidden_clients_receive_frames_without_repaints_or_repeated_wakeups() {
    let mut f = Fixture::new();
    f.request(1);
    f.state.take_redraw_request();
    f.state.refresh();
    let (token, deadline) = f.state.hidden_frames.timer.unwrap();
    for _ in 0..100 {
        f.state.refresh();
        assert_eq!(f.state.hidden_frames.timer, Some((token, deadline)));
    }
    assert!(f.client.events.frames.is_empty());
    while f.state.hidden_frames.timer.is_some() {
        assert!(Instant::now() < deadline + Duration::from_secs(1));
        f.event_loop
            .dispatch(
                deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(10),
                &mut f.state,
            )
            .unwrap();
    }
    f.dispatch();
    assert_eq!(f.client.events.frames.len(), 1);
    assert_eq!(f.client.events.frames[0].0, 1);
    assert!(!f.state.take_redraw_request());
    assert!(f.state.hidden_frames.timer.is_none());
    assert!(f.state.loop_handle.update(&token).is_err());

    f.request(2);
    f.state.schedule_hidden_frames();
    let (_, next) = f.state.hidden_frames.timer.unwrap();
    assert!(next > deadline);
    f.event_loop.dispatch(Duration::ZERO, &mut f.state).unwrap();
    f.dispatch();
    assert_eq!(f.client.events.frames.len(), 1);
    f.expire(&[f.surface.clone()]);
    assert_eq!(
        f.client
            .events
            .frames
            .iter()
            .map(|(label, _)| *label)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(f.state.hidden_frames.timer.is_none());
}

#[test]
fn visible_callbacks_stay_on_the_primary_output_and_cancel_hidden_timers() {
    let mut f = Fixture::new();
    let other = crate::test_support::add_output(&mut f.state, "other", (800, 0), (800, 600));
    f.request(1);
    f.state.schedule_hidden_frames();
    let (token, _) = f.state.hidden_frames.timer.unwrap();
    f.visible(&f.surface, &f.output, true);
    send_frame_callbacks(&mut f.state, &other, Duration::from_millis(10));
    f.dispatch();
    assert!(f.client.events.frames.is_empty());
    assert!(f.state.hidden_frames.timer.is_none());
    assert!(f.state.loop_handle.update(&token).is_err());
    send_frame_callbacks(&mut f.state, &f.output, Duration::from_millis(20));
    f.dispatch();
    assert_eq!(f.client.events.frames, [(1, 20)]);
    f.request(2);
    f.visible(&f.surface, &f.output, false);
    for _ in 0..100 {
        send_frame_callbacks(&mut f.state, &f.output, Duration::from_millis(25));
    }
    f.dispatch();
    assert_eq!(f.client.events.frames, [(1, 20)]);
    f.expire(&[f.surface.clone()]);
    assert_eq!(f.client.events.frames.len(), 2);
}

#[test]
fn pending_requests_wait_for_commit_and_synchronized_subsurface_commits() {
    let mut f = Fixture::new();
    f.surface.frame(&f.client.handle(), 1);
    f.dispatch();
    f.state.refresh();
    assert!(f.state.hidden_frames.timer.is_none());
    let child = f
        .client
        .bind::<wl_compositor::WlCompositor>(6)
        .create_surface(&f.client.handle(), ());
    let _subsurface = f
        .client
        .bind::<wl_subcompositor::WlSubcompositor>(1)
        .get_subsurface(&child, &f.surface, &f.client.handle(), ());
    child.frame(&f.client.handle(), 2);
    child.commit();
    f.dispatch();
    f.state.refresh();
    assert!(f.state.hidden_frames.timer.is_none());
    f.surface.commit();
    f.dispatch();
    f.state.refresh();
    assert!(f.state.hidden_frames.timer.is_some());
    f.expire(&[f.surface.clone(), child]);
    let mut labels: Vec<_> = f
        .client
        .events
        .frames
        .iter()
        .map(|(label, _)| *label)
        .collect();
    labels.sort_unstable();
    assert_eq!(labels, [1, 2]);
}

#[test]
fn popups_share_one_timer_and_destroyed_roles_cancel_it() {
    let mut f = Fixture::new();
    let popup = TestPopup::new(&f.client, Some(&f.shell));
    popup.surface.commit();
    f.dispatch();
    popup.surface.frame(&f.client.handle(), 2);
    popup.surface.commit();
    f.request(1);
    f.state.refresh();
    f.expire(&[f.surface.clone()]);
    assert_eq!(
        f.client
            .events
            .frames
            .iter()
            .map(|(label, _)| *label)
            .collect::<Vec<_>>(),
        [1]
    );
    let (token, _) = f.state.hidden_frames.timer.unwrap();
    assert!(f.state.loop_handle.update(&token).is_ok());
    popup.popup.destroy();
    popup.shell.destroy();
    popup.surface.destroy();
    f.toplevel.destroy();
    f.shell.destroy();
    f.surface.destroy();
    f.dispatch();
    f.state.refresh();
    assert!(f.state.hidden_frames.timer.is_none());
}

#[test]
fn removing_an_output_makes_its_pending_callbacks_eligible_for_throttling() {
    let mut f = Fixture::new();
    f.visible(&f.surface, &f.output, true);
    f.request(1);
    f.state.schedule_hidden_frames();
    assert!(f.state.hidden_frames.timer.is_none());
    f.state.space.unmap_output(&f.output);
    f.state.schedule_hidden_frames();
    assert!(f.state.hidden_frames.timer.is_some());
    f.expire(&[f.surface.clone()]);
    assert_eq!(f.client.events.frames.len(), 1);
}

#[test]
fn committed_callbacks_keep_request_order_and_wrap_the_protocol_timestamp() {
    let mut f = Fixture::new();
    f.visible(&f.surface, &f.output, true);
    for label in 1..=3 {
        f.surface.frame(&f.client.handle(), label);
    }
    f.surface.commit();
    f.dispatch();
    send_frame_callbacks(
        &mut f.state,
        &f.output,
        Duration::from_millis(u64::from(u32::MAX) + 5),
    );
    f.dispatch();
    assert_eq!(f.client.events.frames, [(1, 4), (2, 4), (3, 4)]);
    assert!(f.state.hidden_frames.timer.is_none());
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn hidden_workspace_minimized_and_locked_clients_progress_after_rendering_stops() {
    use smithay::{
        backend::{
            allocator::Fourcc,
            renderer::{
                damage::OutputDamageTracker,
                gles::{GlesRenderer, GlesTexture},
                Bind, Color32F, Offscreen,
            },
        },
        utils::Transform,
    };

    fn render(f: &mut Fixture, renderer: &mut GlesRenderer) {
        let elements = crate::render::output_elements(
            renderer,
            &mut f.state,
            &f.output,
            None,
            crate::render::CursorMode::Hidden,
        );
        let mut texture =
            Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, (800, 600).into())
                .unwrap();
        let mut target = renderer.bind(&mut texture).unwrap();
        let mut tracker = OutputDamageTracker::new((800, 600), 1.0, Transform::Normal);
        let result = tracker
            .render_output(renderer, &mut target, 0, &elements, Color32F::BLACK)
            .unwrap();
        crate::render::update_primary_scanout_output(&f.state, &f.output, &result.states);
        let time = f.state.clock_now();
        send_frame_callbacks(&mut f.state, &f.output, time);
        f.dispatch();
    }

    let egl = crate::test_support::EglTest::new();
    let mut f = Fixture::new();
    let mut renderer = egl.renderer();
    f.state.config.animations.enabled = false;
    f.state.config.decorations.mode = crate::config::DecorationModeConfig::None;
    f.client.paint_surface(&f.surface, 100, 100);
    f.dispatch();
    let (id, window) = f
        .state
        .windows
        .iter()
        .next()
        .map(|(id, managed)| (*id, managed.window.clone()))
        .unwrap();

    for (label, hidden) in [
        (1, false),
        (2, true),
        (3, false),
        (4, true),
        (5, false),
        (6, true),
    ] {
        match label {
            2 => {
                assert!(f.state.switch_workspace(2));
            }
            3 => {
                assert!(f.state.switch_workspace(1));
            }
            4 => {
                assert!(f.state.minimize_window(&window));
            }
            5 => {
                assert!(f.state.focus_window_by_id(id));
            }
            6 => f.state.session_locked = true,
            _ => {}
        }
        f.request(label);
        render(&mut f, &mut renderer);
        if hidden {
            assert_eq!(f.client.events.frames.len(), label as usize - 1);
            assert!(f.state.hidden_frames.timer.is_some());
            f.state.take_redraw_request();
            f.expire(&[f.surface.clone()]);
            assert!(!f.state.take_redraw_request());
        }
        assert_eq!(f.client.events.frames.len(), label as usize);
        assert_eq!(f.client.events.frames.last().unwrap().0, label);
        assert!(f.state.hidden_frames.timer.is_none());
    }
}

#[test]
fn unchanged_hidden_frame_scheduling_allocates_nothing() {
    let mut f = Fixture::new();
    f.request(1);
    f.state.schedule_hidden_frames();
    let timer = f.state.hidden_frames.timer;
    let allocations = crate::test_support::count_allocations(|| {
        for _ in 0..1000 {
            f.state.schedule_hidden_frames();
        }
    });
    assert_eq!(allocations, 0);
    assert_eq!(f.state.hidden_frames.timer, timer);
}
