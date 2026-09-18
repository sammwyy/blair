use crate::config::{DecorationButton, DecorationButtonSide, TitlebarColorMode};

/// Decoration settings resolved against the system theme: every color here
/// is final, and `border_width` is already 0 when the border is hidden.
#[derive(Debug, Clone)]
pub struct DecorationTheme {
    pub titlebar_height: i32,
    pub border_width: i32,
    pub corner_radius: i32,
    pub titlebar_mode: TitlebarColorMode,
    pub active_titlebar: [u8; 4],
    pub inactive_titlebar: [u8; 4],
    pub active_border: [u8; 4],
    pub inactive_border: [u8; 4],
    pub close_button: [u8; 4],
    pub maximize_button: [u8; 4],
    pub minimize_button: [u8; 4],
    pub button_layout: Vec<DecorationButton>,
    pub button_side: DecorationButtonSide,
    pub title_centered: bool,
    pub show_icon: bool,
    pub drag_margin: i32,
}
