use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::config::PowerConfig;

struct Backlight {
    brightness: PathBuf,
}

impl Backlight {
    fn detect() -> Option<Self> {
        Self::under(Path::new("/sys/class/backlight"))
    }

    fn under(root: &Path) -> Option<Self> {
        fs::read_dir(root)
            .ok()?
            .filter_map(Result::ok)
            .find_map(|entry| {
                let brightness = entry.path().join("brightness");
                brightness.is_file().then_some(Self { brightness })
            })
    }

    fn read(&self) -> Result<u32, String> {
        fs::read_to_string(&self.brightness)
            .map_err(|error| format!("{}: {error}", self.brightness.display()))?
            .trim()
            .parse()
            .map_err(|error| format!("{}: {error}", self.brightness.display()))
    }

    fn write(&self, value: u32) -> Result<(), String> {
        fs::write(&self.brightness, value.to_string())
            .map_err(|error| format!("{}: {error}", self.brightness.display()))
    }
}

pub struct IdleDimming {
    backlight: Option<Backlight>,
    last_activity: Instant,
    original: Option<u32>,
    dimmed: bool,
    failed: bool,
}

impl Default for IdleDimming {
    fn default() -> Self {
        Self {
            backlight: Backlight::detect(),
            last_activity: Instant::now(),
            original: None,
            dimmed: false,
            failed: false,
        }
    }
}

impl IdleDimming {
    #[cfg(test)]
    pub fn without_backlight() -> Self {
        Self {
            backlight: None,
            ..Self::default()
        }
    }

    pub fn activity(&mut self) {
        self.last_activity = Instant::now();
        self.failed = false;
        self.restore();
    }

    pub fn deadline(&self, timeout: u64) -> Instant {
        self.last_activity + Duration::from_secs(timeout)
    }

    pub fn tick(&mut self, config: &PowerConfig, inhibited: bool) {
        if !config.idle_dim || inhibited {
            self.restore();
            return;
        }
        if self.dimmed
            || self.failed
            || self.last_activity.elapsed().as_secs() < config.idle_dim_timeout
        {
            return;
        }
        let Some(backlight) = self.backlight.as_ref() else {
            self.fail("no kernel backlight is available for idle dimming".into());
            return;
        };
        let original = match backlight.read() {
            Ok(value) => value,
            Err(error) => {
                self.fail(format!(
                    "could not read backlight for idle dimming: {error}"
                ));
                return;
            }
        };
        if let Err(error) = backlight.write(dimmed_level(original)) {
            self.fail(format!("could not dim the backlight: {error}"));
            return;
        }
        self.original = Some(original);
        self.dimmed = true;
        tracing::info!(
            from = original,
            to = dimmed_level(original),
            "backlight dimmed for idle session"
        );
    }

    fn restore(&mut self) {
        let Some(original) = self.original.take() else {
            return;
        };
        let Some(backlight) = self.backlight.as_ref() else {
            return;
        };
        match backlight.write(original) {
            Ok(()) => {
                self.dimmed = false;
                tracing::info!(
                    brightness = original,
                    "restored backlight after idle dimming"
                );
            }
            Err(error) => {
                self.original = Some(original);
                self.fail(format!("could not restore the backlight: {error}"));
            }
        }
    }

    fn fail(&mut self, error: String) {
        self.failed = true;
        tracing::warn!(%error, "idle dimming unavailable");
    }
}

fn dimmed_level(current: u32) -> u32 {
    (current / 2).max(1)
}

#[cfg(test)]
mod tests {
    use super::dimmed_level;

    #[test]
    fn dimming_preserves_a_visible_backlight() {
        assert_eq!(dimmed_level(0), 1);
        assert_eq!(dimmed_level(1), 1);
        assert_eq!(dimmed_level(80), 40);
    }
}
