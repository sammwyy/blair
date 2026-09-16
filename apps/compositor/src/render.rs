use blair_protocol::Rect;
use smithay::{
    backend::renderer::{Color32F, Frame},
    desktop::{PopupManager, Window},
    utils::{Physical, Rectangle},
    wayland::compositor::{with_surface_tree_downward, SurfaceAttributes, TraversalAction},
};
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    decorations::{DecorationFrame, DecorationTheme},
    state::{window_wants_server_decoration, BlairState},
};

pub fn decoration_theme(state: &BlairState) -> DecorationTheme {
    state.config.decoration.to_theme()
}

pub fn z_ordered_windows(state: &BlairState) -> Vec<Window> {
    match state.space.outputs().next() {
        Some(output) => state.space.elements_for_output(output).cloned().collect(),
        None => state.space.elements().cloned().collect(),
    }
}

pub fn draw_server_decorations<F: Frame>(
    frame: &mut F,
    damage: &[Rectangle<i32, Physical>],
    state: &BlairState,
) -> Result<(), F::Error> {
    if !state.config.window.server_side_decorations {
        return Ok(());
    }
    let theme = decoration_theme(state);
    let stack = z_ordered_windows(state);

    for (index, window) in stack.iter().enumerate() {
        if !window_wants_server_decoration(window) {
            continue;
        }
        let Some(loc) = state.space.element_location(window) else {
            continue;
        };
        let client = window.geometry();
        let geom = DecorationFrame::compute(
            Rect {
                x: loc.x,
                y: loc.y,
                width: client.size.w,
                height: client.size.h,
            },
            &theme,
        );

        let occluders: Vec<Rect> = stack[index + 1..]
            .iter()
            .filter_map(|window| {
                let loc = state.space.element_location(window)?;
                let bbox = window.bbox();
                Some(Rect {
                    x: loc.x,
                    y: loc.y,
                    width: bbox.size.w,
                    height: bbox.size.h,
                })
            })
            .collect();

        let focused = state.window_id(window) == state.focused_window;
        let titlebar_color = rgba(if focused {
            theme.active_titlebar
        } else {
            theme.inactive_titlebar
        });
        let border_color = rgba(if focused {
            theme.active_border
        } else {
            theme.inactive_border
        });

        for (rect, color) in [
            (geom.titlebar, titlebar_color),
            (geom.border_left, border_color),
            (geom.border_right, border_color),
            (geom.border_bottom, border_color),
            (geom.close_btn, rgba(theme.close_button)),
            (geom.maximize_btn, rgba(theme.maximize_button)),
            (geom.minimize_btn, rgba(theme.minimize_button)),
        ] {
            for piece in clip_rect(rect, &occluders) {
                draw_decoration_rect(frame, damage, piece, color)?;
            }
        }
    }
    Ok(())
}

pub fn draw_decoration_rect<F: Frame>(
    frame: &mut F,
    damage: &[Rectangle<i32, Physical>],
    rect: Rect,
    color: Color32F,
) -> Result<(), F::Error> {
    if rect.width <= 0 || rect.height <= 0 {
        return Ok(());
    }
    let dst: Rectangle<i32, Physical> =
        Rectangle::new((rect.x, rect.y).into(), (rect.width, rect.height).into());
    frame.draw_solid(dst, damage, color)
}

pub fn clip_rect(rect: Rect, occluders: &[Rect]) -> Vec<Rect> {
    let mut pieces = vec![rect];
    for occluder in occluders {
        pieces = pieces
            .into_iter()
            .flat_map(|piece| subtract_rect(piece, *occluder))
            .collect();
    }
    pieces
}

fn subtract_rect(a: Rect, b: Rect) -> Vec<Rect> {
    let ax2 = a.x + a.width;
    let ay2 = a.y + a.height;
    let bx1 = b.x.max(a.x);
    let by1 = b.y.max(a.y);
    let bx2 = (b.x + b.width).min(ax2);
    let by2 = (b.y + b.height).min(ay2);
    if bx1 >= bx2 || by1 >= by2 {
        return vec![a];
    }
    let mut out = Vec::new();
    if by1 > a.y {
        out.push(Rect {
            x: a.x,
            y: a.y,
            width: a.width,
            height: by1 - a.y,
        });
    }
    if by2 < ay2 {
        out.push(Rect {
            x: a.x,
            y: by2,
            width: a.width,
            height: ay2 - by2,
        });
    }
    if bx1 > a.x {
        out.push(Rect {
            x: a.x,
            y: by1,
            width: bx1 - a.x,
            height: by2 - by1,
        });
    }
    if bx2 < ax2 {
        out.push(Rect {
            x: bx2,
            y: by1,
            width: ax2 - bx2,
            height: by2 - by1,
        });
    }
    out
}

pub fn rgba(color: [u8; 4]) -> Color32F {
    Color32F::new(
        color[0] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[2] as f32 / 255.0,
        color[3] as f32 / 255.0,
    )
}

pub fn send_frames_surface_tree(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surface, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

// Popups are separate surface trees and need independent frame callbacks.
pub fn send_frame_callbacks(state: &BlairState, time: u32) {
    for surface in state.xdg_shell_state.toplevel_surfaces() {
        let wl_surface = surface.wl_surface();
        send_frames_surface_tree(wl_surface, time);
        for (popup, _) in PopupManager::popups_for_surface(wl_surface) {
            send_frames_surface_tree(popup.wl_surface(), time);
        }
    }
    for layer in &state.layer_surfaces {
        let wl_surface = layer.wl_surface();
        send_frames_surface_tree(wl_surface, time);
        for (popup, _) in PopupManager::popups_for_surface(wl_surface) {
            send_frames_surface_tree(popup.wl_surface(), time);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn clip_rect_with_no_occluders_is_unchanged() {
        let pieces = clip_rect(rect(0, 0, 10, 10), &[]);
        assert_eq!(pieces, vec![rect(0, 0, 10, 10)]);
    }

    #[test]
    fn clip_rect_removes_a_fully_covering_occluder() {
        let pieces = clip_rect(rect(0, 0, 10, 10), &[rect(0, 0, 10, 10)]);
        assert!(pieces.is_empty());
    }

    #[test]
    fn clip_rect_splits_around_a_partial_occluder() {
        let pieces = clip_rect(rect(0, 0, 10, 10), &[rect(4, 4, 2, 2)]);
        let area: i32 = pieces.iter().map(|piece| piece.width * piece.height).sum();
        assert_eq!(area, 100 - 4);
    }
}
