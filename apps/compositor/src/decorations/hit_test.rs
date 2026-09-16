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

const GRAB_MARGIN: f64 = 8.0;

pub fn hit_test_frame(client: Rect, point: Point, theme: &DecorationTheme) -> DecorationPart {
    let geom = DecorationFrame::compute(client, theme, true);

    let fx = geom.frame.x as f64;
    let fy = geom.frame.y as f64;
    let fw = geom.frame.width as f64;
    let fh = geom.frame.height as f64;
    let within_frame = point.x >= fx && point.x <= fx + fw && point.y >= fy && point.y <= fy + fh;
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
    if geom.titlebar.contains(point) {
        return DecorationPart::Titlebar;
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
