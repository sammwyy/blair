use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};
use wayland_server::protocol::wl_surface::WlSurface;

use crate::state::BlairState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeEdges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl ResizeEdges {
    pub fn from_xdg(edge: xdg_toplevel::ResizeEdge) -> Self {
        use xdg_toplevel::ResizeEdge as E;
        Self {
            top: matches!(edge, E::Top | E::TopLeft | E::TopRight),
            bottom: matches!(edge, E::Bottom | E::BottomLeft | E::BottomRight),
            left: matches!(edge, E::Left | E::TopLeft | E::BottomLeft),
            right: matches!(edge, E::Right | E::TopRight | E::BottomRight),
        }
    }

    pub fn is_empty(self) -> bool {
        !(self.top || self.bottom || self.left || self.right)
    }
}

/// Anchors the edges opposite to the ones being dragged while the client
/// catches up with the requested size.
#[derive(Debug, Clone, Copy)]
pub struct ResizeAnchor {
    pub edges: ResizeEdges,
    pub initial: Rectangle<i32, Logical>,
}

impl ResizeAnchor {
    pub fn location_for(&self, size: Size<i32, Logical>) -> Point<i32, Logical> {
        let mut location = self.initial.loc;
        if self.edges.left {
            location.x += self.initial.size.w - size.w;
        }
        if self.edges.top {
            location.y += self.initial.size.h - size.h;
        }
        location
    }
}

macro_rules! forward_gestures {
    () => {
        fn gesture_swipe_begin(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            event: &GestureSwipeBeginEvent,
        ) {
            handle.gesture_swipe_begin(data, event);
        }

        fn gesture_swipe_update(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            event: &GestureSwipeUpdateEvent,
        ) {
            handle.gesture_swipe_update(data, event);
        }

        fn gesture_swipe_end(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            event: &GestureSwipeEndEvent,
        ) {
            handle.gesture_swipe_end(data, event);
        }

        fn gesture_pinch_begin(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            event: &GesturePinchBeginEvent,
        ) {
            handle.gesture_pinch_begin(data, event);
        }

        fn gesture_pinch_update(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            event: &GesturePinchUpdateEvent,
        ) {
            handle.gesture_pinch_update(data, event);
        }

        fn gesture_pinch_end(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            event: &GesturePinchEndEvent,
        ) {
            handle.gesture_pinch_end(data, event);
        }

        fn gesture_hold_begin(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            event: &GestureHoldBeginEvent,
        ) {
            handle.gesture_hold_begin(data, event);
        }

        fn gesture_hold_end(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            event: &GestureHoldEndEvent,
        ) {
            handle.gesture_hold_end(data, event);
        }

        fn relative_motion(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            _focus: Option<(WlSurface, Point<f64, Logical>)>,
            event: &RelativeMotionEvent,
        ) {
            handle.relative_motion(data, None, event);
        }

        fn axis(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
            details: AxisFrame,
        ) {
            handle.axis(data, details);
        }

        fn frame(
            &mut self,
            data: &mut BlairState,
            handle: &mut PointerInnerHandle<'_, BlairState>,
        ) {
            handle.frame(data);
        }

        fn start_data(&self) -> &GrabStartData<BlairState> {
            &self.start_data
        }
    };
}

pub struct MoveGrab {
    pub start_data: GrabStartData<BlairState>,
    pub window: Window,
    pub initial_location: Point<i32, Logical>,
}

impl PointerGrab<BlairState> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut BlairState,
        handle: &mut PointerInnerHandle<'_, BlairState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        if !self.window.alive() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        let delta = event.location - self.start_data.location;
        let location = (self.initial_location.to_f64() + delta).to_i32_round();
        data.move_window(&self.window, location);
    }

    fn button(
        &mut self,
        data: &mut BlairState,
        handle: &mut PointerInnerHandle<'_, BlairState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    forward_gestures!();

    fn unset(&mut self, data: &mut BlairState) {
        tracing::debug!("interactive move finished");
        data.request_redraw();
    }
}

pub struct ResizeGrab {
    pub start_data: GrabStartData<BlairState>,
    pub window: Window,
    pub anchor: ResizeAnchor,
    pub last_size: Size<i32, Logical>,
}

impl ResizeGrab {
    fn size_for(&self, location: Point<f64, Logical>) -> Size<i32, Logical> {
        let delta = (location - self.start_data.location).to_i32_round::<i32>();
        let edges = self.anchor.edges;
        let initial = self.anchor.initial.size;
        let mut width = initial.w;
        let mut height = initial.h;
        if edges.left {
            width -= delta.x;
        } else if edges.right {
            width += delta.x;
        }
        if edges.top {
            height -= delta.y;
        } else if edges.bottom {
            height += delta.y;
        }
        let (min, max) = size_limits(&self.window);
        let clamp = |value: i32, min: i32, max: i32| {
            let max = if max > 0 { max } else { i32::MAX };
            value.clamp(min.max(1), max.max(min.max(1)))
        };
        (clamp(width, min.w, max.w), clamp(height, min.h, max.h)).into()
    }
}

impl PointerGrab<BlairState> for ResizeGrab {
    fn motion(
        &mut self,
        data: &mut BlairState,
        handle: &mut PointerInnerHandle<'_, BlairState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let Some(toplevel) = self.window.toplevel().filter(|_| self.window.alive()) else {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        };
        let size = self.size_for(event.location);
        if size == self.last_size {
            return;
        }
        self.last_size = size;
        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
            state.size = Some(size);
        });
        toplevel.send_pending_configure();
    }

    fn button(
        &mut self,
        data: &mut BlairState,
        handle: &mut PointerInnerHandle<'_, BlairState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    forward_gestures!();

    fn unset(&mut self, data: &mut BlairState) {
        if let Some(toplevel) = self.window.toplevel().filter(|_| self.window.alive()) {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Resizing);
            });
            toplevel.send_pending_configure();
        }
        data.finish_resize(&self.window);
        tracing::debug!(size = ?self.last_size, "interactive resize finished");
    }
}

fn size_limits(window: &Window) -> (Size<i32, Logical>, Size<i32, Logical>) {
    let Some(toplevel) = window.toplevel() else {
        return Default::default();
    };
    with_states(toplevel.wl_surface(), |states| {
        let mut cached = states.cached_state.get::<SurfaceCachedState>();
        let current = cached.current();
        (current.min_size, current.max_size)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_keeps_opposite_edges_fixed() {
        let anchor = ResizeAnchor {
            edges: ResizeEdges {
                top: true,
                bottom: false,
                left: true,
                right: false,
            },
            initial: Rectangle::new((100, 100).into(), (400, 300).into()),
        };
        assert_eq!(anchor.location_for((350, 320).into()), (150, 80).into());
        let bottom_right = ResizeAnchor {
            edges: ResizeEdges {
                top: false,
                bottom: true,
                left: false,
                right: true,
            },
            ..anchor
        };
        assert_eq!(
            bottom_right.location_for((10, 10).into()),
            (100, 100).into()
        );
    }

    #[test]
    fn xdg_edges_map_to_flags() {
        let edges = ResizeEdges::from_xdg(xdg_toplevel::ResizeEdge::BottomLeft);
        assert!(edges.bottom && edges.left && !edges.top && !edges.right);
        assert!(ResizeEdges::from_xdg(xdg_toplevel::ResizeEdge::None).is_empty());
    }
}
