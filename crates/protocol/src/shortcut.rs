/// A compositor action available to a persistent keyboard shortcut.
///
/// This lives in the protocol crate so clients can build a correct editor
/// without duplicating the compositor's action list or argument rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutCommand {
    Close,
    Workspace,
    MoveToWorkspace,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutArgument {
    None,
    Workspace,
    Command,
}

impl ShortcutCommand {
    pub const ALL: [Self; 4] = [
        Self::Close,
        Self::Workspace,
        Self::MoveToWorkspace,
        Self::Execute,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Workspace => "workspace",
            Self::MoveToWorkspace => "move-to-workspace",
            Self::Execute => "exec",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Close => "Close focused window",
            Self::Workspace => "Switch workspace",
            Self::MoveToWorkspace => "Move window to workspace",
            Self::Execute => "Run command",
        }
    }

    pub const fn argument(self) -> ShortcutArgument {
        match self {
            Self::Close => ShortcutArgument::None,
            Self::Workspace | Self::MoveToWorkspace => ShortcutArgument::Workspace,
            Self::Execute => ShortcutArgument::Command,
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.id() == id)
    }
}

/// Client-facing representation of a configured shortcut. It deliberately
/// has no TOML concerns: Blair is responsible for translating and persisting
/// it in its own configuration format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutBinding {
    pub accelerator: String,
    pub command: ShortcutCommand,
    pub argument: Option<String>,
}
