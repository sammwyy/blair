use blair_protocol::Rect;

use super::theme::DecorationTheme;
use crate::config::{DecorationButton, DecorationButtonSide};

#[derive(Debug, Clone, Copy)]
pub struct FrameGeometry {
    pub frame: Rect,
    pub titlebar: Rect,
    pub client: Rect,
    pub close_btn: Rect,
    pub maximize_btn: Rect,
    pub minimize_btn: Rect,
    pub icon: Rect,
    pub title: Rect,
    pub drag: Rect,
}

pub struct DecorationFrame;

const ICON_PADDING: i32 = 6;
const TITLE_PADDING: i32 = 8;

impl DecorationFrame {
    /// Lays out the frame so the themed border traces the outer edge of the
    /// whole window — titlebar included — with the titlebar and client inset
    /// by `border_width` on top/left/right, matching the client's own width.
    /// `has_titlebar` selects whether a titlebar row is reserved above the
    /// client (tiled windows keep the border but drop the titlebar).
    pub fn compute(client: Rect, theme: &DecorationTheme, has_titlebar: bool) -> FrameGeometry {
        let bw = theme.border_width;
        let th = if has_titlebar {
            theme.titlebar_height
        } else {
            0
        };
        let btn_size = theme.titlebar_height - 8;
        let btn_top = client.y - th + 4;

        let frame = Rect {
            x: client.x - bw,
            y: client.y - th - bw,
            width: client.width + bw * 2,
            height: client.height + th + bw * 2,
        };

        let titlebar = Rect {
            x: client.x,
            y: client.y - th,
            width: client.width,
            height: th,
        };

        let icon_size = (th - ICON_PADDING * 2).max(0);
        let icon = if has_titlebar && theme.show_icon && icon_size > 0 {
            Rect {
                x: titlebar.x + ICON_PADDING,
                y: titlebar.y + ICON_PADDING,
                width: icon_size,
                height: icon_size,
            }
        } else {
            Rect::default()
        };
        // The icon always sits at the very left of the titlebar, ahead of
        // the buttons even when they are also on the left side.
        let icon_reserved = if theme.show_icon && icon_size > 0 {
            icon_size + ICON_PADDING * 2
        } else {
            TITLE_PADDING
        };

        let mut close_btn = Rect::default();
        let mut maximize_btn = Rect::default();
        let mut minimize_btn = Rect::default();
        let gap = 4;
        let count = theme.button_layout.len() as i32;
        let buttons_width = if count > 0 {
            count * btn_size + (count - 1) * gap + TITLE_PADDING
        } else {
            0
        };
        let left_anchor = titlebar.x + icon_reserved;
        for (index, button) in theme.button_layout.iter().enumerate() {
            let index = index as i32;
            let x = match theme.button_side {
                DecorationButtonSide::Left => left_anchor + index * (btn_size + gap),
                DecorationButtonSide::Right => {
                    titlebar.x + titlebar.width
                        - TITLE_PADDING
                        - (count - index) * btn_size
                        - (count - index - 1) * gap
                }
            };
            let rect = Rect {
                x,
                y: btn_top,
                width: btn_size,
                height: btn_size,
            };
            match button {
                DecorationButton::Close => close_btn = rect,
                DecorationButton::Maximize => maximize_btn = rect,
                DecorationButton::Minimize => minimize_btn = rect,
            }
        }

        let title_left = if matches!(theme.button_side, DecorationButtonSide::Left) {
            left_anchor + buttons_width
        } else {
            left_anchor
        };
        let title_right = if matches!(theme.button_side, DecorationButtonSide::Right) {
            titlebar.x + titlebar.width - buttons_width
        } else {
            titlebar.x + titlebar.width - TITLE_PADDING
        };
        let title = Rect {
            x: title_left,
            y: titlebar.y,
            width: (title_right - title_left).max(0),
            height: titlebar.height,
        };

        let drag_margin = theme.drag_margin.max(0);
        let drag = Rect {
            x: titlebar.x + drag_margin,
            y: titlebar.y,
            width: (titlebar.width - drag_margin * 2).max(0),
            height: titlebar.height,
        };

        FrameGeometry {
            frame,
            titlebar,
            client,
            close_btn,
            maximize_btn,
            minimize_btn,
            icon,
            title,
            drag,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: Rect = Rect {
        x: 100,
        y: 100,
        width: 300,
        height: 200,
    };

    fn theme(border_width: i32) -> DecorationTheme {
        DecorationTheme {
            titlebar_height: 30,
            border_width,
            active_titlebar: [0; 4],
            inactive_titlebar: [0; 4],
            active_border: [0; 4],
            inactive_border: [0; 4],
            close_button: [0; 4],
            maximize_button: [0; 4],
            minimize_button: [0; 4],
            active_title_text: [0; 4],
            inactive_title_text: [0; 4],
            button_layout: vec![
                DecorationButton::Minimize,
                DecorationButton::Maximize,
                DecorationButton::Close,
            ],
            button_side: DecorationButtonSide::Right,
            title_centered: false,
            show_icon: true,
            drag_margin: 0,
        }
    }

    #[test]
    fn frame_wraps_the_titlebar_as_well_as_the_client() {
        let geom = DecorationFrame::compute(CLIENT, &theme(4), true);

        assert_eq!(geom.frame.y, CLIENT.y - 30 - 4);
        assert_eq!(geom.frame.x, CLIENT.x - 4);
        assert_eq!(geom.frame.width, CLIENT.width + 8);
        assert_eq!(geom.frame.height, CLIENT.height + 30 + 8);
        assert_eq!(geom.titlebar.x, CLIENT.x);
        assert_eq!(geom.titlebar.width, CLIENT.width);
    }

    #[test]
    fn zero_border_width_keeps_the_titlebar() {
        let geom = DecorationFrame::compute(CLIENT, &theme(0), true);

        assert_eq!(geom.frame.y, CLIENT.y - 30);
        assert_eq!(geom.titlebar.height, 30);
    }

    #[test]
    fn without_a_titlebar_the_frame_only_grows_by_the_border() {
        let geom = DecorationFrame::compute(CLIENT, &theme(4), false);

        assert_eq!(geom.titlebar.height, 0);
        assert_eq!(geom.frame.y, CLIENT.y - 4);
        assert_eq!(geom.frame.height, CLIENT.height + 8);
    }

    #[test]
    fn buttons_sit_inside_the_titlebar_on_the_configured_side() {
        let geom = DecorationFrame::compute(CLIENT, &theme(4), true);
        let titlebar_end = geom.titlebar.x + geom.titlebar.width;

        assert!(geom.close_btn.x + geom.close_btn.width <= titlebar_end);
        assert!(geom.minimize_btn.x < geom.maximize_btn.x);
        assert!(geom.maximize_btn.x < geom.close_btn.x);
        assert_eq!(geom.close_btn.y, CLIENT.y - 30 + 4);
    }

    #[test]
    fn icon_sits_at_the_very_left_regardless_of_button_side() {
        let mut theme = theme(4);
        theme.button_side = DecorationButtonSide::Left;
        let geom = DecorationFrame::compute(CLIENT, &theme, true);

        assert_eq!(geom.icon.x, geom.titlebar.x + ICON_PADDING);
        assert!(geom.title.x > geom.minimize_btn.x + geom.minimize_btn.width);
    }

    #[test]
    fn left_side_buttons_never_overlap_the_icon() {
        let mut theme = theme(4);
        theme.button_side = DecorationButtonSide::Left;
        let geom = DecorationFrame::compute(CLIENT, &theme, true);

        assert!(geom.minimize_btn.x >= geom.icon.x + geom.icon.width);
    }

    #[test]
    fn hiding_the_icon_gives_its_space_to_the_title() {
        let mut theme = theme(4);
        theme.show_icon = false;
        let geom = DecorationFrame::compute(CLIENT, &theme, true);

        assert_eq!(geom.icon, Rect::default());
        assert!(geom.title.x < geom.titlebar.x + ICON_PADDING * 2);
    }

    #[test]
    fn drag_margin_shrinks_the_draggable_region_symmetrically() {
        let mut theme = theme(4);
        theme.drag_margin = 10;
        let geom = DecorationFrame::compute(CLIENT, &theme, true);

        assert_eq!(geom.drag.x, geom.titlebar.x + 10);
        assert_eq!(geom.drag.width, geom.titlebar.width - 20);
    }
}
