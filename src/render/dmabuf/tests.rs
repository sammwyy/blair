use super::*;
use crate::test_support::{add_output, ClientEvents, TestClient};
use smithay::{
    backend::{
        allocator::{Format, Fourcc, Modifier},
        renderer::element::{Id, RenderElementState},
    },
    reexports::{calloop::EventLoop, wayland_server::Display},
};
use std::{fs::File, os::unix::fs::FileExt};
use wayland_client::{
    protocol::{
        wl_compositor::WlCompositor, wl_subcompositor::WlSubcompositor, wl_surface::WlSurface,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols::{
    wp::linux_dmabuf::zv1::client::{
        zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1},
        zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    },
    xdg::shell::client::xdg_wm_base::XdgWmBase,
};

wayland_client::delegate_noop!(ClientEvents: ignore ZwpLinuxDmabufV1);
impl Dispatch<ZwpLinuxDmabufFeedbackV1, u32> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        label: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.dmabuf_feedback.push((*label, event));
    }
}

struct Fixture {
    display: Display<BlairState>,
    _event_loop: EventLoop<'static, BlairState>,
    state: BlairState,
    client: TestClient,
    output: Output,
    feedback: ScanoutFeedback,
}

fn format(code: Fourcc, modifier: Modifier) -> Format {
    Format { code, modifier }
}

impl Fixture {
    fn new() -> Self {
        let (mut display, event_loop, mut state) = crate::state::tests::fixture();
        let render = [
            format(Fourcc::Xrgb8888, Modifier::Linear),
            format(Fourcc::Argb8888, Modifier::Linear),
        ]
        .into_iter()
        .collect();
        let plane = [
            format(Fourcc::Xrgb8888, Modifier::Linear),
            format(Fourcc::Xrgb8888, Modifier::from(99)),
        ]
        .into_iter()
        .collect();
        let feedback = ScanoutFeedback::new(1, 2, &render, &plane).unwrap();
        state.dmabuf_global = Some(
            state
                .dmabuf_state
                .create_global_with_default_feedback::<BlairState>(
                    &display.handle(),
                    feedback.render(),
                ),
        );
        let output = add_output(&mut state, "test", (0, 0), (800, 600));
        let client = TestClient::new(&mut display, &mut state);
        Self {
            display,
            _event_loop: event_loop,
            state,
            client,
            output,
            feedback,
        }
    }

    fn dispatch(&mut self) {
        self.client.dispatch(&mut self.display, &mut self.state);
    }

    fn window(&mut self, label: u32) -> WlSurface {
        let surface = self
            .client
            .bind::<WlCompositor>(6)
            .create_surface(&self.client.handle(), ());
        let shell =
            self.client
                .bind::<XdgWmBase>(6)
                .get_xdg_surface(&surface, &self.client.handle(), ());
        let _toplevel = shell.get_toplevel(&self.client.handle(), ());
        surface.commit();
        self.dispatch();
        self.client.paint_surface(&surface, 400, 240);
        self.dispatch();
        self.client
            .bind::<ZwpLinuxDmabufV1>(4)
            .get_surface_feedback(&surface, &self.client.handle(), label);
        self.dispatch();
        surface
    }

    fn fullscreen(&mut self, surface: &WlSurface, enabled: bool) {
        let surface = self.client.surface(surface);
        self.state
            .windows
            .values_mut()
            .find(|managed| managed.window.wl_surface().as_deref() == Some(&surface))
            .unwrap()
            .fullscreen = enabled;
    }

    fn send(&mut self, surfaces: &[(&WlSurface, RenderElementPresentationState)]) {
        let mut states = RenderElementStates::default();
        for (surface, presentation) in surfaces {
            let surface = self.client.surface(surface);
            states.states.insert(
                Id::from_wayland_resource(&surface),
                RenderElementState {
                    visible_area: if *presentation == RenderElementPresentationState::Skipped {
                        0
                    } else {
                        100
                    },
                    presentation_state: *presentation,
                },
            );
        }
        super::super::update_primary_scanout_output(&self.state, &self.output, &states);
        self.feedback.send(&self.state, &self.output, &states);
        self.dispatch();
    }
}

fn tranches(client: &ClientEvents, label: u32) -> Vec<(Dev, bool, Vec<Format>)> {
    use zwp_linux_dmabuf_feedback_v1::Event;
    let mut table = Vec::new();
    let mut tranches = Vec::new();
    let mut device = 0;
    let mut scanout = false;
    let mut formats = Vec::new();
    let mut done = false;
    for (_, event) in client.dmabuf_feedback.iter().filter(|(id, _)| *id == label) {
        match event {
            Event::FormatTable { fd, size } => {
                let mut bytes = vec![0; *size as usize];
                File::from(fd.try_clone().unwrap())
                    .read_exact_at(&mut bytes, 0)
                    .unwrap();
                assert_eq!(bytes.len(), *size as usize);
                table = bytes
                    .chunks_exact(16)
                    .map(|chunk| {
                        format(
                            Fourcc::try_from(u32::from_ne_bytes(chunk[..4].try_into().unwrap()))
                                .unwrap(),
                            Modifier::from(u64::from_ne_bytes(chunk[8..].try_into().unwrap())),
                        )
                    })
                    .collect();
            }
            Event::MainDevice { device } => assert_eq!(device, &1_u64.to_ne_bytes()),
            Event::TrancheTargetDevice { device: bytes } => {
                device = Dev::from_ne_bytes(bytes.as_slice().try_into().unwrap())
            }
            Event::TrancheFlags { flags } => {
                scanout = matches!(flags, WEnum::Value(flags) if flags.contains(zwp_linux_dmabuf_feedback_v1::TrancheFlags::Scanout))
            }
            Event::TrancheFormats { indices } => formats.extend(
                indices
                    .chunks_exact(2)
                    .map(|index| table[u16::from_ne_bytes(index.try_into().unwrap()) as usize]),
            ),
            Event::TrancheDone => {
                tranches.push((device, scanout, std::mem::take(&mut formats)));
                scanout = false;
            }
            Event::Done => done = true,
            _ => {}
        }
    }
    assert!(done);
    tranches
}

#[test]
fn surface_feedback_filters_formats_and_reverts_when_composition_changes() {
    let mut f = Fixture::new();
    let fullscreen = f.window(1);
    let normal = f.window(2);
    f.fullscreen(&fullscreen, true);
    assert!(tranches(&f.client.events, 1)
        .iter()
        .all(|(_, scanout, _)| !scanout));
    f.client.events.dmabuf_feedback.clear();
    f.send(&[
        (
            &fullscreen,
            RenderElementPresentationState::Rendering {
                reason: Some(RenderingReason::FormatUnsupported),
            },
        ),
        (&normal, RenderElementPresentationState::ZeroCopy),
    ]);
    let preferred = tranches(&f.client.events, 1);
    assert_eq!(
        preferred[0],
        (2, true, vec![format(Fourcc::Xrgb8888, Modifier::Linear)])
    );
    assert!(preferred
        .iter()
        .filter(|(_, scanout, _)| !scanout)
        .all(|(device, _, _)| *device == 1));
    assert!(!f
        .client
        .events
        .dmabuf_feedback
        .iter()
        .any(|(label, _)| *label == 2));
    f.client.events.dmabuf_feedback.clear();
    f.send(&[(&fullscreen, RenderElementPresentationState::ZeroCopy)]);
    assert!(f.client.events.dmabuf_feedback.is_empty());
    for presentation in [
        RenderElementPresentationState::Rendering {
            reason: Some(RenderingReason::ScanoutFailed),
        },
        RenderElementPresentationState::Rendering { reason: None },
        RenderElementPresentationState::Skipped,
    ] {
        f.send(&[(&fullscreen, presentation)]);
        assert!(tranches(&f.client.events, 1)
            .iter()
            .all(|(_, scanout, _)| !scanout));
        f.client.events.dmabuf_feedback.clear();
        f.send(&[(&fullscreen, RenderElementPresentationState::ZeroCopy)]);
        assert!(tranches(&f.client.events, 1)[0].1);
        f.client.events.dmabuf_feedback.clear();
    }
    f.state.config.accessibility.zoom = true;
    f.state.config.accessibility.zoom_factor = 2.0;
    f.send(&[(&fullscreen, RenderElementPresentationState::ZeroCopy)]);
    assert!(tranches(&f.client.events, 1)
        .iter()
        .all(|(_, scanout, _)| !scanout));
    f.client.events.dmabuf_feedback.clear();
    f.state.config.accessibility.zoom = false;
    f.send(&[(&fullscreen, RenderElementPresentationState::ZeroCopy)]);
    assert!(tranches(&f.client.events, 1)[0].1);
    f.client.events.dmabuf_feedback.clear();
    f.fullscreen(&fullscreen, false);
    f.send(&[(&fullscreen, RenderElementPresentationState::ZeroCopy)]);
    assert!(tranches(&f.client.events, 1)
        .iter()
        .all(|(_, scanout, _)| !scanout));
}

#[test]
fn feedback_tracks_subsurfaces_output_ownership_lock_and_minimization() {
    let mut f = Fixture::new();
    let root = f.window(1);
    f.fullscreen(&root, true);
    let child = f
        .client
        .bind::<WlCompositor>(6)
        .create_surface(&f.client.handle(), ());
    let subsurface =
        f.client
            .bind::<WlSubcompositor>(1)
            .get_subsurface(&child, &root, &f.client.handle(), ());
    f.client.paint_surface(&child, 32, 32);
    root.commit();
    f.dispatch();
    let child_feedback =
        f.client
            .bind::<ZwpLinuxDmabufV1>(4)
            .get_surface_feedback(&child, &f.client.handle(), 3);
    f.dispatch();
    f.client.events.dmabuf_feedback.clear();
    let zero_copy = RenderElementPresentationState::ZeroCopy;
    f.send(&[(&root, zero_copy), (&child, zero_copy)]);
    assert!(tranches(&f.client.events, 1)[0].1);
    assert!(tranches(&f.client.events, 3)[0].1);
    f.client.events.dmabuf_feedback.clear();
    let mut rendered = RenderElementStates::default();
    for surface in [&root, &child] {
        rendered.states.insert(
            Id::from_wayland_resource(&f.client.surface(surface)),
            RenderElementState {
                visible_area: 100,
                presentation_state: zero_copy,
            },
        );
    }
    let other = add_output(&mut f.state, "other", (800, 0), (800, 600));
    super::super::update_primary_scanout_output(
        &f.state,
        &f.output,
        &RenderElementStates::default(),
    );
    super::super::update_primary_scanout_output(&f.state, &other, &rendered);
    f.feedback.send(&f.state, &f.output, &rendered);
    f.dispatch();
    assert!(f.client.events.dmabuf_feedback.is_empty());
    super::super::update_primary_scanout_output(&f.state, &other, &RenderElementStates::default());
    f.feedback
        .send(&f.state, &f.output, &RenderElementStates::default());
    f.dispatch();
    assert!(tranches(&f.client.events, 1)
        .iter()
        .all(|(_, scanout, _)| !scanout));
    assert!(tranches(&f.client.events, 3)
        .iter()
        .all(|(_, scanout, _)| !scanout));
    for locked in [true, false] {
        f.client.events.dmabuf_feedback.clear();
        f.send(&[(&root, zero_copy), (&child, zero_copy)]);
        assert!(tranches(&f.client.events, 1)[0].1);
        f.client.events.dmabuf_feedback.clear();
        if locked {
            f.state.session_locked = true;
        } else {
            f.state.windows.values_mut().next().unwrap().minimized = Some((0, 0).into());
        }
        f.send(&[(&root, zero_copy), (&child, zero_copy)]);
        assert!(tranches(&f.client.events, 1)
            .iter()
            .all(|(_, scanout, _)| !scanout));
        assert!(tranches(&f.client.events, 3)
            .iter()
            .all(|(_, scanout, _)| !scanout));
        f.state.session_locked = false;
    }
    child_feedback.destroy();
    subsurface.destroy();
    child.destroy();
    f.dispatch();
}

#[test]
fn disjoint_scanout_formats_share_the_render_feedback() {
    let render = [format(Fourcc::Xrgb8888, Modifier::Linear)]
        .into_iter()
        .collect();
    let plane = [format(Fourcc::Argb8888, Modifier::Linear)]
        .into_iter()
        .collect();
    let feedback = ScanoutFeedback::new(1, 2, &render, &plane).unwrap();
    assert_eq!(feedback.render, feedback.scanout);
}
