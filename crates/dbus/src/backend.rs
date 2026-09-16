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
    fn bind_shortcut(&mut self, id: &str, accelerator: &str) -> bool;
    fn unbind_shortcut(&mut self, id: &str);
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
            Command::BindShortcut(id, accelerator, reply) => {
                let _ = reply.send(backend.bind_shortcut(&id, &accelerator));
            }
            Command::UnbindShortcut(id) => backend.unbind_shortcut(&id),
            Command::Quit => backend.quit(),
        }
    }
}
