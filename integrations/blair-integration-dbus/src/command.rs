use blair_protocol::ShortcutBinding;
use tokio::sync::oneshot;

use crate::wire::{DbusWindow, DbusWorkspace};

pub enum Command {
    ListWindows(oneshot::Sender<Vec<DbusWindow>>),
    ListWorkspaces(oneshot::Sender<Vec<DbusWorkspace>>),
    CreateWorkspace(String, oneshot::Sender<u64>),
    SwitchWorkspace(u64, oneshot::Sender<bool>),
    MoveWindowToWorkspace(u64, u64, oneshot::Sender<bool>),
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
    Configuration(oneshot::Sender<String>),
    SetConfiguration(String, oneshot::Sender<bool>),
    ConfiguredShortcuts(oneshot::Sender<Vec<ShortcutBinding>>),
    SetConfiguredShortcuts(Vec<ShortcutBinding>, oneshot::Sender<bool>),
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
    RegisterWindowRule {
        client: String,
        id: String,
        rule_toml: String,
        reply: oneshot::Sender<bool>,
    },
    UnregisterWindowRule {
        client: String,
        id: String,
    },
    ClientDisconnected(String),
    Quit,
}
