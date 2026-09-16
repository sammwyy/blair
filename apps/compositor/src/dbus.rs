use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use blair_dbus::{Command, CompositorBackend};
use blair_protocol::{CompositorEvent, Rect, WindowId, WindowInfo};

use crate::config::WindowLayout;
use crate::state::BlairState;

pub struct DbusService {
    pub commands: Receiver<Command>,
    _thread: JoinHandle<()>,
}

pub fn start() -> (DbusService, Sender<CompositorEvent>) {
    let (commands_tx, commands_rx) = mpsc::channel();
    let (events_tx, events_rx) = mpsc::channel();
    let thread = blair_dbus::serve(commands_tx, events_rx);
    (
        DbusService {
            commands: commands_rx,
            _thread: thread,
        },
        events_tx,
    )
}

impl DbusService {
    pub fn drain(&self, state: &mut BlairState) {
        blair_dbus::drain(&self.commands, state);
    }
}

impl CompositorBackend for BlairState {
    fn list_windows(&self) -> Vec<WindowInfo> {
        BlairState::list_windows(self)
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
        self.shortcuts.bind(client, id, accelerator)
    }

    fn unbind_shortcut(&mut self, client: &str, id: &str) {
        self.shortcuts.unbind(client, id)
    }

    fn unregister_client(&mut self, client: &str) {
        self.shortcuts.unregister_client(client)
    }

    fn quit(&mut self) {
        self.request_exit();
    }
}
