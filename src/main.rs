mod accessibility;
mod backend;
mod blur;
mod config;
mod cursor;
mod decorations;
mod grabs;
mod handlers;
mod hot_corners;
mod idle_dim;
mod input;
mod input_devices;
mod integrations;
mod logging;
mod outputs;
mod render;
mod rules;
mod screencopy;
mod session_lock;
mod shortcuts;
mod state;
mod stats;
#[cfg(test)]
mod test_support;
mod window_integration;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    #[cfg(feature = "profile-with-tracy")]
    tracy_client::Client::start();

    let startup = parse_startup_options(std::env::args().skip(1))?;
    let log_path = logging::setup();
    logging::install_panic_hook();
    logging::install_signal_handlers();

    tracing::info!(
        log = ?log_path.as_deref(),
        pid = std::process::id(),
        "Blair compositor starting"
    );

    config::set_active_profile(&startup.profile);
    let mut config = config::load_active().context("failed to load compositor config")?;
    config.autostart.extend(startup.run);
    tracing::info!(
        profile = %startup.profile,
        backend = %config.general.backend,
        kiosk = config.general.kiosk,
        autostart_count = config.autostart.len(),
        "config loaded"
    );

    backend::run(config)
}

struct StartupOptions {
    run: Vec<config::AutostartConfig>,
    profile: String,
}

fn parse_startup_options(args: impl IntoIterator<Item = String>) -> Result<StartupOptions> {
    let mut args = args.into_iter();
    let mut run = Vec::new();
    let mut profile = config::USER_PROFILE.to_owned();

    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--run" => args.next().context("missing command after --run")?,
            "--profile" => {
                profile = parse_profile(&args.next().context("missing profile name")?)?;
                continue;
            }
            "--help" | "-h" => {
                println!("Usage: blair [--profile <id>] [--run <command>]...");
                println!("\n  --run      Add a command to this session's autostart list.");
                println!(
                    "  --profile  Configuration profile. \"{}\" (the default) loads the system and user directories.",
                    config::USER_PROFILE
                );
                println!("             Any other id loads /etc/blair/profiles/<id>.toml.");
                std::process::exit(0);
            }
            _ => {
                if let Some(value) = arg.strip_prefix("--run=") {
                    value.to_owned()
                } else if let Some(value) = arg.strip_prefix("--profile=") {
                    profile = parse_profile(value)?;
                    continue;
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

    Ok(StartupOptions { run, profile })
}

fn parse_profile(value: &str) -> Result<String> {
    config::validate_profile_id(value)?;
    Ok(value.to_owned())
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
        assert_eq!(options.profile, crate::config::USER_PROFILE);
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

    #[test]
    fn defaults_to_the_user_profile() {
        let options = parse_startup_options(std::iter::empty()).unwrap();
        assert_eq!(options.profile, crate::config::USER_PROFILE);
    }

    #[test]
    fn parses_a_named_profile() {
        let options = parse_startup_options(["--profile".into(), "greeter".into()]).unwrap();
        assert_eq!(options.profile, "greeter");
        let options = parse_startup_options(["--profile=kiosk".into()]).unwrap();
        assert_eq!(options.profile, "kiosk");
    }

    #[test]
    fn rejects_profile_ids_that_are_not_file_names() {
        assert!(parse_startup_options(["--profile".into()]).is_err());
        assert!(parse_startup_options(["--profile=".into()]).is_err());
        assert!(parse_startup_options(["--profile=../greeter".into()]).is_err());
        assert!(parse_startup_options(["--profile=greeter.toml".into()]).is_err());
        assert!(parse_startup_options(["--profile=/etc/blair/profiles/greeter".into()]).is_err());
    }
}
