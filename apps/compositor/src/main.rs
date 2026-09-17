mod backend;
mod config;
mod decorations;
mod input;
mod integrations;
mod logging;
mod render;
mod shortcuts;
mod state;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let startup = parse_startup_options(std::env::args().skip(1))?;
    let log_path = logging::setup();
    logging::install_panic_hook();
    logging::install_signal_handlers();

    tracing::info!(
        log = ?log_path.as_deref(),
        pid = std::process::id(),
        "Blair compositor starting"
    );

    let mut config = config::load_or_default().context("failed to load compositor config")?;
    config.autostart.extend(startup.run);
    tracing::info!(
        backend = %config.general.backend,
        autostart_count = config.autostart.len(),
        "config loaded"
    );

    backend::run(config)
}

struct StartupOptions {
    run: Vec<config::AutostartConfig>,
}

fn parse_startup_options(args: impl IntoIterator<Item = String>) -> Result<StartupOptions> {
    let mut args = args.into_iter();
    let mut run = Vec::new();

    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--run" => args.next().context("missing command after --run")?,
            "--help" | "-h" => {
                println!("Usage: blair [--run <command>]...");
                println!("\n  --run  Add a command to this session's autostart list.");
                std::process::exit(0);
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--run=") {
                    value.to_owned()
                } else {
                    bail!("unknown argument: {arg}");
                }
            }
        };

        if value.trim().is_empty() {
            bail!("--run command cannot be empty");
        }
        run.push(config::AutostartConfig {
            command: value,
            restart: false,
        });
    }

    Ok(StartupOptions { run })
}

#[cfg(test)]
mod tests {
    use super::parse_startup_options;

    #[test]
    fn parses_multiple_run_commands() {
        let options = parse_startup_options([
            "--run".into(),
            "waybar".into(),
            "--run=swaybg -i wallpaper.png".into(),
        ])
        .unwrap();
        assert_eq!(options.run.len(), 2);
        assert_eq!(options.run[1].command, "swaybg -i wallpaper.png");
    }

    #[test]
    fn accepts_equals_syntax() {
        let options = parse_startup_options(["--run=another-shell".into()]).unwrap();
        assert_eq!(options.run[0].command, "another-shell");
    }

    #[test]
    fn rejects_unknown_or_duplicate_arguments() {
        assert!(parse_startup_options(["--unknown".into()]).is_err());
        assert!(parse_startup_options(["--run".into(), " ".into()]).is_err());
    }
}
