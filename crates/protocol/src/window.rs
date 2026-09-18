use std::fmt;

use crate::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u64);

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    pub app_id: Option<String>,
    pub geometry: Rect,
    pub focused: bool,
    pub minimized: bool,
    pub maximized: bool,
}
