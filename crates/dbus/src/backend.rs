use std::sync::mpsc::Receiver;

use blair_protocol::{Rect, WindowId, WindowInfo};

use crate::{command::Command, wire::DbusWindow};

pub trait CompositorBackend {
    fn list_windows(&self) -> Vec<WindowInfo>;
    fn focus_window(&mut self, id: WindowId) -> bool;
    fn close_window(&mut self, id: WindowId) -> bool;
    fn minimize_window(&mut self, id: WindowId) -> bool;
    fn toggle_maximize_window(&mut self, id: WindowId) -> bool;
    fn move_resize_window(&mut self, id: WindowId, geometry: Rect) -> bool;
    fn work_area(&self, output: &str) -> Rect;
    fn outputs(&self) -> Vec<String>;
    fn window_settings(&self) -> (i32, i32, i32, bool);
    fn layout_settings(&self) -> (String, i32, i32);
    fn set_layout_settings(
        &mut self,
        layout: &str,
        work_area_padding: i32,
        corner_radius: i32,
    ) -> bool;
    fn set_window_settings(
        &mut self,
        titlebar_height: i32,
        border_width: i32,
        corner_radius: i32,
        server_side_decorations: bool,
    ) -> bool;
    fn bind_shortcut(&mut self, client: &str, id: &str, accelerator: &str) -> bool;
    fn unbind_shortcut(&mut self, client: &str, id: &str);
    fn unregister_client(&mut self, owner: &str);
    fn quit(&mut self);
}

/// Drains pending D-Bus method calls.
pub fn drain(commands: &Receiver<Command>, backend: &mut impl CompositorBackend) {
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
            Command::BindShortcut {
                client,
                id,
                accelerator,
                reply,
            } => {
                let _ = reply.send(backend.bind_shortcut(&client, &id, &accelerator));
            }
            Command::UnbindShortcut { client, id } => backend.unbind_shortcut(&client, &id),
            Command::ClientDisconnected(client) => backend.unregister_client(&client),
            Command::Quit => backend.quit(),
        }
    }
}
