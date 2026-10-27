//! `blair_blur_unstable_v1`, blair's own background-blur protocol: a client
//! asks blair to blur whatever is behind a region of its surface, without
//! dictating how — blair owns the region's rectangles, the render pass, and
//! (via [`crate::config`]) the strength. CreamUI's Wayland backend speaks
//! this when running under blair; other compositors simply don't advertise
//! the global, so unrelated clients never see it.

use std::sync::Mutex;

use blair_blur_protocol::server::{
    blair_blur_manager_v1::{self, BlairBlurManagerV1},
    blair_blur_v1::{self, BlairBlurV1},
};
use smithay::{
    reexports::wayland_server::{
        protocol::{wl_region::WlRegion, wl_surface::WlSurface},
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    },
    utils::{Logical, Rectangle},
    wayland::compositor::{get_region_attributes, with_states, RectangleKind},
};

use crate::state::BlairState;

const VERSION: u32 = 1;

/// The blur region a surface's `blair_blur_v1` object last committed.
/// `Blur(None)` blurs the whole surface, matching `set_region(None)` in the
/// protocol.
#[derive(Clone, Debug, PartialEq)]
pub struct Blur(pub Option<Vec<Rectangle<i32, Logical>>>);

#[derive(Default)]
struct SurfaceBlurState(Mutex<Option<Blur>>);

/// The region currently blurred behind `surface`, or `None` if it has no
/// active blur (never requested one, or it was `unset`).
pub fn blur_of(surface: &WlSurface) -> Option<Blur> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<SurfaceBlurState>()
            .and_then(|state| state.0.lock().expect("blur state lock poisoned").clone())
    })
}

fn set_blur(state: &mut BlairState, surface: &WlSurface, blur: Option<Blur>) {
    let changed = with_states(surface, |states| {
        let data = states.data_map.get_or_insert(SurfaceBlurState::default);
        let mut current = data.0.lock().expect("blur state lock poisoned");
        if *current == blur {
            return false;
        }
        *current = blur;
        true
    });
    if changed {
        state.request_redraw();
    }
}

/// Registers the `blair_blur_manager_v1` global.
pub fn register(display: &DisplayHandle) {
    display.create_global::<BlairState, BlairBlurManagerV1, _>(VERSION, ());
}

/// Pending state of one `blair_blur_v1` object, applied to its surface on
/// `commit`.
pub struct BlurObjectData {
    surface: WlSurface,
    pending: Mutex<Blur>,
}

impl GlobalDispatch<BlairBlurManagerV1, ()> for BlairState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<BlairBlurManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<BlairBlurManagerV1, ()> for BlairState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &BlairBlurManagerV1,
        request: blair_blur_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            blair_blur_manager_v1::Request::GetBlur { id, surface } => {
                data_init.init(
                    id,
                    BlurObjectData {
                        surface,
                        pending: Mutex::new(Blur(None)),
                    },
                );
            }
            blair_blur_manager_v1::Request::Unset { surface } => set_blur(state, &surface, None),
            _ => {}
        }
    }
}

impl Dispatch<BlairBlurV1, BlurObjectData> for BlairState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _blur: &BlairBlurV1,
        request: blair_blur_v1::Request,
        data: &BlurObjectData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            blair_blur_v1::Request::SetRegion { region } => {
                *data.pending.lock().expect("blur state lock poisoned") =
                    Blur(region.as_ref().map(region_rects));
            }
            blair_blur_v1::Request::Commit => {
                let region = data
                    .pending
                    .lock()
                    .expect("blur state lock poisoned")
                    .clone();
                set_blur(state, &data.surface, Some(region));
            }
            _ => {}
        }
    }
}

/// Preserve union/subtraction and disjoint shapes instead of expanding them
/// into one bounding box (which blurs gaps and transparent rounded corners).
fn region_rects(region: &WlRegion) -> Vec<Rectangle<i32, Logical>> {
    normalize_region(get_region_attributes(region).rects)
}

fn normalize_region(
    ops: Vec<(RectangleKind, Rectangle<i32, Logical>)>,
) -> Vec<Rectangle<i32, Logical>> {
    let mut result = Vec::new();
    for (kind, rect) in ops {
        if rect.is_empty() {
            continue;
        }
        match kind {
            RectangleKind::Add => result.extend(rect.subtract_rects(result.clone())),
            RectangleKind::Subtract => {
                result = Rectangle::subtract_rects_many_in_place(result, [rect])
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_blur_changes_request_a_repaint_without_a_surface_commit() {
        use blair_blur_protocol::client::blair_blur_manager_v1::BlairBlurManagerV1;
        use wayland_client::protocol::wl_compositor::WlCompositor;
        let (mut display, _event_loop, mut state) = crate::state::tests::fixture();
        let mut client = crate::test_support::TestClient::new(&mut display, &mut state);
        let compositor = client.bind::<WlCompositor>(6);
        let manager = client.bind::<BlairBlurManagerV1>(1);
        let surface = compositor.create_surface(&client.handle(), ());
        let blur = manager.get_blur(&surface, &client.handle(), ());
        let region = compositor.create_region(&client.handle(), ());
        region.add(10, 20, 100, 80);
        blur.set_region(Some(&region));
        state.take_redraw_request();
        client.dispatch(&mut display, &mut state);
        let surface = client.surface(&surface);
        assert!(blur_of(&surface).is_none());
        assert!(!state.take_redraw_request());

        blur.commit();
        client.dispatch(&mut display, &mut state);
        assert_eq!(
            blur_of(&surface),
            Some(Blur(Some(vec![Rectangle::new(
                (10, 20).into(),
                (100, 80).into()
            )])))
        );
        assert!(state.take_redraw_request());
        blur.commit();
        client.dispatch(&mut display, &mut state);
        assert!(!state.take_redraw_request());

        let client_surface = compositor.create_surface(&client.handle(), ());
        let whole = manager.get_blur(&client_surface, &client.handle(), ());
        whole.commit();
        client.dispatch(&mut display, &mut state);
        assert_eq!(blur_of(&client.surface(&client_surface)), Some(Blur(None)));
        assert!(state.take_redraw_request());
        manager.unset(&client_surface);
        client.dispatch(&mut display, &mut state);
        assert!(blur_of(&client.surface(&client_surface)).is_none());
        assert!(state.take_redraw_request());
        manager.unset(&client_surface);
        client.dispatch(&mut display, &mut state);
        assert!(!state.take_redraw_request());
    }

    #[test]
    fn regions_preserve_gaps_holes_and_do_not_overlap() {
        let rect = |x, y, w, h| Rectangle::new((x, y).into(), (w, h).into());
        let regions = normalize_region(vec![
            (RectangleKind::Add, rect(0, 0, 20, 20)),
            (RectangleKind::Add, rect(10, 0, 20, 20)),
            (RectangleKind::Add, rect(40, 0, 10, 10)),
            (RectangleKind::Subtract, rect(5, 5, 10, 10)),
        ]);
        let contains = |x, y| regions.iter().any(|r| r.contains((x, y)));
        assert!(contains(0, 0));
        assert!(contains(25, 10));
        assert!(contains(45, 5));
        assert!(!contains(7, 7));
        assert!(!contains(35, 5));
        for (i, a) in regions.iter().enumerate() {
            assert!(regions[i + 1..].iter().all(|b| !a.overlaps(*b)));
        }
    }
}
