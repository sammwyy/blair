use std::{collections::HashMap, time::Duration};

use anyhow::Result;
use blair_protocol::WindowId;
use serde::{Deserialize, Serialize};
use smithay::{
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point, Rectangle},
};

use crate::state::BlairState;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CornerAction {
    #[default]
    None,
    NextWorkspace,
    PreviousWorkspace,
    ShowDesktop,
    Exec,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CornerConfig {
    pub action: CornerAction,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotCornersConfig {
    pub enabled: bool,
    pub dwell_ms: u64,
    pub threshold: i32,
    pub top_left: CornerConfig,
    pub top_right: CornerConfig,
    pub bottom_left: CornerConfig,
    pub bottom_right: CornerConfig,
}

impl Default for HotCornersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dwell_ms: 350,
            threshold: 4,
            top_left: CornerConfig::default(),
            top_right: CornerConfig::default(),
            bottom_left: CornerConfig::default(),
            bottom_right: CornerConfig::default(),
        }
    }
}

impl HotCornersConfig {
    pub fn validate(&self) -> Result<()> {
        if self.dwell_ms > 10_000 || !(1..=64).contains(&self.threshold) {
            anyhow::bail!("hot corner dwell must be 0..=10000 ms and threshold 1..=64 pixels");
        }
        for corner in [
            &self.top_left,
            &self.top_right,
            &self.bottom_left,
            &self.bottom_right,
        ] {
            if corner.action == CornerAction::Exec && corner.command.trim().is_empty() {
                anyhow::bail!("hot corner exec action requires a command");
            }
        }
        Ok(())
    }

    fn corner(&self, corner: Corner) -> &CornerConfig {
        match corner {
            Corner::TopLeft => &self.top_left,
            Corner::TopRight => &self.top_right,
            Corner::BottomLeft => &self.bottom_left,
            Corner::BottomRight => &self.bottom_right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    fn at(
        area: Rectangle<i32, Logical>,
        point: Point<f64, Logical>,
        threshold: i32,
    ) -> Option<Self> {
        if !area.to_f64().contains(point) {
            return None;
        }
        let threshold = f64::from(threshold.min(area.size.w / 2).min(area.size.h / 2));
        let left = point.x < f64::from(area.loc.x) + threshold;
        let right = point.x >= f64::from(area.loc.x + area.size.w) - threshold;
        let top = point.y < f64::from(area.loc.y) + threshold;
        let bottom = point.y >= f64::from(area.loc.y + area.size.h) - threshold;
        match (left, right, top, bottom) {
            (true, _, true, _) => Some(Self::TopLeft),
            (_, true, true, _) => Some(Self::TopRight),
            (true, _, _, true) => Some(Self::BottomLeft),
            (_, true, _, true) => Some(Self::BottomRight),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct HotCornerState {
    target: Option<(String, Corner)>,
    generation: u64,
    hidden_windows: HashMap<u64, Vec<WindowId>>,
}

impl HotCornerState {
    pub fn cancel(&mut self) {
        self.target = None;
        self.generation = self.generation.wrapping_add(1);
    }
}

impl BlairState {
    fn hot_corner_at(&self, location: Point<f64, Logical>) -> Option<(String, Corner)> {
        if self.session_locked
            || !self.config.hot_corners.enabled
            || self
                .seat
                .get_pointer()
                .is_some_and(|pointer| pointer.is_grabbed())
        {
            return None;
        }
        let output = self.output_at(location)?;
        let geometry = self.space.output_geometry(&output)?;
        let corner = Corner::at(geometry, location, self.config.hot_corners.threshold)?;
        (self.config.hot_corners.corner(corner).action != CornerAction::None)
            .then(|| (output.name(), corner))
    }

    pub fn update_hot_corners(&mut self, location: Point<f64, Logical>) {
        let target = self.hot_corner_at(location);
        if self.hot_corners.target == target {
            return;
        }
        self.hot_corners.cancel();
        self.hot_corners.target = target.clone();
        let Some(target) = target else {
            return;
        };
        let generation = self.hot_corners.generation;
        let delay = Duration::from_millis(self.config.hot_corners.dwell_ms);
        if let Err(error) = self.loop_handle.insert_source(
            Timer::from_duration(delay),
            move |_, _, state: &mut BlairState| {
                if state.hot_corners.generation == generation
                    && state.hot_corner_at(state.pointer_location()).as_ref() == Some(&target)
                {
                    state.run_corner_action(target.1);
                }
                TimeoutAction::Drop
            },
        ) {
            tracing::warn!(%error, "failed to schedule hot corner");
            self.hot_corners.cancel();
        }
    }

    fn run_corner_action(&mut self, corner: Corner) {
        let config = self.config.hot_corners.corner(corner).clone();
        tracing::info!(?corner, action = ?config.action, "hot corner activated");
        match config.action {
            CornerAction::None => {}
            CornerAction::Exec => self.spawn_command(&config.command),
            CornerAction::NextWorkspace => {
                let target = self.focused_workspace_id() + 1;
                self.ensure_workspace(target);
                self.switch_workspace(target);
            }
            CornerAction::PreviousWorkspace => {
                let current = self.focused_workspace_id();
                let target = if current == 1 && self.config.workspaces.wrap {
                    self.config.workspaces.count
                } else {
                    current.saturating_sub(1).max(1)
                };
                self.switch_workspace(target);
            }
            CornerAction::ShowDesktop => self.toggle_desktop(),
        }
    }

    fn toggle_desktop(&mut self) {
        let workspace = self.focused_workspace_id();
        if let Some(windows) = self.hot_corners.hidden_windows.remove(&workspace) {
            for id in windows {
                if self.windows.get(&id).is_some_and(|managed| {
                    managed.workspace == workspace && managed.minimized.is_some()
                }) {
                    self.focus_window_by_id(id);
                }
            }
            return;
        }
        let focused = self.focused_window;
        let mut windows: Vec<_> = self
            .windows
            .iter()
            .filter(|(_, managed)| {
                managed.mapped && managed.workspace == workspace && managed.minimized.is_none()
            })
            .map(|(id, _)| *id)
            .collect();
        windows.sort_by_key(|id| Some(*id) == focused);
        for id in &windows {
            self.minimize_window_by_id(*id);
        }
        if !windows.is_empty() {
            self.hot_corners.hidden_windows.insert(workspace, windows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_use_output_coordinates_and_require_both_edges() {
        let area = Rectangle::new((100, 200).into(), (800, 600).into());
        assert_eq!(
            Corner::at(area, (100.0, 200.0).into(), 4),
            Some(Corner::TopLeft)
        );
        assert_eq!(
            Corner::at(area, (899.0, 200.0).into(), 4),
            Some(Corner::TopRight)
        );
        assert_eq!(
            Corner::at(area, (100.0, 799.0).into(), 4),
            Some(Corner::BottomLeft)
        );
        assert_eq!(
            Corner::at(area, (899.0, 799.0).into(), 4),
            Some(Corner::BottomRight)
        );
        assert_eq!(Corner::at(area, (100.0, 500.0).into(), 4), None);
        assert_eq!(Corner::at(area, (99.0, 200.0).into(), 4), None);
    }

    #[test]
    fn executable_corners_require_a_nonempty_command() {
        let mut config = HotCornersConfig::default();
        config.top_left.action = CornerAction::Exec;
        assert!(config.validate().is_err());
        config.top_left.command = "foot".into();
        assert!(config.validate().is_ok());
    }
}
