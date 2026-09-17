use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

use blair_protocol::{Rect, WindowId, WindowInfo, WorkspaceInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct DbusWindow {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub focused: bool,
    pub minimized: bool,
    pub maximized: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct DbusWorkspace {
    pub id: u64,
    pub name: String,
    pub active: bool,
    pub output: String,
    pub window_count: u32,
}

impl From<&WorkspaceInfo> for DbusWorkspace {
    fn from(info: &WorkspaceInfo) -> Self {
        Self {
            id: info.id,
            name: info.name.clone(),
            active: info.active,
            output: info.output.clone().unwrap_or_default(),
            window_count: info.window_count.try_into().unwrap_or(u32::MAX),
        }
    }
}

impl From<DbusWindow> for WindowInfo {
    fn from(window: DbusWindow) -> Self {
        Self {
            id: WindowId(window.id),
            title: window.title,
            app_id: (!window.app_id.is_empty()).then_some(window.app_id),
            geometry: Rect {
                x: window.x,
                y: window.y,
                width: window.width,
                height: window.height,
            },
            focused: window.focused,
            minimized: window.minimized,
            maximized: window.maximized,
        }
    }
}

impl From<&WindowInfo> for DbusWindow {
    fn from(info: &WindowInfo) -> Self {
        Self {
            id: info.id.0,
            title: info.title.clone(),
            app_id: info.app_id.clone().unwrap_or_default(),
            x: info.geometry.x,
            y: info.geometry.y,
            width: info.geometry.width,
            height: info.geometry.height,
            focused: info.focused,
            minimized: info.minimized,
            maximized: info.maximized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_a_flat_ten_field_struct() {
        assert_eq!(<DbusWindow as Type>::SIGNATURE.to_string(), "(tssiiiibbb)");
    }

    #[test]
    fn workspace_signature_contains_identity_and_state() {
        assert_eq!(<DbusWorkspace as Type>::SIGNATURE.to_string(), "(tsbsu)");
    }

    #[test]
    fn round_trips_through_protocol_window_info() {
        let info = WindowInfo {
            id: WindowId(7),
            title: "Terminal".to_owned(),
            app_id: Some("org.example.Terminal".to_owned()),
            geometry: Rect {
                x: 1,
                y: 2,
                width: 800,
                height: 600,
            },
            focused: true,
            minimized: false,
            maximized: false,
        };
        let wire = DbusWindow::from(&info);
        assert_eq!(WindowInfo::from(wire), info);
    }

    #[test]
    fn empty_app_id_round_trips_to_none() {
        let wire = DbusWindow {
            id: 1,
            title: "Untitled".to_owned(),
            app_id: String::new(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            focused: false,
            minimized: false,
            maximized: false,
        };
        assert_eq!(WindowInfo::from(wire).app_id, None);
    }
}
