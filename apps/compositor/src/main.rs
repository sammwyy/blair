mod backend;
mod config;
mod dbus;
mod decorations;
mod input;
mod logging;
mod render;
mod shortcuts;
mod state;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let log_path = logging::setup();
    logging::install_panic_hook();
    logging::install_signal_handlers();

    tracing::info!(
        log = ?log_path.as_deref(),
        pid = std::process::id(),
        "Blair compositor starting"
    );

    let config = config::load_or_default().context("failed to load compositor config")?;
    tracing::info!(
        backend = %config.general.backend,
        primary_client = %config.general.primary_client,
        "config loaded"
    );

    backend::run(config)
}
