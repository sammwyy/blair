use std::sync::Arc;

use blair_integration::{CompositorApi, EventChannel, EventFanout, Transport};
use blair_integration_dbus::DbusIntegration;
use blair_protocol::{Rect, WindowId, WindowInfo};

use crate::config::WindowLayout;
use crate::state::BlairState;

pub struct Integrations {
    transports: Vec<Box<dyn Transport>>,
    events: Arc<dyn EventChannel>,
}

impl Integrations {
    pub fn start(dbus_enabled: bool) -> Self {
        let mut transports: Vec<Box<dyn Transport>> = Vec::new();
        let mut event_channels = Vec::new();
        if dbus_enabled {
            let dbus = DbusIntegration::start();
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

    fn window_settings(&self) -> (i32, i32, i32, bool) {
        let decoration = &self.config.decoration;
        (
            decoration.titlebar_height,
            decoration.border_width,
            decoration.corner_radius,
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
            self.config.decoration.corner_radius,
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
        self.config.decoration.corner_radius = corner_radius;
        if crate::config::save(&self.config).is_err() {
            return false;
        }
        self.tile_focused_window();
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
        self.config.decoration.titlebar_height = titlebar_height;
        self.config.decoration.border_width = border_width;
        self.config.decoration.corner_radius = corner_radius;
        self.config.window.server_side_decorations = server_side_decorations;
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

    fn unregister_client(&mut self, client: &str) {
        if client == "blair-config" {
            return;
        }
        self.shortcuts.unregister_client(client)
    }

    fn quit(&mut self) {
        self.request_exit();
    }
}
