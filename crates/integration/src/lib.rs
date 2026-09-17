use std::sync::Arc;

use blair_protocol::{CompositorEvent, Rect, WindowId, WindowInfo, WorkspaceInfo};

/// Receives compositor events. Integrations provide implementations for their
/// own transport; the compositor never needs to know the transport details.
pub trait EventChannel: Send + Sync {
    fn publish(&self, event: CompositorEvent);
}

/// Fans events out to every enabled integration.
pub struct EventFanout {
    channels: Vec<Arc<dyn EventChannel>>,
}

impl EventFanout {
    pub fn new(channels: Vec<Arc<dyn EventChannel>>) -> Self {
        Self { channels }
    }
}

impl EventChannel for EventFanout {
    fn publish(&self, event: CompositorEvent) {
        for channel in &self.channels {
            channel.publish(event.clone());
        }
    }
}

/// Request surface offered by the compositor to every integration.
pub trait CompositorApi {
    fn list_windows(&self) -> Vec<WindowInfo>;
    fn list_workspaces(&self) -> Vec<WorkspaceInfo>;
    fn create_workspace(&mut self, name: String) -> u64;
    fn switch_workspace(&mut self, id: u64) -> bool;
    fn move_window_to_workspace(&mut self, window: WindowId, workspace_id: u64) -> bool;
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
    fn unregister_client(&mut self, client: &str);
    fn quit(&mut self);
}

/// A pluggable request transport. Backends call this from their main loop.
pub trait Transport: Send {
    fn drain(&mut self, compositor: &mut dyn CompositorApi);
}
