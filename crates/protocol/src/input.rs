#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputCapabilities {
    pub backend: String,
    pub devices: Vec<InputDeviceInfo>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputDeviceInfo {
    pub id: String,
    pub name: String,
    pub touchpad: bool,
    pub sensitivity: bool,
    pub acceleration_profiles: Vec<String>,
    pub tap: bool,
    pub natural_scroll: bool,
    pub disable_while_typing: bool,
}
