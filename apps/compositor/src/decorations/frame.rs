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
    pub border_bottom: Rect,
    pub border_left: Rect,
    pub border_right: Rect,
}

pub struct DecorationFrame;

impl DecorationFrame {
    pub fn compute(client: Rect, theme: &DecorationTheme) -> FrameGeometry {
        let bw = theme.border_width;
        let th = theme.titlebar_height;
        let btn_size = th - 8;
        let btn_top = client.y - th + 4;

        let frame = Rect {
            x: client.x - bw,
            y: client.y - th,
            width: client.width + bw * 2,
            height: client.height + th + bw,
        };

        FrameGeometry {
            frame,
            titlebar: Rect {
                x: frame.x,
                y: frame.y,
                width: frame.width,
                height: th,
            },
            client,
            close_btn: Rect {
                x: frame.x + frame.width - bw - btn_size - 8,
                y: btn_top,
                width: btn_size,
                height: btn_size,
            },
            maximize_btn: Rect {
                x: frame.x + frame.width - bw - btn_size * 2 - 12,
                y: btn_top,
                width: btn_size,
                height: btn_size,
            },
            minimize_btn: Rect {
                x: frame.x + frame.width - bw - btn_size * 3 - 16,
                y: btn_top,
                width: btn_size,
                height: btn_size,
            },
            border_bottom: Rect {
                x: client.x,
                y: client.y + client.height,
                width: client.width,
                height: bw,
            },
            border_left: Rect {
                x: frame.x,
                y: client.y,
                width: bw,
                height: client.height,
            },
            border_right: Rect {
                x: client.x + client.width,
                y: client.y,
                width: bw,
                height: client.height,
            },
        }
    }
}
