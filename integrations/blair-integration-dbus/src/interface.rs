use tokio::sync::oneshot;
use zbus::{interface, message::Header, object_server::SignalEmitter};

use crate::{
    command::{Command, CommandSender},
    wire::{DbusWindow, DbusWorkspace},
};
use blair_protocol::{ShortcutBinding, ShortcutCommand};

pub struct CompositorInterface {
    commands: CommandSender,
}

impl CompositorInterface {
    pub fn new(commands: CommandSender) -> Self {
        Self { commands }
    }

    async fn call<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> Command) -> Option<T> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self.commands.send(build(reply_tx)) {
            return None;
        }
        reply_rx.await.ok()
    }
}

#[interface(
    interface = "org.blair.Compositor1",
    proxy(
        async_name = "CompositorProxy",
        gen_blocking = false,
        assume_defaults = true,
        default_service = "org.blair.Compositor",
        default_path = "/org/blair/Compositor",
        visibility = "pub",
    )
)]
impl CompositorInterface {
    async fn list_windows(&self) -> Vec<DbusWindow> {
        self.call(Command::ListWindows).await.unwrap_or_default()
    }

    async fn list_workspaces(&self) -> Vec<DbusWorkspace> {
        self.call(Command::ListWorkspaces).await.unwrap_or_default()
    }

    async fn create_workspace(&self, name: &str) -> u64 {
        self.call(|reply| Command::CreateWorkspace(name.to_owned(), reply))
            .await
            .unwrap_or_default()
    }

    async fn switch_workspace(&self, id: u64) -> bool {
        self.call(|reply| Command::SwitchWorkspace(id, reply))
            .await
            .unwrap_or(false)
    }

    async fn move_window_to_workspace(&self, window_id: u64, workspace_id: u64) -> bool {
        self.call(|reply| Command::MoveWindowToWorkspace(window_id, workspace_id, reply))
            .await
            .unwrap_or(false)
    }

    async fn focus_window(&self, id: u64) -> bool {
        self.call(|reply| Command::FocusWindow(id, reply))
            .await
            .unwrap_or(false)
    }

    async fn close_window(&self, id: u64) -> bool {
        self.call(|reply| Command::CloseWindow(id, reply))
            .await
            .unwrap_or(false)
    }

    async fn minimize_window(&self, id: u64) -> bool {
        self.call(|reply| Command::MinimizeWindow(id, reply))
            .await
            .unwrap_or(false)
    }

    async fn toggle_maximize_window(&self, id: u64) -> bool {
        self.call(|reply| Command::ToggleMaximizeWindow(id, reply))
            .await
            .unwrap_or(false)
    }

    async fn move_resize_window(&self, id: u64, x: i32, y: i32, width: i32, height: i32) -> bool {
        self.call(|reply| Command::MoveResizeWindow(id, (x, y, width, height), reply))
            .await
            .unwrap_or(false)
    }

    async fn work_area(&self, output: &str) -> (i32, i32, i32, i32) {
        self.call(|reply| Command::WorkArea(output.to_owned(), reply))
            .await
            .unwrap_or_default()
    }

    async fn outputs(&self) -> Vec<String> {
        self.call(Command::Outputs).await.unwrap_or_default()
    }

    /// Writes a PNG screenshot of `output` (empty selects the focused one)
    /// to the absolute `path`.
    async fn screenshot(&self, output: &str, path: &str) -> bool {
        self.call(|reply| Command::Screenshot {
            output: output.to_owned(),
            path: path.to_owned(),
            reply,
        })
        .await
        .unwrap_or(false)
    }

    /// Returns `(frames, empty_frames, fps, build_ms, render_ms,
    /// render_max_ms, present_interval_ms)` for the last reporting interval.
    async fn render_stats(&self) -> (u64, u64, f64, f64, f64, f64, f64) {
        let stats = self.call(Command::RenderStats).await.unwrap_or_default();
        (
            stats.frames,
            stats.empty_frames,
            stats.fps,
            stats.build_ms_avg,
            stats.render_ms_avg,
            stats.render_ms_max,
            stats.present_interval_ms_avg,
        )
    }

    async fn window_settings(&self) -> (i32, i32, i32, bool) {
        self.call(Command::WindowSettings).await.unwrap_or_default()
    }

    async fn layout_settings(&self) -> (String, i32, i32) {
        self.call(Command::LayoutSettings).await.unwrap_or_default()
    }

    async fn set_layout_settings(
        &self,
        layout: &str,
        work_area_padding: i32,
        corner_radius: i32,
    ) -> bool {
        self.call(|reply| {
            Command::SetLayoutSettings(layout.to_owned(), work_area_padding, corner_radius, reply)
        })
        .await
        .unwrap_or(false)
    }

    async fn set_window_settings(
        &self,
        titlebar_height: i32,
        border_width: i32,
        corner_radius: i32,
        server_side_decorations: bool,
    ) -> bool {
        self.call(|reply| {
            Command::SetWindowSettings(
                titlebar_height,
                border_width,
                corner_radius,
                server_side_decorations,
                reply,
            )
        })
        .await
        .unwrap_or(false)
    }

    async fn configuration(&self) -> String {
        self.call(Command::Configuration).await.unwrap_or_default()
    }

    async fn set_configuration(&self, configuration: &str) -> bool {
        self.call(|reply| Command::SetConfiguration(configuration.to_owned(), reply))
            .await
            .unwrap_or(false)
    }

    /// Returns `(accelerator, command, argument)` records.  Command names
    /// are defined by `blair-protocol::ShortcutCommand`.
    async fn configured_shortcuts(&self) -> Vec<(String, String, String)> {
        self.call(Command::ConfiguredShortcuts)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|binding| {
                (
                    binding.accelerator,
                    binding.command.id().to_owned(),
                    binding.argument.unwrap_or_default(),
                )
            })
            .collect()
    }

    async fn set_configured_shortcuts(&self, bindings: Vec<(String, String, String)>) -> bool {
        let mut parsed = Vec::with_capacity(bindings.len());
        for (accelerator, command, argument) in bindings {
            let Some(command) = ShortcutCommand::parse(&command) else {
                return false;
            };
            parsed.push(ShortcutBinding {
                accelerator,
                command,
                argument: (!argument.trim().is_empty()).then_some(argument),
            });
        }
        self.call(|reply| Command::SetConfiguredShortcuts(parsed, reply))
            .await
            .unwrap_or(false)
    }

    async fn bind_shortcut(
        &self,
        id: &str,
        accelerator: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> bool {
        let Some(client) = header.sender() else {
            return false;
        };
        self.call(|reply| Command::BindShortcut {
            client: client.to_string(),
            id: id.to_owned(),
            accelerator: accelerator.to_owned(),
            reply,
        })
        .await
        .unwrap_or(false)
    }

    async fn unbind_shortcut(&self, id: &str, #[zbus(header)] header: Header<'_>) {
        let Some(client) = header.sender() else {
            return;
        };
        self.commands.send(Command::UnbindShortcut {
            client: client.to_string(),
            id: id.to_owned(),
        });
    }

    /// Adds a process-owned temporary rule. `rule_toml` is one `[[rules]]`
    /// table without its array-table header.
    async fn register_window_rule(
        &self,
        id: &str,
        rule_toml: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> bool {
        let Some(client) = header.sender() else {
            return false;
        };
        self.call(|reply| Command::RegisterWindowRule {
            client: client.to_string(),
            id: id.to_owned(),
            rule_toml: rule_toml.to_owned(),
            reply,
        })
        .await
        .unwrap_or(false)
    }

    async fn unregister_window_rule(&self, id: &str, #[zbus(header)] header: Header<'_>) {
        let Some(client) = header.sender() else {
            return;
        };
        self.commands.send(Command::UnregisterWindowRule {
            client: client.to_string(),
            id: id.to_owned(),
        });
    }

    async fn quit(&self) {
        self.commands.send(Command::Quit);
    }

    #[zbus(signal)]
    pub async fn window_opened(
        emitter: &SignalEmitter<'_>,
        id: u64,
        title: &str,
        app_id: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn window_closed(emitter: &SignalEmitter<'_>, id: u64) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn window_focused(emitter: &SignalEmitter<'_>, id: u64) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn focus_cleared(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn window_app_id_changed(
        emitter: &SignalEmitter<'_>,
        id: u64,
        app_id: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn window_title_changed(
        emitter: &SignalEmitter<'_>,
        id: u64,
        title: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn window_geometry_changed(
        emitter: &SignalEmitter<'_>,
        id: u64,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn window_minimized(emitter: &SignalEmitter<'_>, id: u64) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn window_restored(emitter: &SignalEmitter<'_>, id: u64) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn window_maximized(
        emitter: &SignalEmitter<'_>,
        id: u64,
        maximized: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn output_added(emitter: &SignalEmitter<'_>, name: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn output_removed(emitter: &SignalEmitter<'_>, name: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn work_area_changed(
        emitter: &SignalEmitter<'_>,
        output: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn workspace_activated(
        emitter: &SignalEmitter<'_>,
        output: &str,
        id: u64,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn workspaces_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn configuration_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn shortcut_activated(emitter: &SignalEmitter<'_>, id: &str) -> zbus::Result<()>;
}
