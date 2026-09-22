use tracing_subscriber::EnvFilter;

fn state_log_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("blair").join("compositor.log"))
}

/// Installs tracing and returns the log path when file logging is available.
pub fn setup() -> Option<std::path::PathBuf> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("blair=debug,warn"));

    if let Some(path) = state_log_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => {
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .init();
                return Some(path);
            }
            Err(err) => {
                eprintln!("blair: cannot open {}: {err}", path.display());
            }
        }
    }

    tracing_subscriber::fmt().with_env_filter(filter).init();
    None
}

/// Installs SIGINT, SIGTERM, and SIGHUP handlers.
pub fn install_signal_handlers() {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;

    let mut signals = match Signals::new([SIGINT, SIGTERM, SIGHUP]) {
        Ok(signals) => signals,
        Err(err) => {
            tracing::warn!(%err, "failed to install signal handlers");
            return;
        }
    };

    std::thread::Builder::new()
        .name("blair-signals".into())
        .spawn(move || {
            if let Some(signal) = signals.forever().next() {
                tracing::error!(signal, "received fatal signal — forcing exit");
                let _ = std::io::Write::flush(&mut std::io::stderr().lock());
                let _ = std::io::Write::flush(&mut std::io::stdout().lock());
                std::process::exit(128 + signal);
            }
        })
        .expect("failed to spawn signal handler thread");
}

/// Installs the panic hook used by the compositor process.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "<unknown>".into());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");

        tracing::error!(
            location = %location,
            payload = %payload,
            "compositor panicked — forcing exit to release DRM master"
        );

        let _ = std::io::Write::flush(&mut std::io::stderr().lock());
        let _ = std::io::Write::flush(&mut std::io::stdout().lock());

        default_hook(info);

        std::process::exit(101);
    }));
}
