use blair_protocol::{Point, Rect};

use super::{frame::DecorationFrame, theme::DecorationTheme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationPart {
    None,
    Client,
    Titlebar,
    CloseButton,
    MaximizeButton,
    MinimizeButton,
    ResizeTop,
    ResizeBottom,
    ResizeLeft,
    ResizeRight,
    ResizeTopLeft,
    ResizeTopRight,
    ResizeBottomLeft,
    ResizeBottomRight,
}

const GRAB_MARGIN: f64 = 16.0;

/// How far outside the visible frame the resize handles still respond, in
/// logical pixels.
pub const RESIZE_OUTSET: i32 = 6;

pub fn hit_test_frame(
    client: Rect,
    point: Point,
    theme: &DecorationTheme,
    has_titlebar: bool,
) -> DecorationPart {
    let geom = DecorationFrame::compute(client, theme, has_titlebar);

    let fx = geom.frame.x as f64;
    let fy = geom.frame.y as f64;
    let fw = geom.frame.width as f64;
    let fh = geom.frame.height as f64;
    let outset = f64::from(RESIZE_OUTSET);
    let within_frame = point.x >= fx - outset
        && point.x <= fx + fw + outset
        && point.y >= fy - outset
        && point.y <= fy + fh + outset;
    let near_top = point.y < fy + GRAB_MARGIN;
    let near_bottom = point.y > fy + fh - GRAB_MARGIN;
    let near_left = point.x < fx + GRAB_MARGIN;
    let near_right = point.x > fx + fw - GRAB_MARGIN;

    // Corners take priority over titlebar controls.
    if within_frame {
        match (near_top, near_bottom, near_left, near_right) {
            (true, _, true, _) => return DecorationPart::ResizeTopLeft,
            (true, _, _, true) => return DecorationPart::ResizeTopRight,
            (_, true, true, _) => return DecorationPart::ResizeBottomLeft,
            (_, true, _, true) => return DecorationPart::ResizeBottomRight,
            _ => {}
        }
    }

    if geom.close_btn.contains(point) {
        return DecorationPart::CloseButton;
    }
    if geom.maximize_btn.contains(point) {
        return DecorationPart::MaximizeButton;
    }
    if geom.minimize_btn.contains(point) {
        return DecorationPart::MinimizeButton;
    }
    if has_titlebar && geom.drag.contains(point) {
        return DecorationPart::Titlebar;
    }
    if has_titlebar && geom.titlebar.contains(point) {
        return DecorationPart::None;
    }
    if geom.client.contains(point) {
        return DecorationPart::Client;
    }

    if within_frame {
        match (near_top, near_bottom, near_left, near_right) {
            (true, _, _, _) => return DecorationPart::ResizeTop,
            (_, true, _, _) => return DecorationPart::ResizeBottom,
            (_, _, true, _) => return DecorationPart::ResizeLeft,
            (_, _, _, true) => return DecorationPart::ResizeRight,
            _ => {}
        }
    }

    DecorationPart::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DecorationButton, DecorationButtonSide};

    const CLIENT: Rect = Rect {
        x: 100,
        y: 100,
        width: 300,
        height: 200,
    };

    fn theme(drag_margin: i32) -> DecorationTheme {
        DecorationTheme {
            titlebar_height: 30,
            border_width: 4,
            corner_radius: 0,
            popup_corner_radius: 0,
            titlebar_mode: crate::config::TitlebarColorMode::Theme,
            active_titlebar: [0; 4],
            inactive_titlebar: [0; 4],
            active_border: [0; 4],
            inactive_border: [0; 4],
            close_button: [0; 4],
            maximize_button: [0; 4],
            minimize_button: [0; 4],
            button_layout: vec![DecorationButton::Close],
            button_side: DecorationButtonSide::Right,
            title_centered: false,
            show_icon: false,
            drag_margin,
        }
    }

    #[test]
    fn edges_resize_just_outside_the_frame_even_without_a_border() {
        let mut theme = theme(0);
        theme.border_width = 0;
        let left_of_frame = Point {
            x: (CLIENT.x - 3) as f64,
            y: (CLIENT.y + 100) as f64,
        };
        assert_eq!(
            hit_test_frame(CLIENT, left_of_frame, &theme, true),
            DecorationPart::ResizeLeft
        );
        let far_away = Point {
            x: (CLIENT.x - 20) as f64,
            y: (CLIENT.y + 100) as f64,
        };
        assert_eq!(
            hit_test_frame(CLIENT, far_away, &theme, true),
            DecorationPart::None
        );
    }

    #[test]
    fn titlebar_center_is_draggable_without_a_margin() {
        let theme = theme(0);
        let point = Point {
            x: (CLIENT.x + CLIENT.width / 2) as f64,
            y: (CLIENT.y - 15) as f64,
        };
        assert_eq!(
            hit_test_frame(CLIENT, point, &theme, true),
            DecorationPart::Titlebar
        );
    }

    #[test]
    fn a_drag_margin_excludes_the_titlebar_edges_from_dragging() {
        let theme = theme(40);
        let near_left_edge = Point {
            x: (CLIENT.x + 5) as f64,
            y: (CLIENT.y - 15) as f64,
        };
        assert_eq!(
            hit_test_frame(CLIENT, near_left_edge, &theme, true),
            DecorationPart::None
        );
        let center = Point {
            x: (CLIENT.x + CLIENT.width / 2) as f64,
            y: (CLIENT.y - 15) as f64,
        };
        assert_eq!(
            hit_test_frame(CLIENT, center, &theme, true),
            DecorationPart::Titlebar
        );
    }
}
