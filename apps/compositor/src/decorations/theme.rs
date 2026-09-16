#[derive(Debug, Clone, Copy)]
pub struct DecorationTheme {
    pub titlebar_height: i32,
    pub border_width: i32,
    pub active_titlebar: [u8; 4],
    pub inactive_titlebar: [u8; 4],
    pub active_border: [u8; 4],
    pub inactive_border: [u8; 4],
    pub close_button: [u8; 4],
    pub maximize_button: [u8; 4],
    pub minimize_button: [u8; 4],
}
