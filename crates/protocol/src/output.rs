/// A physical display mode. Refresh rates are expressed in millihertz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: i32,
    pub height: i32,
    pub refresh_millihz: i32,
}

impl DisplayMode {
    pub fn config_value(self) -> String {
        format!(
            "{}x{}@{:.3}",
            self.width,
            self.height,
            self.refresh_millihz as f64 / 1000.0
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub name: String,
    pub current: DisplayMode,
    pub modes: Vec<DisplayMode>,
    pub can_change_mode: bool,
    pub pending_confirmation: bool,
    pub confirmation_seconds: u32,
}
