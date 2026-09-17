use std::{
    collections::BTreeMap,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompositorConfig {
    pub general: GeneralConfig,
    pub integrations: IntegrationsConfig,
    pub bindings: Vec<BindingConfig>,
    pub rules: Vec<WindowRuleConfig>,
    pub input: InputConfig,
    pub workspaces: WorkspacesConfig,
    pub outputs: BTreeMap<String, OutputConfig>,
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
pub struct IntegrationsConfig {
    pub dbus: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    pub keyboard: KeyboardConfig,
    pub mouse: MouseConfig,
    pub touchpad: TouchpadConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspacesConfig {
    pub count: u64,
    pub dynamic: bool,
    pub wrap: bool,
    #[serde(flatten)]
    pub definitions: BTreeMap<String, WorkspaceDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceDefinition {
    pub name: Option<String>,
    pub output: Option<String>,
}

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self {
            count: 10,
            dynamic: false,
            wrap: true,
            definitions: BTreeMap::new(),
        }
    }
}

impl WorkspacesConfig {
    fn validate(&self) -> Result<()> {
        if !(1..=100).contains(&self.count) {
            anyhow::bail!("workspaces count must be between 1 and 100");
        }
        for (id, definition) in &self.definitions {
            let id = id
                .parse::<u64>()
                .with_context(|| format!("workspace key '{id}' must be a positive integer"))?;
            if id == 0 || id > self.count {
                anyhow::bail!("workspace {id} is outside the configured count");
            }
            if definition
                .name
                .as_deref()
                .is_some_and(|name| name.trim().is_empty())
            {
                anyhow::bail!("workspace {id} has an empty name");
            }
            if definition
                .output
                .as_deref()
                .is_some_and(|output| output.trim().is_empty())
            {
                anyhow::bail!("workspace {id} has an empty output");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeyboardConfig {
    pub layout: String,
    pub variant: String,
    pub repeat_delay: i32,
    pub repeat_rate: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MouseConfig {
    pub sensitivity: f64,
    pub acceleration: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TouchpadConfig {
    pub tap: bool,
    pub natural_scroll: bool,
    pub disable_while_typing: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            keyboard: KeyboardConfig::default(),
            mouse: MouseConfig::default(),
            touchpad: TouchpadConfig::default(),
        }
    }
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: "us".to_string(),
            variant: String::new(),
            repeat_delay: 250,
            repeat_rate: 35,
        }
    }
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            sensitivity: 0.0,
            acceleration: "adaptive".to_string(),
        }
    }
}

impl Default for TouchpadConfig {
    fn default() -> Self {
        Self {
            tap: true,
            natural_scroll: true,
            disable_while_typing: true,
        }
    }
}

impl InputConfig {
    fn validate(&self) -> Result<()> {
        if self.keyboard.layout.trim().is_empty() {
            anyhow::bail!("keyboard layout cannot be empty");
        }
        if !(0..=2_000).contains(&self.keyboard.repeat_delay)
            || !(1..=100).contains(&self.keyboard.repeat_rate)
        {
            anyhow::bail!("keyboard repeat_delay must be 0..=2000 and repeat_rate 1..=100");
        }
        if !self.mouse.sensitivity.is_finite() || !(-1.0..=1.0).contains(&self.mouse.sensitivity) {
            anyhow::bail!("mouse sensitivity must be between -1.0 and 1.0");
        }
        if !matches!(self.mouse.acceleration.as_str(), "adaptive" | "flat") {
            anyhow::bail!("mouse acceleration must be adaptive or flat");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingConfig {
    pub keys: Vec<String>,
    pub action: Option<String>,
    pub exec: Option<String>,
    pub value: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputConfig {
    pub enabled: Option<bool>,
    pub mode: Option<String>,
    pub position: Option<[i32; 2]>,
    pub scale: Option<f64>,
    pub transform: Option<String>,
    pub vrr: Option<bool>,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            mode: None,
            position: None,
            scale: None,
            transform: None,
            vrr: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputMode {
    pub width: i32,
    pub height: i32,
    pub refresh_millihz: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTransform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl OutputConfig {
    pub fn parsed_mode(&self) -> Result<Option<OutputMode>> {
        self.mode.as_deref().map(parse_output_mode).transpose()
    }

    pub fn parsed_transform(&self) -> Result<Option<OutputTransform>> {
        self.transform
            .as_deref()
            .map(|value| match value {
                "normal" => Ok(OutputTransform::Normal),
                "90" => Ok(OutputTransform::Rotate90),
                "180" => Ok(OutputTransform::Rotate180),
                "270" => Ok(OutputTransform::Rotate270),
                "flipped" => Ok(OutputTransform::Flipped),
                "flipped-90" => Ok(OutputTransform::Flipped90),
                "flipped-180" => Ok(OutputTransform::Flipped180),
                "flipped-270" => Ok(OutputTransform::Flipped270),
                _ => anyhow::bail!("invalid output transform '{value}'"),
            })
            .transpose()
    }

    fn validate(&self, name: &str) -> Result<()> {
        self.parsed_mode()
            .with_context(|| format!("invalid mode for output {name}"))?;
        self.parsed_transform()
            .with_context(|| format!("invalid transform for output {name}"))?;
        if let Some(scale) = self.scale {
            if !scale.is_finite() || scale <= 0.0 {
                anyhow::bail!("output {name} has an invalid scale");
            }
        }
        Ok(())
    }
}

pub fn parse_output_mode(value: &str) -> Result<OutputMode> {
    let (resolution, refresh) = value
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("mode must use WIDTHxHEIGHT@REFRESH, got '{value}'"))?;
    let (width, height) = resolution
        .split_once('x')
        .ok_or_else(|| anyhow::anyhow!("mode must use WIDTHxHEIGHT@REFRESH, got '{value}'"))?;
    let width = width.parse::<i32>().context("invalid mode width")?;
    let height = height.parse::<i32>().context("invalid mode height")?;
    let refresh_hz = refresh.parse::<f64>().context("invalid refresh rate")?;
    if width <= 0 || height <= 0 || !refresh_hz.is_finite() || refresh_hz <= 0.0 {
        anyhow::bail!("mode dimensions and refresh rate must be positive");
    }
    Ok(OutputMode {
        width,
        height,
        refresh_millihz: (refresh_hz * 1000.0).round() as i32,
    })
}

impl BindingConfig {
    pub fn accelerator(&self) -> String {
        self.keys.join("+")
    }

    pub(crate) fn validate(&self, index: usize) -> Result<()> {
        if self.keys.is_empty() || self.keys.iter().any(|key| key.trim().is_empty()) {
            anyhow::bail!("binding #{index} must contain at least one non-empty key");
        }
        crate::shortcuts::validate_accelerator(&self.accelerator())
            .map_err(|error| anyhow::anyhow!("binding #{index}: {error}"))?;
        match (&self.action, &self.exec, self.value) {
            (Some(_), Some(_), _) => anyhow::bail!("binding #{index} cannot specify both action and exec"),
            (None, None, _) => anyhow::bail!("binding #{index} requires action or exec"),
            (None, Some(command), None) if !command.trim().is_empty() => Ok(()),
            (None, Some(_), _) => anyhow::bail!("binding #{index} has an invalid exec command"),
            (Some(action), None, None) if action == "close" => Ok(()),
            (Some(action), None, Some(value))
                if matches!(action.as_str(), "workspace" | "move-to-workspace") && value > 0 =>
            {
                Ok(())
            }
            (Some(action), _, _) => anyhow::bail!(
                "binding #{index} has invalid action '{action}' or value; supported actions are close, workspace, and move-to-workspace"
            ),
        }
    }
}

/// A declarative rule evaluated, in order, when an XDG toplevel is created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowRuleConfig {
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub class: Option<String>,
    pub regex: Option<String>,
    pub role: Option<String>,
    #[serde(rename = "type")]
    pub window_type: Option<String>,
    pub floating: Option<bool>,
    pub tiled: Option<bool>,
    pub workspace: Option<String>,
    pub output: Option<String>,
    pub size: Option<[i32; 2]>,
    pub position: Option<[i32; 2]>,
    pub opacity: Option<f32>,
    pub always_on_top: Option<bool>,
    pub decoration: Option<bool>,
}

impl Default for WindowRuleConfig {
    fn default() -> Self {
        Self {
            app_id: None,
            title: None,
            class: None,
            regex: None,
            role: None,
            window_type: None,
            floating: None,
            tiled: None,
            workspace: None,
            output: None,
            size: None,
            position: None,
            opacity: None,
            always_on_top: None,
            decoration: None,
        }
    }
}

impl WindowRuleConfig {
    pub(crate) fn validate(&self, index: usize) -> Result<()> {
        if self.app_id.is_none()
            && self.title.is_none()
            && self.class.is_none()
            && self.regex.is_none()
            && self.role.is_none()
            && self.window_type.is_none()
        {
            anyhow::bail!("rule #{index} requires at least one match field");
        }
        if let Some(pattern) = &self.regex {
            regex::Regex::new(pattern)
                .with_context(|| format!("rule #{index} has an invalid regex"))?;
        }
        if self.floating == Some(true) && self.tiled == Some(true) {
            anyhow::bail!("rule #{index} cannot be both floating and tiled");
        }
        if self
            .size
            .is_some_and(|[width, height]| width <= 0 || height <= 0)
        {
            anyhow::bail!("rule #{index} size must be positive");
        }
        if self
            .opacity
            .is_some_and(|opacity| !opacity.is_finite() || !(0.0..=1.0).contains(&opacity))
        {
            anyhow::bail!("rule #{index} opacity must be between 0.0 and 1.0");
        }
        for value in [
            &self.app_id,
            &self.title,
            &self.class,
            &self.role,
            &self.window_type,
        ] {
            if value
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                anyhow::bail!("rule #{index} has an empty match value");
            }
        }
        Ok(())
    }
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

impl Default for IntegrationsConfig {
    fn default() -> Self {
        Self { dbus: true }
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

    let config: CompositorConfig = merged
        .try_into()
        .context("merged compositor configuration does not match the schema")?;
    for (index, binding) in config.bindings.iter().enumerate() {
        binding.validate(index + 1)?;
    }
    for (index, rule) in config.rules.iter().enumerate() {
        rule.validate(index + 1)?;
    }
    for (name, output) in &config.outputs {
        output.validate(name)?;
    }
    config.input.validate()?;
    config.workspaces.validate()?;
    Ok(config)
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
        assert!(restored.integrations.dbus);
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

    #[test]
    fn invalid_static_binding_is_rejected() {
        let root = scratch_dir("invalid-binding");
        let paths = ConfigPaths {
            system_dir: root.join("etc").join("blair"),
            user_dir: root.join("user").join("blair"),
        };
        fs::create_dir_all(&paths.user_dir).unwrap();
        fs::write(
            paths.user_config_path(),
            "[[bindings]]\nkeys = [\"SUPER\", \"Q\"]\naction = \"unknown\"\n",
        )
        .unwrap();

        assert!(load_from_paths(&paths).is_err());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parses_output_mode_and_validates_output_fields() {
        let mode = parse_output_mode("2560x1440@165").unwrap();
        assert_eq!(
            mode,
            OutputMode {
                width: 2560,
                height: 1440,
                refresh_millihz: 165_000,
            }
        );
        assert!(parse_output_mode("2560x1440").is_err());
        assert!(OutputConfig {
            scale: Some(0.0),
            ..Default::default()
        }
        .validate("DP-1")
        .is_err());
    }

    #[test]
    fn rejects_invalid_input_settings() {
        let mut input = InputConfig::default();
        input.mouse.sensitivity = 1.1;
        assert!(input.validate().is_err());
        input.mouse.sensitivity = 0.0;
        input.mouse.acceleration = "invalid".to_string();
        assert!(input.validate().is_err());
    }

    #[test]
    fn parses_declarative_workspaces() {
        let config: CompositorConfig = toml::from_str(
            "[workspaces]\ncount = 2\ndynamic = false\nwrap = true\n\n[workspaces.\"1\"]\nname = \"dev\"\noutput = \"DP-1\"\n",
        )
        .unwrap();
        config.workspaces.validate().unwrap();
        assert_eq!(
            config.workspaces.definitions["1"].name.as_deref(),
            Some("dev")
        );
        assert_eq!(
            config.workspaces.definitions["1"].output.as_deref(),
            Some("DP-1")
        );
    }

    #[test]
    fn parses_and_validates_window_rules() {
        let config: CompositorConfig = toml::from_str(
            "[[rules]]\napp_id = \"pavucontrol\"\nfloating = true\nsize = [700, 500]\n\n[[rules]]\nregex = \"Picture-in-Picture\"\nalways_on_top = true\nopacity = 0.9\n",
        )
        .unwrap();
        for (index, rule) in config.rules.iter().enumerate() {
            rule.validate(index + 1).unwrap();
        }
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].size, Some([700, 500]));
    }

    #[test]
    fn rejects_invalid_window_rule() {
        let config: CompositorConfig =
            toml::from_str("[[rules]]\napp_id = \"x\"\nfloating = true\ntiled = true\n").unwrap();
        assert!(config.rules[0].validate(1).is_err());
    }
}
