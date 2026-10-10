use blair_protocol::DisplayMode;

pub enum DisplayAction {
    Apply(DisplayMode),
    Confirm,
    Revert,
}

pub struct DisplayRequest {
    pub output: String,
    pub action: DisplayAction,
    pub reply: Box<dyn FnOnce(Result<(), String>) + Send>,
}
