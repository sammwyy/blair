#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub name: String,
    pub active: bool,
    /// Output currently displaying this workspace, if any.
    pub output: Option<String>,
    pub window_count: usize,
}
