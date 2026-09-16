use crate::{Rect, WindowId};

#[derive(Debug, Clone, PartialEq)]
pub enum CompositorEvent {
    WindowOpened {
        id: WindowId,
        title: String,
        app_id: Option<String>,
    },
    WindowClosed {
        id: WindowId,
    },
    WindowFocused {
        id: WindowId,
    },
    FocusCleared,
    WindowTitleChanged {
        id: WindowId,
        title: String,
    },
    WindowGeometryChanged {
        id: WindowId,
        geometry: Rect,
    },
    WindowMinimized {
        id: WindowId,
    },
    WindowRestored {
        id: WindowId,
    },
    WindowMaximized {
        id: WindowId,
        maximized: bool,
    },
    OutputAdded {
        name: String,
    },
    OutputRemoved {
        name: String,
    },
    WorkAreaChanged {
        output: String,
        area: Rect,
    },
    ShortcutActivated {
        id: String,
    },
}
