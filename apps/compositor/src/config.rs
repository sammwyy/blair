use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::{
    self,
    timer::{TimeoutAction, Timer},
    LoopHandle, RegistrationToken,
};
use toml::Value;

use crate::{decorations::DecorationTheme, state::BlairState};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompositorConfig {
    pub general: GeneralConfig,
    pub integrations: IntegrationsConfig,
    pub bindings: Vec<BindingConfig>,
    pub focus: FocusConfig,
    pub animations: AnimationsConfig,
    pub autostart: Vec<AutostartConfig>,
    pub rules: Vec<WindowRuleConfig>,
    pub input: InputConfig,
    pub workspaces: WorkspacesConfig,
    pub outputs: BTreeMap<String, OutputConfig>,
    pub window: WindowConfig,
    #[serde(alias = "decoration")]
    pub decorations: DecorationConfig,
    pub cursor: CursorConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CursorConfig {
    /// Falls back to `XCURSOR_THEME`, then to the `default` theme.
    pub theme: Option<String>,
    /// Falls back to `XCURSOR_SIZE`, then to 24.
    pub size: Option<u32>,
}

impl CursorConfig {
    fn validate(&self) -> Result<()> {
        if self
            .theme
            .as_deref()
            .is_some_and(|theme| theme.trim().is_empty())
        {
            anyhow::bail!("cursor theme cannot be empty");
        }
        if self.size.is_some_and(|size| !(8..=256).contains(&size)) {
            anyhow::bail!("cursor size must be between 8 and 256");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    pub backend: String,
    pub hot_reload: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutostartConfig {
    pub command: String,
    #[serde(default)]
    pub restart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FocusConfig {
    pub policy: FocusPolicy,
    pub raise_on_focus: bool,
    pub focus_new_windows: bool,
    pub focus_previous_on_close: bool,
    pub warp_cursor: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusPolicy {
    #[default]
    Click,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            policy: FocusPolicy::Click,
            raise_on_focus: true,
            focus_new_windows: true,
            focus_previous_on_close: true,
            warp_cursor: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnimationsConfig {
    pub enabled: bool,
    pub window_open: AnimationConfig,
    pub window_close: AnimationConfig,
    pub workspace: AnimationConfig,
    pub minimize: AnimationConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnimationConfig {
    pub duration: u64,
    pub curve: AnimationCurve,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnimationCurve {
    Linear,
    EaseIn,
    EaseOut,
    #[default]
    EaseInOut,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            duration: 150,
            curve: AnimationCurve::EaseOut,
        }
    }
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_open: AnimationConfig::default(),
            window_close: AnimationConfig::default(),
            workspace: AnimationConfig {
                duration: 200,
                curve: AnimationCurve::EaseInOut,
            },
            minimize: AnimationConfig::default(),
        }
    }
}

impl AnimationsConfig {
    fn validate(&self) -> Result<()> {
        for (name, animation) in [
            ("window_open", &self.window_open),
            ("window_close", &self.window_close),
            ("workspace", &self.workspace),
            ("minimize", &self.minimize),
        ] {
            if animation.duration > 10_000 {
                anyhow::bail!("animations.{name}.duration must be at most 10000 ms");
            }
        }
        Ok(())
    }
}

impl AutostartConfig {
    fn validate(&self, index: usize) -> Result<()> {
        if self.command.trim().is_empty() {
            anyhow::bail!("autostart #{index} command cannot be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IntegrationsConfig {
    pub dbus: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputConfig {
    pub enabled: Option<bool>,
    pub mode: Option<String>,
    pub position: Option<[i32; 2]>,
    pub scale: Option<f64>,
    pub transform: Option<String>,
    pub vrr: Option<bool>,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub default_width: i32,
    pub default_height: i32,
    pub server_side_decorations: bool,
    pub layout: WindowLayout,
    /// Margin kept between the tiled area and the work area.
    pub work_area_padding: i32,
    /// Spacing between tiled windows.
    pub gap: i32,
    /// Share of the width taken by the master window when tiling.
    pub master_ratio: f64,
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
    pub mode: DecorationModeConfig,
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
    pub buttons: DecorationButtonsConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecorationModeConfig {
    Server,
    Client,
    #[default]
    Auto,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DecorationButtonsConfig {
    pub layout: Vec<DecorationButton>,
    pub side: DecorationButtonSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecorationButton {
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecorationButtonSide {
    Left,
    #[default]
    Right,
}

impl Default for DecorationButtonsConfig {
    fn default() -> Self {
        Self {
            layout: vec![
                DecorationButton::Minimize,
                DecorationButton::Maximize,
                DecorationButton::Close,
            ],
            side: DecorationButtonSide::Right,
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
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
            gap: 8,
            master_ratio: 0.5,
        }
    }
}

impl WindowConfig {
    fn validate(&self) -> Result<()> {
        if self.default_width <= 0 || self.default_height <= 0 {
            anyhow::bail!("window default size must be positive");
        }
        if !(0..=512).contains(&self.work_area_padding) || !(0..=512).contains(&self.gap) {
            anyhow::bail!("window padding and gap must be between 0 and 512");
        }
        if !self.master_ratio.is_finite() || !(0.1..=0.9).contains(&self.master_ratio) {
            anyhow::bail!("window master_ratio must be between 0.1 and 0.9");
        }
        Ok(())
    }
}

impl Default for DecorationConfig {
    fn default() -> Self {
        Self {
            mode: DecorationModeConfig::Auto,
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
            buttons: DecorationButtonsConfig::default(),
        }
    }
}

impl DecorationConfig {
    fn validate(&self) -> Result<()> {
        if !(0..=16).contains(&self.border_width)
            || !(0..=64).contains(&self.corner_radius)
            || !(0..=96).contains(&self.titlebar_height)
        {
            anyhow::bail!("invalid decoration dimensions");
        }
        let mut seen = std::collections::BTreeSet::new();
        if self
            .buttons
            .layout
            .iter()
            .any(|button| !seen.insert(*button as u8))
        {
            anyhow::bail!("decoration button layout contains duplicates");
        }
        Ok(())
    }

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
            button_layout: self.buttons.layout.clone(),
            button_side: self.buttons.side,
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
    validate(&config)?;
    Ok(config)
}

/// Validate a configuration received through an integration before it is
/// persisted or applied to the running compositor.
pub fn validate(config: &CompositorConfig) -> Result<()> {
    for (index, binding) in config.bindings.iter().enumerate() {
        binding.validate(index + 1)?;
    }
    for (index, rule) in config.rules.iter().enumerate() {
        rule.validate(index + 1)?;
    }
    for (index, autostart) in config.autostart.iter().enumerate() {
        autostart.validate(index + 1)?;
    }
    for (name, output) in &config.outputs {
        output.validate(name)?;
    }
    config.input.validate()?;
    config.workspaces.validate()?;
    config.window.validate()?;
    config.decorations.validate()?;
    config.animations.validate()?;
    config.cursor.validate()?;
    Ok(())
}

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(100);

/// Watches the configuration roots and reloads once changes settle.
///
/// Only existing roots are observed: creating a new configuration root still
/// requires a restart, while edits to an active configuration apply live.
pub fn watch(handle: &LoopHandle<'static, BlairState>, config: &CompositorConfig) -> Result<()> {
    if !config.general.hot_reload {
        tracing::info!("configuration hot reload disabled");
        return Ok(());
    }
    let paths = ConfigPaths::default();
    let (sender, channel) = calloop::channel::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })
    .context("failed to initialize configuration watcher")?;

    let mut watched = 0;
    for directory in [&paths.system_dir, &paths.user_dir] {
        if !directory.is_dir() {
            continue;
        }
        match watcher.watch(directory, RecursiveMode::Recursive) {
            Ok(()) => {
                watched += 1;
                tracing::debug!(path = %directory.display(), "watching config directory");
            }
            Err(error) => {
                tracing::warn!(%error, path = %directory.display(), "failed to watch config directory")
            }
        }
    }
    if watched == 0 {
        tracing::info!("no configuration directory to watch");
        return Ok(());
    }

    let mut pending: Option<RegistrationToken> = None;
    let timer_handle = handle.clone();
    handle
        .insert_source(channel, move |event, _, state: &mut BlairState| {
            // Keeps the watcher alive for as long as the source is registered.
            let _ = &watcher;
            let calloop::channel::Event::Msg(event) = event else {
                return;
            };
            match event {
                Ok(event) => {
                    tracing::trace!(?event.kind, paths = ?event.paths, "config filesystem event")
                }
                Err(error) => {
                    tracing::warn!(%error, "config watcher error");
                    return;
                }
            }
            if !state.config.general.hot_reload {
                return;
            }
            if let Some(token) = pending.take() {
                timer_handle.remove(token);
            }
            pending = timer_handle
                .insert_source(
                    Timer::from_duration(RELOAD_DEBOUNCE),
                    |_, _, state: &mut BlairState| {
                        reload(state);
                        TimeoutAction::Drop
                    },
                )
                .ok();
        })
        .map_err(|error| anyhow::anyhow!("config watcher source: {error}"))?;
    Ok(())
}

fn reload(state: &mut BlairState) {
    match load_from_paths(&ConfigPaths::default()) {
        Ok(next) if next == state.config => {}
        Ok(next) => {
            state.apply_config(next);
            tracing::info!("configuration reloaded");
        }
        Err(error) => {
            tracing::error!(%error, "configuration reload failed; keeping previous configuration")
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
    // Written through a temporary file so the watcher never reads a partial
    // configuration and a failed write cannot truncate the previous one.
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, toml)
        .with_context(|| format!("failed to write config to {}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("failed to replace config {}", path.display()))
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
        assert!(config.autostart.is_empty());
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

    #[test]
    fn parses_plural_decorations_and_legacy_alias() {
        let config: CompositorConfig = toml::from_str(
            "[decorations]\nmode = \"server\"\nborder_width = 2\ncorner_radius = 8\ntitlebar_height = 28\n\n[decorations.buttons]\nlayout = [\"close\", \"minimize\"]\nside = \"left\"\n",
        )
        .unwrap();
        assert_eq!(config.decorations.mode, DecorationModeConfig::Server);
        assert_eq!(config.decorations.buttons.side, DecorationButtonSide::Left);

        let legacy: CompositorConfig = toml::from_str("[decoration]\ncorner_radius = 4\n").unwrap();
        assert_eq!(legacy.decorations.corner_radius, 4);
    }

    #[test]
    fn parses_autostart_with_optional_restart() {
        let config: CompositorConfig = toml::from_str(
            "[[autostart]]\ncommand = \"waybar\"\nrestart = true\n\n[[autostart]]\ncommand = \"mako\"\n",
        )
        .unwrap();
        assert_eq!(config.autostart.len(), 2);
        assert!(config.autostart[0].restart);
        assert!(!config.autostart[1].restart);
        assert!(config.autostart[0].validate(1).is_ok());
    }

    #[test]
    fn focus_defaults_are_click_to_focus() {
        let config = CompositorConfig::default();
        assert_eq!(config.focus.policy, FocusPolicy::Click);
        assert!(config.focus.raise_on_focus);
        assert!(config.focus.focus_new_windows);
        assert!(config.focus.focus_previous_on_close);
        assert!(!config.focus.warp_cursor);
    }

    #[test]
    fn parses_animation_settings() {
        let config: CompositorConfig = toml::from_str(
            "[animations]\nenabled = true\n\n[animations.window_open]\nduration = 150\ncurve = \"ease-out\"\n\n[animations.workspace]\nduration = 200\ncurve = \"ease-in-out\"\n",
        )
        .unwrap();
        config.animations.validate().unwrap();
        assert_eq!(config.animations.window_open.duration, 150);
        assert_eq!(config.animations.workspace.curve, AnimationCurve::EaseInOut);
    }
}
