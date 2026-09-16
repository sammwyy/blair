mod backend;
mod config;
mod dbus;
mod decorations;
mod input;
mod logging;
mod render;
mod shortcuts;
mod state;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let primary_client = parse_primary_client(std::env::args().skip(1))?;
    let log_path = logging::setup();
    logging::install_panic_hook();
    logging::install_signal_handlers();

    tracing::info!(
        log = ?log_path.as_deref(),
        pid = std::process::id(),
        "Blair compositor starting"
    );

    let mut config = config::load_or_default().context("failed to load compositor config")?;
    if let Some(primary_client) = primary_client {
        config.general.primary_client = primary_client;
        config.general.spawn_primary_client = true;
    }
    tracing::info!(
        backend = %config.general.backend,
        primary_client = %config.general.primary_client,
        "config loaded"
    );

    backend::run(config)
}

fn parse_primary_client(args: impl IntoIterator<Item = String>) -> Result<Option<String>> {
    let mut args = args.into_iter();
    let mut primary_client = None;

    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--primary-client" => args
                .next()
                .context("missing command after --primary-client")?,
            "--help" | "-h" => {
                println!("Usage: blair [--primary-client <command>]");
                println!(
                    "\n  --primary-client  Start this command instead of the configured client."
                );
                std::process::exit(0);
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--primary-client=") {
                    value.to_owned()
                } else {
                    bail!("unknown argument: {arg}");
                }
            }
        };

        if value.trim().is_empty() {
            bail!("primary client command cannot be empty");
        }
        if primary_client.replace(value).is_some() {
            bail!("primary client command specified more than once");
        }
    }

    Ok(primary_client)
}

#[cfg(test)]
mod tests {
    use super::parse_primary_client;

    #[test]
    fn parses_primary_client_override() {
        let command =
            parse_primary_client(["--primary-client".into(), "shell --debug".into()]).unwrap();
        assert_eq!(command.as_deref(), Some("shell --debug"));
    }

    #[test]
    fn accepts_equals_syntax() {
        let command = parse_primary_client(["--primary-client=another-shell".into()]).unwrap();
        assert_eq!(command.as_deref(), Some("another-shell"));
    }

    #[test]
    fn rejects_unknown_or_duplicate_arguments() {
        assert!(parse_primary_client(["--unknown".into()]).is_err());
        assert!(parse_primary_client([
            "--primary-client".into(),
            "one".into(),
            "--primary-client".into(),
            "two".into(),
        ])
        .is_err());
    }
}
