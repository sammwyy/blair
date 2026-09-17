#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub name: String,
    pub active: bool,
    pub window_count: usize,
}
