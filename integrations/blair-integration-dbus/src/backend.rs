use std::{
    sync::{
        mpsc::{self, Receiver},
        Arc,
    },
    thread::JoinHandle,
};

use blair_integration::{CompositorApi, EventChannel, Transport};
use blair_protocol::{Rect, WindowId};

use crate::{
    command::{Command, CommandSender},
    wire::{DbusWindow, DbusWorkspace},
};

pub struct DbusIntegration {
    commands: Receiver<Command>,
    events: Arc<DbusEventChannel>,
    _thread: JoinHandle<()>,
}

struct DbusEventChannel(std::sync::mpsc::Sender<blair_protocol::CompositorEvent>);

impl EventChannel for DbusEventChannel {
    fn publish(&self, event: blair_protocol::CompositorEvent) {
        let _ = self.0.send(event);
    }
}

impl DbusIntegration {
    /// `wake` is called after every queued request so the compositor's event
    /// loop picks it up without polling.
    pub fn start(wake: Arc<dyn Fn() + Send + Sync>, environment: Vec<(String, String)>) -> Self {
        let (commands_tx, commands) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        let thread = crate::server::serve(
            CommandSender::new(commands_tx, wake),
            events_rx,
            environment,
        );
        Self {
            commands,
            events: Arc::new(DbusEventChannel(events_tx)),
            _thread: thread,
        }
    }

    pub fn event_channel(&self) -> Arc<dyn EventChannel> {
        self.events.clone()
    }
}

impl Transport for DbusIntegration {
    fn drain(&mut self, compositor: &mut dyn CompositorApi) {
        drain(&self.commands, compositor);
    }
}

/// Drains pending D-Bus method calls.
fn drain(commands: &Receiver<Command>, backend: &mut dyn CompositorApi) {
    while let Ok(command) = commands.try_recv() {
        match command {
            Command::ListWindows(reply) => {
                let windows = backend
                    .list_windows()
                    .iter()
                    .map(DbusWindow::from)
                    .collect();
                let _ = reply.send(windows);
            }
            Command::ListWorkspaces(reply) => {
                let workspaces = backend
                    .list_workspaces()
                    .iter()
                    .map(DbusWorkspace::from)
                    .collect();
                let _ = reply.send(workspaces);
            }
            Command::CreateWorkspace(name, reply) => {
                let _ = reply.send(backend.create_workspace(name));
            }
            Command::SwitchWorkspace(id, reply) => {
                let _ = reply.send(backend.switch_workspace(id));
            }
            Command::MoveWindowToWorkspace(window, workspace, reply) => {
                let _ = reply.send(backend.move_window_to_workspace(WindowId(window), workspace));
            }
            Command::FocusWindow(id, reply) => {
                let _ = reply.send(backend.focus_window(WindowId(id)));
            }
            Command::CloseWindow(id, reply) => {
                let _ = reply.send(backend.close_window(WindowId(id)));
            }
            Command::MinimizeWindow(id, reply) => {
                let _ = reply.send(backend.minimize_window(WindowId(id)));
            }
            Command::ToggleMaximizeWindow(id, reply) => {
                let _ = reply.send(backend.toggle_maximize_window(WindowId(id)));
            }
            Command::MoveResizeWindow(id, (x, y, width, height), reply) => {
                let geometry = Rect {
                    x,
                    y,
                    width,
                    height,
                };
                let _ = reply.send(backend.move_resize_window(WindowId(id), geometry));
            }
            Command::WorkArea(output, reply) => {
                let area = backend.work_area(&output);
                let _ = reply.send((area.x, area.y, area.width, area.height));
            }
            Command::Outputs(reply) => {
                let _ = reply.send(backend.outputs());
            }
            Command::RenderStats(reply) => {
                let _ = reply.send(backend.render_stats());
            }
            Command::Screenshot {
                output,
                path,
                reply,
            } => backend.screenshot(
                &output,
                &path,
                Box::new(move |written| {
                    let _ = reply.send(written);
                }),
            ),
            Command::WindowSettings(reply) => {
                let _ = reply.send(backend.window_settings());
            }
            Command::LayoutSettings(reply) => {
                let _ = reply.send(backend.layout_settings());
            }
            Command::SetLayoutSettings(layout, padding, radius, reply) => {
                let _ = reply.send(backend.set_layout_settings(&layout, padding, radius));
            }
            Command::SetWindowSettings(
                titlebar_height,
                border_width,
                corner_radius,
                server_side_decorations,
                reply,
            ) => {
                let _ = reply.send(backend.set_window_settings(
                    titlebar_height,
                    border_width,
                    corner_radius,
                    server_side_decorations,
                ));
            }
            Command::Configuration(reply) => {
                let _ = reply.send(backend.configuration());
            }
            Command::SetConfiguration(configuration, reply) => {
                let _ = reply.send(backend.set_configuration(&configuration));
            }
            Command::ConfiguredShortcuts(reply) => {
                let _ = reply.send(backend.configured_shortcuts());
            }
            Command::SetConfiguredShortcuts(bindings, reply) => {
                let _ = reply.send(backend.set_configured_shortcuts(bindings));
            }
            Command::BindShortcut {
                client,
                id,
                accelerator,
                reply,
            } => {
                let _ = reply.send(backend.bind_shortcut(&client, &id, &accelerator));
            }
            Command::UnbindShortcut { client, id } => backend.unbind_shortcut(&client, &id),
            Command::RegisterWindowRule {
                client,
                id,
                rule_toml,
                reply,
            } => {
                let _ = reply.send(backend.register_window_rule(&client, &id, &rule_toml));
            }
            Command::UnregisterWindowRule { client, id } => {
                backend.unregister_window_rule(&client, &id);
            }
            Command::ClientDisconnected(client) => backend.unregister_client(&client),
            Command::ThemeChanged => backend.theme_changed(),
            Command::Quit => backend.quit(),
        }
    }
}
