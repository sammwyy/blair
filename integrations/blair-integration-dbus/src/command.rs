use std::{
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant},
};

use blair_protocol::{RenderStats, ShortcutBinding};
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use zbus::fdo;

use crate::wire::{DbusWindow, DbusWorkspace};

const COMMAND_CAPACITY: usize = 128;
const COMMAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const COMMAND_BATCH: usize = 16;
const COMMAND_TIME_SLICE: Duration = Duration::from_millis(2);

pub(crate) struct QueuedCommand {
    pub(crate) command: Command,
    _memory: OwnedSemaphorePermit,
}

#[derive(Clone)]
pub struct CommandSender {
    commands: mpsc::Sender<QueuedCommand>,
    memory: Arc<Semaphore>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl CommandSender {
    pub(crate) async fn closed(&self) {
        self.commands.closed().await;
    }

    pub(crate) fn channel(
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> (Self, mpsc::Receiver<QueuedCommand>) {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        (
            Self {
                commands,
                memory: Arc::new(Semaphore::new(COMMAND_BYTES)),
                wake,
            },
            receiver,
        )
    }

    fn bytes(command: &Command) -> fdo::Result<u32> {
        let bytes = command.retained_bytes();
        if bytes > MAX_COMMAND_BYTES {
            return Err(fdo::Error::LimitsExceeded(
                "Compositor request exceeds 1 MiB".into(),
            ));
        }
        Ok(bytes as u32)
    }

    pub fn send(&self, command: Command) -> fdo::Result<()> {
        let bytes = Self::bytes(&command)?;
        let memory = self
            .memory
            .clone()
            .try_acquire_many_owned(bytes)
            .map_err(|_| fdo::Error::LimitsExceeded("Compositor request queue is full".into()))?;
        self.commands
            .try_send(QueuedCommand {
                command,
                _memory: memory,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    fdo::Error::LimitsExceeded("Compositor request queue is full".into())
                }
                mpsc::error::TrySendError::Closed(_) => {
                    fdo::Error::Failed("Compositor stopped".into())
                }
            })?;
        (self.wake)();
        Ok(())
    }

    pub(crate) async fn send_control(&self, command: Command) -> fdo::Result<()> {
        let bytes = Self::bytes(&command)?;
        let memory = self
            .memory
            .clone()
            .acquire_many_owned(bytes)
            .await
            .map_err(|_| fdo::Error::Failed("Compositor stopped".into()))?;
        self.commands
            .send(QueuedCommand {
                command,
                _memory: memory,
            })
            .await
            .map_err(|_| fdo::Error::Failed("Compositor stopped".into()))?;
        (self.wake)();
        Ok(())
    }
}

pub(crate) fn dispatch_batch(
    commands: &mut mpsc::Receiver<QueuedCommand>,
    wake: &dyn Fn(),
    mut dispatch: impl FnMut(Command),
) {
    let started = Instant::now();
    for _ in 0..COMMAND_BATCH {
        let Ok(queued) = commands.try_recv() else {
            return;
        };
        let QueuedCommand { command, _memory } = queued;
        drop(_memory);
        dispatch(command);
        if started.elapsed() >= COMMAND_TIME_SLICE {
            break;
        }
    }
    if !commands.is_empty() {
        wake();
    }
}

pub enum Command {
    DisplayInfo(oneshot::Sender<Vec<crate::wire::DbusDisplay>>),
    ApplyDisplayMode(
        String,
        blair_protocol::DisplayMode,
        oneshot::Sender<Result<(), String>>,
    ),
    ConfirmDisplayMode(String, oneshot::Sender<Result<(), String>>),
    RevertDisplayMode(String, oneshot::Sender<Result<(), String>>),
    ListWindows(oneshot::Sender<Vec<DbusWindow>>),
    ListWorkspaces(oneshot::Sender<Vec<DbusWorkspace>>),
    CreateWorkspace(String, oneshot::Sender<u64>),
    DestroyWorkspace(u64, oneshot::Sender<bool>),
    WorkspaceCapabilities(oneshot::Sender<(bool, bool)>),
    SwitchWorkspace(u64, oneshot::Sender<bool>),
    MoveWindowToWorkspace(u64, u64, oneshot::Sender<bool>),
    FocusWindow(u64, oneshot::Sender<bool>),
    CloseWindow(u64, oneshot::Sender<bool>),
    MinimizeWindow(u64, oneshot::Sender<bool>),
    ToggleMaximizeWindow(u64, oneshot::Sender<bool>),
    MoveResizeWindow(u64, (i32, i32, i32, i32), oneshot::Sender<bool>),
    WorkArea(String, oneshot::Sender<(i32, i32, i32, i32)>),
    Outputs(oneshot::Sender<Vec<String>>),
    RenderStats(oneshot::Sender<RenderStats>),
    Screenshot {
        output: String,
        path: String,
        reply: oneshot::Sender<bool>,
    },
    WindowSettings(oneshot::Sender<(i32, i32, i32, bool)>),
    LayoutSettings(oneshot::Sender<(String, i32, i32)>),
    SetLayoutSettings(String, i32, i32, oneshot::Sender<bool>),
    SetWindowSettings(i32, i32, i32, bool, oneshot::Sender<bool>),
    Configuration(oneshot::Sender<String>),
    InputCapabilities(oneshot::Sender<crate::wire::DbusInputCapabilities>),
    PrepareForSleep,
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
    ThemeChanged,
    ScreenCapturePolicy(oneshot::Sender<blair_integration::CapturePolicy>),
    Quit,
}

impl Command {
    fn retained_bytes(&self) -> usize {
        let payload = match self {
            Self::CreateWorkspace(value, _)
            | Self::WorkArea(value, _)
            | Self::ConfirmDisplayMode(value, _)
            | Self::RevertDisplayMode(value, _)
            | Self::ApplyDisplayMode(value, _, _)
            | Self::SetConfiguration(value, _)
            | Self::SetLayoutSettings(value, _, _, _)
            | Self::ClientDisconnected(value) => value.capacity(),
            Self::Screenshot { output, path, .. } => {
                output.capacity().saturating_add(path.capacity())
            }
            Self::SetConfiguredShortcuts(bindings, _) => bindings.iter().fold(
                bindings
                    .capacity()
                    .saturating_mul(size_of::<ShortcutBinding>()),
                |total, binding| {
                    total
                        .saturating_add(binding.accelerator.capacity())
                        .saturating_add(binding.argument.as_ref().map_or(0, String::capacity))
                },
            ),
            Self::BindShortcut {
                client,
                id,
                accelerator,
                ..
            } => client
                .capacity()
                .saturating_add(id.capacity())
                .saturating_add(accelerator.capacity()),
            Self::RegisterWindowRule {
                client,
                id,
                rule_toml,
                ..
            } => client
                .capacity()
                .saturating_add(id.capacity())
                .saturating_add(rule_toml.capacity()),
            Self::UnbindShortcut { client, id } | Self::UnregisterWindowRule { client, id } => {
                client.capacity().saturating_add(id.capacity())
            }
            Self::ScreenCapturePolicy(_)
            | Self::DisplayInfo(_)
            | Self::ListWindows(_)
            | Self::ListWorkspaces(_)
            | Self::DestroyWorkspace(_, _)
            | Self::WorkspaceCapabilities(_)
            | Self::SwitchWorkspace(_, _)
            | Self::MoveWindowToWorkspace(_, _, _)
            | Self::FocusWindow(_, _)
            | Self::CloseWindow(_, _)
            | Self::MinimizeWindow(_, _)
            | Self::ToggleMaximizeWindow(_, _)
            | Self::MoveResizeWindow(_, _, _)
            | Self::Outputs(_)
            | Self::RenderStats(_)
            | Self::WindowSettings(_)
            | Self::LayoutSettings(_)
            | Self::SetWindowSettings(_, _, _, _, _)
            | Self::Configuration(_)
            | Self::InputCapabilities(_)
            | Self::PrepareForSleep
            | Self::ConfiguredShortcuts(_)
            | Self::ThemeChanged
            | Self::Quit => 0,
        };
        size_of::<QueuedCommand>().saturating_add(payload)
    }
}

#[cfg(test)]
mod tests;
