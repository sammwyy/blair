use std::sync::Arc;

use blair_integration::{CompositorApi, EventChannel, EventFanout, Transport};
use blair_integration_dbus::DbusIntegration;
use blair_protocol::{Rect, RenderStats, ShortcutBinding, ShortcutCommand, WindowId, WindowInfo};

use crate::config::{BindingConfig, DecorationModeConfig, WindowLayout};
use crate::state::BlairState;

pub struct Integrations {
    transports: Vec<Box<dyn Transport>>,
    events: Arc<dyn EventChannel>,
}

impl Integrations {
    /// `wake` is handed to integration threads so queued requests wake the
    /// compositor's event loop.
    pub fn start(
        dbus_enabled: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
        environment: Vec<(String, String)>,
    ) -> Self {
        let mut transports: Vec<Box<dyn Transport>> = Vec::new();
        let mut event_channels = Vec::new();
        if dbus_enabled {
            let dbus = DbusIntegration::start(wake, environment);
            event_channels.push(dbus.event_channel());
            transports.push(Box::new(dbus));
        }
        Self {
            transports,
            events: Arc::new(EventFanout::new(event_channels)),
        }
    }

    pub fn event_channel(&self) -> Arc<dyn EventChannel> {
        self.events.clone()
    }

    pub fn drain(&mut self, state: &mut BlairState) {
        for transport in &mut self.transports {
            transport.drain(state);
        }
    }
}

impl CompositorApi for BlairState {
    fn list_windows(&self) -> Vec<WindowInfo> {
        BlairState::list_windows(self)
    }

    fn list_workspaces(&self) -> Vec<blair_protocol::WorkspaceInfo> {
        BlairState::list_workspaces(self)
    }

    fn create_workspace(&mut self, name: String) -> u64 {
        BlairState::create_workspace(self, name)
    }

    fn switch_workspace(&mut self, id: u64) -> bool {
        BlairState::switch_workspace(self, id)
    }

    fn move_window_to_workspace(&mut self, window: WindowId, workspace_id: u64) -> bool {
        BlairState::move_window_to_workspace(self, window, workspace_id)
    }

    fn focus_window(&mut self, id: WindowId) -> bool {
        self.focus_window_by_id(id)
    }

    fn close_window(&mut self, id: WindowId) -> bool {
        self.close_window_by_id(id)
    }

    fn minimize_window(&mut self, id: WindowId) -> bool {
        self.minimize_window_by_id(id)
    }

    fn toggle_maximize_window(&mut self, id: WindowId) -> bool {
        self.toggle_maximize_window_by_id(id)
    }

    fn move_resize_window(&mut self, id: WindowId, geometry: Rect) -> bool {
        BlairState::move_resize_window(self, id, geometry)
    }

    fn work_area(&self, output: &str) -> Rect {
        BlairState::work_area(self, output)
    }

    fn outputs(&self) -> Vec<String> {
        BlairState::outputs(self)
    }

    fn screenshot(&mut self, output: &str, path: &str, reply: Box<dyn FnOnce(bool) + Send>) {
        self.request_screenshot(output, path, reply);
    }

    fn render_stats(&self) -> RenderStats {
        self.render_stats
    }

    fn window_settings(&self) -> (i32, i32, i32, bool) {
        let decoration = &self.config.decorations;
        (
            decoration.titlebar_height,
            self.decoration_theme().border_width,
            self.decoration_theme().corner_radius,
            self.config.window.server_side_decorations,
        )
    }

    fn layout_settings(&self) -> (String, i32, i32) {
        let layout = match self.config.window.layout {
            WindowLayout::Floating => "floating",
            WindowLayout::Tiling => "tiling",
        };
        (
            layout.to_string(),
            self.config.window.work_area_padding,
            self.decoration_theme().corner_radius,
        )
    }

    fn set_layout_settings(
        &mut self,
        layout: &str,
        work_area_padding: i32,
        corner_radius: i32,
    ) -> bool {
        let layout = match layout {
            "floating" => WindowLayout::Floating,
            "tiling" => WindowLayout::Tiling,
            _ => return false,
        };
        if !(0..=128).contains(&work_area_padding) || !matches!(corner_radius, 0 | 12 | 24) {
            return false;
        }
        self.config.window.layout = layout;
        self.config.window.work_area_padding = work_area_padding;
        self.config.decorations.corner_radius = corner_radius;
        if let Err(error) = crate::config::save(&self.config) {
            tracing::warn!(%error, "failed to persist layout settings");
            return false;
        }
        tracing::info!(
            ?layout,
            work_area_padding,
            corner_radius,
            "layout settings updated by an integration"
        );
        self.retile_focused_workspace();
        self.request_redraw();
        true
    }

    fn set_window_settings(
        &mut self,
        titlebar_height: i32,
        border_width: i32,
        corner_radius: i32,
        server_side_decorations: bool,
    ) -> bool {
        if !(20..=96).contains(&titlebar_height)
            || !(0..=16).contains(&border_width)
            || !(0..=64).contains(&corner_radius)
        {
            tracing::warn!(
                titlebar_height,
                border_width,
                corner_radius,
                "invalid window settings"
            );
            return false;
        }
        self.config.decorations.titlebar_height = titlebar_height;
        // Pixel widths from integrations snap to the nearest named size; 0
        // hides the border like the settings' "None" choice.
        if border_width == 0 {
            self.config.decorations.border = crate::config::BorderColorMode::None;
        } else {
            if self.config.decorations.border == crate::config::BorderColorMode::None {
                self.config.decorations.border = crate::config::BorderColorMode::Theme;
            }
            self.config.decorations.border_size = crate::config::BorderSize::nearest(border_width);
        }
        self.config.decorations.corner_radius = corner_radius;
        self.config.window.server_side_decorations = server_side_decorations;
        self.config.decorations.mode = if server_side_decorations {
            DecorationModeConfig::Server
        } else {
            DecorationModeConfig::Client
        };
        if let Err(error) = crate::config::save(&self.config) {
            tracing::warn!(%error, "failed to persist window settings");
            return false;
        }
        self.request_redraw();
        tracing::info!(
            titlebar_height,
            border_width,
            corner_radius,
            server_side_decorations,
            "window settings updated"
        );
        true
    }

    fn configuration(&self) -> String {
        toml::to_string_pretty(&self.config).unwrap_or_default()
    }

    fn set_configuration(&mut self, configuration: &str) -> bool {
        let next = match toml::from_str(configuration) {
            Ok(next) => next,
            Err(error) => {
                tracing::warn!(%error, "integration sent an invalid configuration");
                return false;
            }
        };
        if let Err(error) = crate::config::validate(&next) {
            tracing::warn!(%error, "integration sent a configuration that failed validation");
            return false;
        }
        if let Err(error) = crate::config::save(&next) {
            tracing::warn!(%error, "failed to persist the configuration");
            return false;
        }
        tracing::info!("configuration replaced by an integration");
        self.apply_config(next);
        true
    }

    fn configured_shortcuts(&self) -> Vec<ShortcutBinding> {
        self.config
            .bindings
            .iter()
            .filter_map(
                |binding| match (&binding.action, &binding.exec, binding.value) {
                    (Some(action), None, None) if action == "close" => Some(ShortcutBinding {
                        accelerator: binding.accelerator(),
                        command: ShortcutCommand::Close,
                        argument: None,
                    }),
                    (Some(action), None, Some(value)) if action == "workspace" => {
                        Some(ShortcutBinding {
                            accelerator: binding.accelerator(),
                            command: ShortcutCommand::Workspace,
                            argument: Some(value.to_string()),
                        })
                    }
                    (Some(action), None, Some(value)) if action == "move-to-workspace" => {
                        Some(ShortcutBinding {
                            accelerator: binding.accelerator(),
                            command: ShortcutCommand::MoveToWorkspace,
                            argument: Some(value.to_string()),
                        })
                    }
                    (None, Some(command), None) => Some(ShortcutBinding {
                        accelerator: binding.accelerator(),
                        command: ShortcutCommand::Execute,
                        argument: Some(command.clone()),
                    }),
                    _ => None,
                },
            )
            .collect()
    }

    fn set_configured_shortcuts(&mut self, bindings: Vec<ShortcutBinding>) -> bool {
        let mut configured = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let keys = binding
                .accelerator
                .split('+')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
                .collect();
            let next = match binding.command {
                ShortcutCommand::Close if binding.argument.is_none() => BindingConfig {
                    keys,
                    action: Some("close".to_owned()),
                    exec: None,
                    value: None,
                },
                ShortcutCommand::Workspace | ShortcutCommand::MoveToWorkspace => {
                    let Some(value) = binding
                        .argument
                        .as_deref()
                        .and_then(|value| value.parse().ok())
                    else {
                        return false;
                    };
                    BindingConfig {
                        keys,
                        action: Some(binding.command.id().to_owned()),
                        exec: None,
                        value: Some(value),
                    }
                }
                ShortcutCommand::Execute => {
                    let Some(command) = binding.argument.filter(|value| !value.trim().is_empty())
                    else {
                        return false;
                    };
                    BindingConfig {
                        keys,
                        action: None,
                        exec: Some(command),
                        value: None,
                    }
                }
                _ => return false,
            };
            if next.validate(configured.len() + 1).is_err() {
                return false;
            }
            configured.push(next);
        }
        self.config.bindings = configured;
        if crate::config::save(&self.config).is_err() {
            return false;
        }
        self.replace_static_bindings(&self.config.bindings.clone());
        true
    }

    fn bind_shortcut(&mut self, client: &str, id: &str, accelerator: &str) -> bool {
        if client == "blair-config" {
            return false;
        }
        self.shortcuts.bind(client, id, accelerator)
    }

    fn unbind_shortcut(&mut self, client: &str, id: &str) {
        if client == "blair-config" {
            return;
        }
        self.shortcuts.unbind(client, id)
    }

    fn register_window_rule(&mut self, client: &str, id: &str, rule_toml: &str) -> bool {
        if client == "blair-config" {
            return false;
        }
        self.register_temporary_rule(client, id, rule_toml)
    }

    fn unregister_window_rule(&mut self, client: &str, id: &str) {
        self.rules.unregister(client, id);
    }

    fn unregister_client(&mut self, client: &str) {
        if client == "blair-config" {
            return;
        }
        self.shortcuts.unregister_client(client);
        self.rules.unregister_client(client);
    }

    fn theme_changed(&mut self) {
        self.reload_system_theme();
    }

    fn quit(&mut self) {
        self.request_exit();
    }
}
