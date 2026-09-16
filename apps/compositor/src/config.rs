use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::decorations::DecorationTheme;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CompositorConfig {
    pub general: GeneralConfig,
    pub window: WindowConfig,
    pub decoration: DecorationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Command started after the Wayland socket is ready.
    pub primary_client: String,
    pub spawn_primary_client: bool,
    pub backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub default_width: i32,
    pub default_height: i32,
    pub server_side_decorations: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DecorationConfig {
    pub titlebar_height: i32,
    pub border_width: i32,
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
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            default_width: 900,
            default_height: 600,
            server_side_decorations: true,
        }
    }
}

impl Default for DecorationConfig {
    fn default() -> Self {
        Self {
            titlebar_height: 32,
            border_width: 4,
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

pub fn user_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("blair")
        .join("compositor.toml")
}

pub fn load_or_default() -> Result<CompositorConfig> {
    let path = user_config_path();
    if !path.exists() {
        return write_default(&path);
    }
    read_config(&path)
}

fn read_config(path: &Path) -> Result<CompositorConfig> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("invalid TOML in {}", path.display()))
}

fn write_default(path: &Path) -> Result<CompositorConfig> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config dir {}", parent.display()))?;
    }
    let config = CompositorConfig::default();
    let toml_str = toml::to_string_pretty(&config).context("failed to serialize default config")?;
    fs::write(path, &toml_str)
        .with_context(|| format!("failed to write default config to {}", path.display()))?;
    tracing::info!(path = %path.display(), "created default compositor config");
    Ok(config)
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
    fn write_default_creates_the_parent_dir_and_writes_compiled_in_defaults() {
        let dir = scratch_dir("write-default");
        let user_path = dir.join("nested").join("compositor.toml");

        let config = write_default(&user_path).unwrap();
        assert_eq!(config.general.primary_client, "coconut");
        assert!(user_path.exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_config_returns_the_file_contents_verbatim() {
        let dir = scratch_dir("existing-user-config");
        let user_path = dir.join("compositor.toml");
        fs::write(&user_path, "[general]\nprimary_client = \"already-mine\"\n").unwrap();

        let config = read_config(&user_path).unwrap();
        assert_eq!(config.general.primary_client, "already-mine");

        fs::remove_dir_all(&dir).ok();
    }
}
