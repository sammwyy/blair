use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

use blair_protocol::{Rect, WindowId, WindowInfo, WorkspaceInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct DbusDisplay {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub refresh_millihz: i32,
    pub modes: Vec<(i32, i32, i32)>,
    pub can_change_mode: bool,
    pub pending_confirmation: bool,
    pub confirmation_seconds: u32,
}

impl From<blair_protocol::DisplayInfo> for DbusDisplay {
    fn from(info: blair_protocol::DisplayInfo) -> Self {
        Self {
            name: info.name,
            width: info.current.width,
            height: info.current.height,
            refresh_millihz: info.current.refresh_millihz,
            modes: info
                .modes
                .into_iter()
                .map(|mode| (mode.width, mode.height, mode.refresh_millihz))
                .collect(),
            can_change_mode: info.can_change_mode,
            pending_confirmation: info.pending_confirmation,
            confirmation_seconds: info.confirmation_seconds,
        }
    }
}

impl From<DbusDisplay> for blair_protocol::DisplayInfo {
    fn from(info: DbusDisplay) -> Self {
        Self {
            name: info.name,
            current: blair_protocol::DisplayMode {
                width: info.width,
                height: info.height,
                refresh_millihz: info.refresh_millihz,
            },
            modes: info
                .modes
                .into_iter()
                .map(
                    |(width, height, refresh_millihz)| blair_protocol::DisplayMode {
                        width,
                        height,
                        refresh_millihz,
                    },
                )
                .collect(),
            can_change_mode: info.can_change_mode,
            pending_confirmation: info.pending_confirmation,
            confirmation_seconds: info.confirmation_seconds,
        }
    }
}

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

impl From<DbusWorkspace> for WorkspaceInfo {
    fn from(workspace: DbusWorkspace) -> Self {
        Self {
            id: workspace.id,
            name: workspace.name,
            active: workspace.active,
            output: (!workspace.output.is_empty()).then_some(workspace.output),
            window_count: workspace.window_count as usize,
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
    fn display_signature_and_round_trip_preserve_physical_modes_and_preview() {
        assert_eq!(
            <DbusDisplay as Type>::SIGNATURE.to_string(),
            "(siiia(iii)bbu)"
        );
        let current = blair_protocol::DisplayMode {
            width: 1920,
            height: 1080,
            refresh_millihz: 59940,
        };
        let info = blair_protocol::DisplayInfo {
            name: "Virtual-1".into(),
            current,
            modes: vec![current],
            can_change_mode: true,
            pending_confirmation: true,
            confirmation_seconds: 15,
        };
        assert_eq!(
            blair_protocol::DisplayInfo::from(DbusDisplay::from(info.clone())),
            info
        );
        assert_eq!(current.config_value(), "1920x1080@59.940");
    }

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
