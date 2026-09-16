use blair_protocol::Rect;

use super::theme::DecorationTheme;

#[derive(Debug, Clone, Copy)]
pub struct FrameGeometry {
    pub frame: Rect,
    pub titlebar: Rect,
    pub client: Rect,
    pub close_btn: Rect,
    pub maximize_btn: Rect,
    pub minimize_btn: Rect,
    pub border_top: Rect,
    pub border_bottom: Rect,
    pub border_left: Rect,
    pub border_right: Rect,
}

pub struct DecorationFrame;

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

        FrameGeometry {
            frame,
            titlebar,
            client,
            close_btn: Rect {
                x: titlebar.x + titlebar.width - btn_size - 8,
                y: btn_top,
                width: btn_size,
                height: btn_size,
            },
            maximize_btn: Rect {
                x: titlebar.x + titlebar.width - btn_size * 2 - 12,
                y: btn_top,
                width: btn_size,
                height: btn_size,
            },
            minimize_btn: Rect {
                x: titlebar.x + titlebar.width - btn_size * 3 - 16,
                y: btn_top,
                width: btn_size,
                height: btn_size,
            },
            border_top: Rect {
                x: frame.x,
                y: frame.y,
                width: frame.width,
                height: bw,
            },
            border_bottom: Rect {
                x: frame.x,
                y: frame.y + frame.height - bw,
                width: frame.width,
                height: bw,
            },
            border_left: Rect {
                x: frame.x,
                y: frame.y + bw,
                width: bw,
                height: frame.height - bw * 2,
            },
            border_right: Rect {
                x: frame.x + frame.width - bw,
                y: frame.y + bw,
                width: bw,
                height: frame.height - bw * 2,
            },
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
        }
    }

    #[test]
    fn border_wraps_the_titlebar_as_well_as_the_client() {
        let geom = DecorationFrame::compute(CLIENT, &theme(4), true);

        assert_eq!(geom.frame.y, CLIENT.y - 30 - 4);
        assert_eq!(geom.frame.x, CLIENT.x - 4);
        assert_eq!(geom.frame.width, CLIENT.width + 8);
        assert_eq!(geom.frame.height, CLIENT.height + 30 + 8);

        assert_eq!(geom.border_top.y, geom.frame.y);
        assert_eq!(geom.border_top.width, geom.frame.width);
        assert_eq!(geom.titlebar.x, CLIENT.x);
        assert_eq!(geom.titlebar.width, CLIENT.width);
        assert_eq!(geom.border_left.y, geom.frame.y + 4);
        assert_eq!(
            geom.border_left.height,
            geom.frame.height - geom.border_top.height - geom.border_bottom.height
        );
    }

    #[test]
    fn zero_border_width_collapses_border_pieces_but_keeps_the_titlebar() {
        let geom = DecorationFrame::compute(CLIENT, &theme(0), true);

        assert_eq!(geom.border_top.height, 0);
        assert_eq!(geom.border_left.width, 0);
        assert_eq!(geom.frame.y, CLIENT.y - 30);
        assert_eq!(geom.titlebar.height, 30);
    }

    #[test]
    fn without_a_titlebar_the_border_still_frames_the_client() {
        let geom = DecorationFrame::compute(CLIENT, &theme(4), false);

        assert_eq!(geom.titlebar.height, 0);
        assert_eq!(geom.frame.y, CLIENT.y - 4);
        assert_eq!(geom.frame.height, CLIENT.height + 8);
        assert_eq!(geom.border_top.y, geom.frame.y);
        assert_eq!(geom.border_top.width, geom.frame.width);
    }
}
