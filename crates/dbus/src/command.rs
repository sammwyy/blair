use tokio::sync::oneshot;

use crate::wire::DbusWindow;

pub enum Command {
    ListWindows(oneshot::Sender<Vec<DbusWindow>>),
    FocusWindow(u64, oneshot::Sender<bool>),
    CloseWindow(u64, oneshot::Sender<bool>),
    MinimizeWindow(u64, oneshot::Sender<bool>),
    ToggleMaximizeWindow(u64, oneshot::Sender<bool>),
    MoveResizeWindow(u64, (i32, i32, i32, i32), oneshot::Sender<bool>),
    WorkArea(String, oneshot::Sender<(i32, i32, i32, i32)>),
    Outputs(oneshot::Sender<Vec<String>>),
    BindShortcut(String, String, oneshot::Sender<bool>),
    UnbindShortcut(String),
    Quit,
}
