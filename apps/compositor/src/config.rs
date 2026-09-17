use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use toml::Value;

use crate::decorations::DecorationTheme;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompositorConfig {
    pub general: GeneralConfig,
    pub window: WindowConfig,
    pub decoration: DecorationConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    /// Command started after the Wayland socket is ready.
    pub primary_client: String,
    pub spawn_primary_client: bool,
    pub backend: String,
    pub hot_reload: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub default_width: i32,
    pub default_height: i32,
    pub server_side_decorations: bool,
    pub layout: WindowLayout,
    pub work_area_padding: i32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowLayout {
    #[default]
    Floating,
    Tiling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DecorationConfig {
    pub titlebar_height: i32,
    pub border_width: i32,
    pub corner_radius: i32,
    pub active_titlebar: String,
    pub inactive_titlebar: String,
    pub active_border: String,
    pub inactive_border: String,
    pub close_button: String,
    pub maximize_button: String,
    pub minimize_button: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            primary_client: "coconut".to_string(),
            spawn_primary_client: true,
            backend: "auto".to_string(),
            hot_reload: true,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            default_width: 900,
            default_height: 600,
            server_side_decorations: true,
            layout: WindowLayout::Floating,
            work_area_padding: 16,
        }
    }
}

impl Default for DecorationConfig {
    fn default() -> Self {
        Self {
            titlebar_height: 32,
            border_width: 4,
            corner_radius: 12,
            active_titlebar: "#1e1e2e".to_string(),
            inactive_titlebar: "#11111b".to_string(),
            active_border: "#89b4fa".to_string(),
            inactive_border: "#313244".to_string(),
            close_button: "#f38ba8".to_string(),
            maximize_button: "#a6e3a1".to_string(),
            minimize_button: "#f9e2af".to_string(),
        }
    }
}

impl DecorationConfig {
    pub fn to_theme(&self) -> DecorationTheme {
        DecorationTheme {
            titlebar_height: self.titlebar_height,
            border_width: self.border_width,
            active_titlebar: parse_color(&self.active_titlebar),
            inactive_titlebar: parse_color(&self.inactive_titlebar),
            active_border: parse_color(&self.active_border),
            inactive_border: parse_color(&self.inactive_border),
            close_button: parse_color(&self.close_button),
            maximize_button: parse_color(&self.maximize_button),
            minimize_button: parse_color(&self.minimize_button),
        }
    }
}

fn parse_color(hex: &str) -> [u8; 4] {
    let hex = hex.trim_start_matches('#');
    let channel = |range: std::ops::Range<usize>| {
        hex.get(range)
            .and_then(|part| u8::from_str_radix(part, 16).ok())
    };
    match (channel(0..2), channel(2..4), channel(4..6)) {
        (Some(r), Some(g), Some(b)) => [r, g, b, channel(6..8).unwrap_or(0xff)],
        _ => {
            tracing::warn!(hex, "invalid decoration color, using opaque black");
            [0, 0, 0, 0xff]
        }
    }
}

const CONFIG_FILE_NAME: &str = "config.toml";

/// Locations contributing configuration, in increasing precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub system_dir: PathBuf,
    pub user_dir: PathBuf,
}

impl Default for ConfigPaths {
    fn default() -> Self {
        Self {
            system_dir: PathBuf::from("/etc/blair"),
            user_dir: dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from(".config"))
                .join("blair"),
        }
    }
}

impl ConfigPaths {
    fn main_file(&self, directory: &Path) -> PathBuf {
        directory.join(CONFIG_FILE_NAME)
    }

    fn fragments_dir(&self, directory: &Path) -> PathBuf {
        directory.join("conf.d")
    }

    pub fn user_config_path(&self) -> PathBuf {
        self.main_file(&self.user_dir)
    }
}

pub fn user_config_path() -> PathBuf {
    ConfigPaths::default().user_config_path()
}

/// Loads built-in defaults followed by system and user configuration.
///
/// Scalars and arrays in a later layer replace earlier values. TOML tables
/// merge recursively. Fragments are loaded in lexicographic filename order.
pub fn load_or_default() -> Result<CompositorConfig> {
    load_from_paths(&ConfigPaths::default())
}

pub fn load_from_paths(paths: &ConfigPaths) -> Result<CompositorConfig> {
    let mut merged = Value::try_from(CompositorConfig::default())
        .context("failed to serialize built-in compositor config")?;

    for directory in [&paths.system_dir, &paths.user_dir] {
        merge_file_if_present(&mut merged, &paths.main_file(directory))?;
        for fragment in config_fragments(&paths.fragments_dir(directory))? {
            merge_file_if_present(&mut merged, &fragment)?;
        }
    }

    merged
        .try_into()
        .context("merged compositor configuration does not match the schema")
}

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(100);

/// Watches the configuration roots and reports a reload after changes settle.
///
/// The watcher intentionally only observes existing roots. Creating a new
/// configuration root still requires a compositor restart, while edits to an
/// active configuration are picked up immediately.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    last_event: Option<Instant>,
    paths: ConfigPaths,
}

impl ConfigWatcher {
    pub fn new(paths: &ConfigPaths) -> Result<Self> {
        let (sender, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })
        .context("failed to initialize configuration watcher")?;

        for directory in [&paths.system_dir, &paths.user_dir] {
            if directory.is_dir() {
                watcher
                    .watch(directory, RecursiveMode::Recursive)
                    .with_context(|| {
                        format!("failed to watch config directory {}", directory.display())
                    })?;
                tracing::debug!(path = %directory.display(), "watching config directory");
            }
        }

        Ok(Self {
            _watcher: watcher,
            events,
            last_event: None,
            paths: paths.clone(),
        })
    }

    /// Returns true once filesystem activity has been quiet for the debounce
    /// interval. Files are deliberately parsed only by the caller at that
    /// point, never from the notify callback.
    pub fn reload_due(&mut self) -> bool {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Ok(event) => {
                    tracing::debug!(?event.kind, paths = ?event.paths, "config filesystem event");
                    self.last_event = Some(Instant::now());
                }
                Err(error) => tracing::warn!(%error, "config watcher error"),
            }
        }

        self.last_event
            .is_some_and(|last_event| last_event.elapsed() >= RELOAD_DEBOUNCE)
            && self.last_event.take().is_some()
    }

    pub fn reload_if_due(&mut self, state: &mut crate::state::BlairState) {
        if !state.config.general.hot_reload || !self.reload_due() {
            return;
        }

        match load_from_paths(&self.paths) {
            Ok(next) => {
                state.apply_config(next);
                tracing::info!("configuration reloaded");
            }
            Err(error) => {
                tracing::error!(%error, "configuration reload failed; keeping previous configuration")
            }
        }
    }
}

fn config_fragments(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut fragments = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(fragments),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read config directory {}", directory.display())
            })
        }
    };

    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", directory.display()))?;
        let path = entry.path();
        if entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?
            .is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "toml")
        {
            fragments.push(path);
        }
    }
    fragments.sort();
    Ok(fragments)
}

fn merge_file_if_present(target: &mut Value, path: &Path) -> Result<()> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let layer: Value = toml::from_str(&contents)
                .with_context(|| format!("invalid TOML in {}", path.display()))?;
            merge_toml(target, layer);
            tracing::debug!(path = %path.display(), "loaded config layer");
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to read config {}", path.display()))
        }
    }
}

fn merge_toml(base: &mut Value, override_value: Value) {
    match (base, override_value) {
        (Value::Table(base), Value::Table(override_table)) => {
            for (key, value) in override_table {
                match base.get_mut(&key) {
                    Some(existing) => merge_toml(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, override_value) => *base = override_value,
    }
}

pub fn save(config: &CompositorConfig) -> Result<()> {
    let path = user_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config dir {}", parent.display()))?;
    }
    let toml = toml::to_string_pretty(config).context("failed to serialize compositor config")?;
    fs::write(&path, toml).with_context(|| format!("failed to write config to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips() {
        let config = CompositorConfig::default();
        let serialized = toml::to_string_pretty(&config).expect("serialize");
        let restored: CompositorConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(restored.window.default_width, config.window.default_width);
    }

    #[test]
    fn parses_six_and_eight_digit_hex() {
        assert_eq!(parse_color("#1e1e2e"), [0x1e, 0x1e, 0x2e, 0xff]);
        assert_eq!(parse_color("#1e1e2e80"), [0x1e, 0x1e, 0x2e, 0x80]);
    }

    #[test]
    fn falls_back_on_invalid_hex() {
        assert_eq!(parse_color("not-a-color"), [0, 0, 0, 0xff]);
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "blair-config-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn layers_merge_in_documented_precedence_order() {
        let root = scratch_dir("layered-load");
        let paths = ConfigPaths {
            system_dir: root.join("etc").join("blair"),
            user_dir: root.join("user").join("blair"),
        };
        fs::create_dir_all(paths.system_dir.join("conf.d")).unwrap();
        fs::create_dir_all(paths.user_dir.join("conf.d")).unwrap();

        fs::write(
            paths.system_dir.join(CONFIG_FILE_NAME),
            "[general]\nbackend = \"winit\"\n[window]\ndefault_width = 1000\n",
        )
        .unwrap();
        fs::write(
            paths.system_dir.join("conf.d").join("20-window.toml"),
            "[window]\ndefault_width = 1100\ndefault_height = 700\n",
        )
        .unwrap();
        fs::write(
            paths.system_dir.join("conf.d").join("10-window.toml"),
            "[window]\ndefault_width = 1050\n",
        )
        .unwrap();
        fs::write(
            paths.user_dir.join(CONFIG_FILE_NAME),
            "[general]\nbackend = \"drm\"\n[window]\ndefault_height = 800\n",
        )
        .unwrap();
        fs::write(
            paths.user_dir.join("conf.d").join("30-window.toml"),
            "[window]\ndefault_width = 1200\n",
        )
        .unwrap();

        let config = load_from_paths(&paths).unwrap();
        assert_eq!(config.general.backend, "drm");
        assert_eq!(config.window.default_width, 1200);
        assert_eq!(config.window.default_height, 800);
        assert_eq!(config.window.work_area_padding, 16);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_layers_use_built_in_defaults_without_writing_files() {
        let root = scratch_dir("missing-layers");
        let paths = ConfigPaths {
            system_dir: root.join("etc").join("blair"),
            user_dir: root.join("user").join("blair"),
        };

        let config = load_from_paths(&paths).unwrap();
        assert_eq!(config.general.primary_client, "coconut");
        assert!(!paths.user_config_path().exists());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn invalid_layer_is_rejected_before_any_config_is_applied() {
        let root = scratch_dir("invalid-layer");
        let paths = ConfigPaths {
            system_dir: root.join("etc").join("blair"),
            user_dir: root.join("user").join("blair"),
        };
        fs::create_dir_all(&paths.user_dir).unwrap();
        fs::write(
            paths.user_config_path(),
            "[window]\ndefault_width = \"hello\"\n",
        )
        .unwrap();

        assert!(load_from_paths(&paths).is_err());

        fs::remove_dir_all(&root).ok();
    }
}
