use blair_protocol::{Point as CorePoint, Rect as CoreRect};
use smithay::{
    desktop::{layer_map_for_output, Window, WindowSurfaceType},
    utils::{Logical, Point},
    wayland::{
        compositor::with_states,
        shell::{wlr_layer::Layer as WlrLayer, xdg::SurfaceCachedState},
    },
};
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    decorations::{hit_test_frame, DecorationFrame, DecorationPart},
    render::decoration_theme,
    state::{window_wants_server_decoration, BlairState},
};

#[derive(Debug, Clone)]
pub enum WindowDrag {
    Move {
        window: Window,
        grab_offset: Point<i32, Logical>,
    },
    Resize {
        window: Window,
        edge: DecorationPart,
        start: CoreRect,
        grab_pos: Point<f64, Logical>,
    },
}

pub fn move_dragged_window(
    state: &mut BlairState,
    drag: Option<&WindowDrag>,
    pointer_pos: Point<f64, Logical>,
) -> bool {
    match drag {
        Some(WindowDrag::Move {
            window,
            grab_offset,
        }) => {
            let loc = (
                pointer_pos.x.round() as i32 - grab_offset.x,
                pointer_pos.y.round() as i32 - grab_offset.y,
            );
            state.space.map_element(window.clone(), loc, true);
            state.window_geometry_changed(window, loc);
            true
        }
        Some(WindowDrag::Resize {
            window,
            edge,
            start,
            grab_pos,
        }) => {
            if let Some(loc) = apply_resize(window, *edge, *start, *grab_pos, pointer_pos) {
                state.space.map_element(window.clone(), loc, true);
                state.window_geometry_changed(window, loc);
            }
            true
        }
        None => false,
    }
}

pub fn begin_window_drag(
    state: &mut BlairState,
    drag: &mut Option<WindowDrag>,
    window: Window,
    pointer_pos: Point<f64, Logical>,
) -> bool {
    let Some(window_loc) = state.space.element_location(&window) else {
        return false;
    };
    state.focus_window(&window);
    drag.replace(WindowDrag::Move {
        grab_offset: (
            pointer_pos.x.round() as i32 - window_loc.x,
            pointer_pos.y.round() as i32 - window_loc.y,
        )
            .into(),
        window,
    });
    true
}

pub fn begin_window_resize(
    state: &mut BlairState,
    drag: &mut Option<WindowDrag>,
    window: Window,
    edge: DecorationPart,
    pointer_pos: Point<f64, Logical>,
) -> bool {
    let Some(loc) = state.space.element_location(&window) else {
        return false;
    };
    let bbox = window.bbox();
    state.focus_window(&window);
    drag.replace(WindowDrag::Resize {
        start: CoreRect {
            x: loc.x,
            y: loc.y,
            width: bbox.size.w,
            height: bbox.size.h,
        },
        grab_pos: pointer_pos,
        edge,
        window,
    });
    true
}

fn toplevel_size_limits(
    window: &Window,
) -> (
    smithay::utils::Size<i32, Logical>,
    smithay::utils::Size<i32, Logical>,
) {
    let Some(toplevel) = window.toplevel() else {
        return Default::default();
    };
    with_states(toplevel.wl_surface(), |states| {
        let mut guard = states.cached_state.get::<SurfaceCachedState>();
        let current = guard.current();
        (current.min_size, current.max_size)
    })
}

fn apply_resize(
    window: &Window,
    edge: DecorationPart,
    start: CoreRect,
    grab_pos: Point<f64, Logical>,
    pointer_pos: Point<f64, Logical>,
) -> Option<(i32, i32)> {
    let toplevel = window.toplevel()?;
    let (min, max) = toplevel_size_limits(window);
    let dx = (pointer_pos.x - grab_pos.x).round() as i32;
    let dy = (pointer_pos.y - grab_pos.y).round() as i32;

    let grows_left = matches!(
        edge,
        DecorationPart::ResizeLeft
            | DecorationPart::ResizeTopLeft
            | DecorationPart::ResizeBottomLeft
    );
    let grows_right = matches!(
        edge,
        DecorationPart::ResizeRight
            | DecorationPart::ResizeTopRight
            | DecorationPart::ResizeBottomRight
    );
    let grows_top = matches!(
        edge,
        DecorationPart::ResizeTop | DecorationPart::ResizeTopLeft | DecorationPart::ResizeTopRight
    );
    let grows_bottom = matches!(
        edge,
        DecorationPart::ResizeBottom
            | DecorationPart::ResizeBottomLeft
            | DecorationPart::ResizeBottomRight
    );

    let mut width = start.width;
    let mut height = start.height;
    if grows_right {
        width = start.width + dx;
    }
    if grows_left {
        width = start.width - dx;
    }
    if grows_bottom {
        height = start.height + dy;
    }
    if grows_top {
        height = start.height - dy;
    }

    let min_w = if min.w > 0 { min.w } else { 1 };
    let min_h = if min.h > 0 { min.h } else { 1 };
    width = width.clamp(min_w, if max.w > 0 { max.w } else { i32::MAX });
    height = height.clamp(min_h, if max.h > 0 { max.h } else { i32::MAX });

    let x = if grows_left {
        start.x + (start.width - width)
    } else {
        start.x
    };
    let y = if grows_top {
        start.y + (start.height - height)
    } else {
        start.y
    };

    toplevel.with_pending_state(|state| state.size = Some((width, height).into()));
    toplevel.send_configure();
    (grows_left || grows_top).then_some((x, y))
}

pub fn window_under_including_decoration(
    state: &BlairState,
    pos: Point<f64, Logical>,
) -> Option<Window> {
    if let Some((window, _)) = state.space.element_under(pos) {
        return Some(window.clone());
    }
    if !state.config.window.server_side_decorations
        || matches!(
            state.config.window.layout,
            crate::config::WindowLayout::Tiling
        )
    {
        return None;
    }
    let theme = decoration_theme(state);
    let point = CorePoint { x: pos.x, y: pos.y };
    crate::render::z_ordered_windows(state)
        .into_iter()
        .rev()
        .find(|window| {
            if !window_wants_server_decoration(window) {
                return false;
            }
            let Some(loc) = state.space.element_location(window) else {
                return false;
            };
            let client = window.geometry();
            let geom = DecorationFrame::compute(
                CoreRect {
                    x: loc.x,
                    y: loc.y,
                    width: client.size.w,
                    height: client.size.h,
                },
                &theme,
            );
            geom.frame.contains(point)
        })
}

pub fn handle_decoration_press(
    state: &mut BlairState,
    drag: &mut Option<WindowDrag>,
    window: Window,
    pointer_pos: Point<f64, Logical>,
) -> bool {
    if !state.config.window.server_side_decorations
        || matches!(
            state.config.window.layout,
            crate::config::WindowLayout::Tiling
        )
        || !window_wants_server_decoration(&window)
    {
        return false;
    }
    let Some(window_loc) = state.space.element_location(&window) else {
        return false;
    };
    let client = window.geometry();
    let frame = CoreRect {
        x: window_loc.x,
        y: window_loc.y,
        width: client.size.w,
        height: client.size.h,
    };
    let point = CorePoint {
        x: pointer_pos.x,
        y: pointer_pos.y,
    };
    let part = hit_test_frame(frame, point, &decoration_theme(state));
    match part {
        DecorationPart::CloseButton => state.close_window(&window),
        DecorationPart::MinimizeButton => state.minimize_window(&window),
        DecorationPart::MaximizeButton => state.toggle_maximize_window(&window),
        DecorationPart::Titlebar => begin_window_drag(state, drag, window, pointer_pos),
        DecorationPart::ResizeTop
        | DecorationPart::ResizeBottom
        | DecorationPart::ResizeLeft
        | DecorationPart::ResizeRight
        | DecorationPart::ResizeTopLeft
        | DecorationPart::ResizeTopRight
        | DecorationPart::ResizeBottomLeft
        | DecorationPart::ResizeBottomRight => {
            begin_window_resize(state, drag, window, part, pointer_pos)
        }
        _ => false,
    }
}

pub fn window_surface_under(
    state: &BlairState,
    pos: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let (window, render_loc) = state.space.element_under(pos)?;
    let relative = pos - render_loc.to_f64();
    let (surface, surface_loc) = window.surface_under(relative, WindowSurfaceType::ALL)?;
    Some((surface, (render_loc + surface_loc).to_f64()))
}

fn layer_surface_under_in(
    state: &BlairState,
    pos: Point<f64, Logical>,
    layers: &[WlrLayer],
) -> Option<(WlSurface, Point<f64, Logical>, bool)> {
    let output = state.space.outputs().next()?.clone();
    let layer_map = layer_map_for_output(&output);
    for &layer in layers {
        for layer_surface in layer_map.layers_on(layer).rev() {
            let Some(geo) = layer_map.layer_geometry(layer_surface) else {
                continue;
            };
            let local: Point<f64, Logical> =
                (pos.x - geo.loc.x as f64, pos.y - geo.loc.y as f64).into();
            let Some((target, target_offset)) =
                layer_surface.surface_under(local, WindowSurfaceType::ALL)
            else {
                continue;
            };
            let can_focus = layer_surface.can_receive_keyboard_focus();
            let origin = geo.loc.to_f64() + target_offset.to_f64();
            tracing::trace!(
                is_popup = target != *layer_surface.wl_surface(),
                "layer hit-test resolved"
            );
            return Some((target, origin, can_focus));
        }
    }
    None
}

pub fn upper_layer_surface_under(
    state: &BlairState,
    pos: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>, bool)> {
    layer_surface_under_in(state, pos, &[WlrLayer::Overlay, WlrLayer::Top])
}

pub fn lower_layer_surface_under(
    state: &BlairState,
    pos: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>, bool)> {
    layer_surface_under_in(state, pos, &[WlrLayer::Bottom, WlrLayer::Background])
}
