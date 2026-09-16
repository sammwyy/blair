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
    WindowSettings(oneshot::Sender<(i32, i32, i32, bool)>),
    LayoutSettings(oneshot::Sender<(String, i32, i32)>),
    SetLayoutSettings(String, i32, i32, oneshot::Sender<bool>),
    SetWindowSettings(i32, i32, i32, bool, oneshot::Sender<bool>),
    BindShortcut {
        client: String,
        id: String,
        accelerator: String,
        reply: oneshot::Sender<bool>,
    },
    UnbindShortcut {
        client: String,
        id: String,
    },
    ClientDisconnected(String),
    Quit,
}
