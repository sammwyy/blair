mod drm;
mod winit;

use anyhow::Result;

use crate::config::CompositorConfig;

pub fn run(config: CompositorConfig) -> Result<()> {
    let in_display =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();

    match config.general.backend.as_str() {
        "drm" | "kms" => {
            tracing::info!("config requested DRM/KMS backend");
            drm::run(config)
        }
        "winit" | "nested" => {
            tracing::info!("config requested winit backend (nested)");
            winit::run(config)
        }
        "auto" if in_display => {
            tracing::info!("parent display detected — using winit backend (nested)");
            winit::run(config)
        }
        "auto" => {
            tracing::info!("no parent display — using DRM/KMS backend");
            drm::run(config)
        }
        other => {
            tracing::warn!(backend = other, "unknown backend, falling back to auto");
            if in_display {
                winit::run(config)
            } else {
                drm::run(config)
            }
        }
    }
}
