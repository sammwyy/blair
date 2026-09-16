use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use blair_dbus::{Command, CompositorBackend};
use blair_protocol::{CompositorEvent, Rect, WindowId, WindowInfo};

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

    fn bind_shortcut(&mut self, id: &str, accelerator: &str) -> bool {
        self.shortcuts.bind(id, accelerator)
    }

    fn unbind_shortcut(&mut self, id: &str) {
        self.shortcuts.unbind(id)
    }

    fn quit(&mut self) {
        self.request_exit();
    }
}
