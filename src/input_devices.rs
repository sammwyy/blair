use crate::config::InputConfig;
use blair_protocol::{InputCapabilities, InputDeviceInfo};
use smithay::{
    backend::input::Device as InputDevice,
    reexports::input::{AccelProfile, Device, ScrollMethod},
};

#[derive(Default)]
pub struct InputDevices {
    native: bool,
    devices: Vec<NativeDevice>,
    errors: Vec<String>,
}

#[derive(Debug)]
pub struct InputConfigRollback(Vec<Snapshot>);

impl InputDevices {
    pub fn enable_native(&mut self) {
        self.native = true;
    }

    pub fn capabilities(&self) -> InputCapabilities {
        InputCapabilities {
            backend: if self.native { "drm" } else { "winit" }.into(),
            devices: self
                .devices
                .iter()
                .map(|device| device.info.clone())
                .collect(),
            errors: self.errors.clone(),
        }
    }

    pub fn add(&mut self, device: Device, config: &InputConfig) {
        let mut device = NativeDevice::new(device);
        self.errors = configure_transaction(std::slice::from_mut(&mut device), config)
            .err()
            .unwrap_or_default();
        self.log_errors();
        self.devices.push(device);
    }

    pub fn remove(&mut self, device: &Device) {
        self.devices.retain(|known| &known.device != device);
    }

    pub fn configure(
        &mut self,
        config: &InputConfig,
        previous: &InputConfig,
    ) -> Result<InputConfigRollback, String> {
        self.preflight(config, previous)?;
        if !self.native {
            return Ok(InputConfigRollback(Vec::new()));
        }
        match configure_transaction(&mut self.devices, config) {
            Ok(snapshots) => {
                self.errors.clear();
                Ok(InputConfigRollback(snapshots))
            }
            Err(errors) => {
                self.errors = errors;
                self.log_errors();
                Err(self.errors.join("; "))
            }
        }
    }

    pub fn rollback(&mut self, rollback: InputConfigRollback) -> Result<(), String> {
        self.errors = restore_transaction(&mut self.devices, rollback.0);
        self.log_errors();
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors.join("; "))
        }
    }

    pub fn reapply(&mut self, config: &InputConfig) -> Result<(), String> {
        if !self.native {
            return Ok(());
        }
        self.errors = configure_transaction(&mut self.devices, config)
            .err()
            .unwrap_or_default();
        self.log_errors();
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors.join("; "))
        }
    }

    fn preflight(&mut self, config: &InputConfig, previous: &InputConfig) -> Result<(), String> {
        if config.mouse == previous.mouse && config.touchpad == previous.touchpad {
            return Ok(());
        }
        if !self.native {
            self.errors = vec!["Pointer and touchpad settings are controlled by the host compositor in nested sessions".into()];
            self.log_errors();
            return Err(self.errors.join("; "));
        }
        let mut errors = Vec::new();
        if config.mouse.sensitivity != previous.mouse.sensitivity
            && !self.devices.iter().any(|device| device.info.sensitivity)
        {
            errors.push("No connected device supports pointer sensitivity".into());
        }
        if config.mouse.acceleration != previous.mouse.acceleration
            && !self.devices.iter().any(|device| {
                device
                    .info
                    .acceleration_profiles
                    .iter()
                    .any(|profile| profile == &config.mouse.acceleration)
            })
        {
            errors.push(format!(
                "No connected device supports the {} acceleration profile",
                config.mouse.acceleration
            ));
        }
        if config.touchpad.tap != previous.touchpad.tap
            && !self.devices.iter().any(|device| device.info.tap)
        {
            errors.push("No connected touchpad supports tap to click".into());
        }
        if config.touchpad.natural_scroll != previous.touchpad.natural_scroll
            && !self.devices.iter().any(|device| device.info.natural_scroll)
        {
            errors.push("No connected touchpad supports natural scrolling".into());
        }
        if config.touchpad.disable_while_typing != previous.touchpad.disable_while_typing
            && !self
                .devices
                .iter()
                .any(|device| device.info.disable_while_typing)
        {
            errors.push("No connected touchpad supports disable while typing".into());
        }
        self.errors = errors;
        self.log_errors();
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors.join("; "))
        }
    }

    fn log_errors(&self) {
        for error in &self.errors {
            tracing::warn!(%error, "input configuration failed");
        }
    }
}

trait ConfigurableDevice {
    type Snapshot;
    fn snapshot(&self) -> Self::Snapshot;
    fn apply(&mut self, config: &InputConfig) -> Vec<String>;
    fn restore(&mut self, snapshot: Self::Snapshot) -> Vec<String>;
}

fn configure_transaction<D: ConfigurableDevice>(
    devices: &mut [D],
    config: &InputConfig,
) -> Result<Vec<D::Snapshot>, Vec<String>> {
    let snapshots: Vec<_> = devices.iter().map(ConfigurableDevice::snapshot).collect();
    let mut errors: Vec<_> = devices
        .iter_mut()
        .flat_map(|device| device.apply(config))
        .collect();
    if errors.is_empty() {
        return Ok(snapshots);
    }
    errors.extend(restore_transaction(devices, snapshots));
    Err(errors)
}

fn restore_transaction<D: ConfigurableDevice>(
    devices: &mut [D],
    snapshots: Vec<D::Snapshot>,
) -> Vec<String> {
    if devices.len() != snapshots.len() {
        return vec!["Rollback failed: the input device list changed during configuration".into()];
    }
    devices
        .iter_mut()
        .zip(snapshots)
        .flat_map(|(device, snapshot)| device.restore(snapshot))
        .map(|error| format!("Rollback failed: {error}"))
        .collect()
}

struct NativeDevice {
    device: Device,
    info: InputDeviceInfo,
}

impl NativeDevice {
    fn new(device: Device) -> Self {
        let touchpad = is_touchpad(
            device.config_tap_finger_count(),
            device.config_scroll_default_method(),
        );
        let info = InputDeviceInfo {
            id: InputDevice::id(&device),
            name: device.name().into(),
            touchpad,
            sensitivity: device.config_accel_is_available(),
            acceleration_profiles: device
                .config_accel_profiles()
                .into_iter()
                .filter_map(|profile| match profile {
                    AccelProfile::Flat => Some("flat".into()),
                    AccelProfile::Adaptive => Some("adaptive".into()),
                    _ => None,
                })
                .collect(),
            tap: device.config_tap_finger_count() > 0,
            natural_scroll: touchpad && device.config_scroll_has_natural_scroll(),
            disable_while_typing: device.config_dwt_is_available(),
        };
        Self { device, info }
    }
}

pub(crate) fn is_touchpad(tap_fingers: u32, scroll_method: Option<ScrollMethod>) -> bool {
    tap_fingers > 0
        || matches!(
            scroll_method,
            Some(ScrollMethod::TwoFinger | ScrollMethod::Edge)
        )
}

#[derive(Debug)]
struct Snapshot {
    profile: Option<AccelProfile>,
    speed: f64,
    tap: bool,
    natural: bool,
    dwt: bool,
}

impl ConfigurableDevice for NativeDevice {
    type Snapshot = Snapshot;

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            profile: self.device.config_accel_profile(),
            speed: self.device.config_accel_speed(),
            tap: self.device.config_tap_enabled(),
            natural: self.device.config_scroll_natural_scroll_enabled(),
            dwt: self.device.config_dwt_enabled(),
        }
    }

    fn apply(&mut self, config: &InputConfig) -> Vec<String> {
        let mut errors = Vec::new();
        let name = &self.info.name;
        let mut report = |setting: &str, result: smithay::reexports::input::DeviceConfigResult| {
            if let Err(error) = result {
                errors.push(format!("{name}: {setting}: {error:?}"));
            }
        };
        if !self.info.acceleration_profiles.is_empty() {
            let profile = if config.mouse.acceleration == "flat" {
                AccelProfile::Flat
            } else {
                AccelProfile::Adaptive
            };
            if self
                .info
                .acceleration_profiles
                .contains(&config.mouse.acceleration)
                && self.device.config_accel_profile() != Some(profile)
            {
                report(
                    "acceleration",
                    self.device.config_accel_set_profile(profile),
                );
            }
        }
        if self.info.sensitivity && self.device.config_accel_speed() != config.mouse.sensitivity {
            report(
                "sensitivity",
                self.device.config_accel_set_speed(config.mouse.sensitivity),
            );
        }
        if self.info.tap && self.device.config_tap_enabled() != config.touchpad.tap {
            report(
                "tap to click",
                self.device.config_tap_set_enabled(config.touchpad.tap),
            );
        }
        if self.info.natural_scroll
            && self.device.config_scroll_natural_scroll_enabled() != config.touchpad.natural_scroll
        {
            report(
                "natural scrolling",
                self.device
                    .config_scroll_set_natural_scroll_enabled(config.touchpad.natural_scroll),
            );
        }
        if self.info.disable_while_typing
            && self.device.config_dwt_enabled() != config.touchpad.disable_while_typing
        {
            report(
                "disable while typing",
                self.device
                    .config_dwt_set_enabled(config.touchpad.disable_while_typing),
            );
        }
        errors
    }

    fn restore(&mut self, snapshot: Snapshot) -> Vec<String> {
        let mut errors = Vec::new();
        let name = &self.info.name;
        let mut report = |setting: &str, result: smithay::reexports::input::DeviceConfigResult| {
            if let Err(error) = result {
                errors.push(format!("{name}: {setting}: {error:?}"));
            }
        };
        if let Some(profile) = snapshot.profile {
            report(
                "acceleration",
                self.device.config_accel_set_profile(profile),
            );
        }
        if self.info.sensitivity {
            report(
                "sensitivity",
                self.device.config_accel_set_speed(snapshot.speed),
            );
        }
        if self.info.tap {
            report(
                "tap to click",
                self.device.config_tap_set_enabled(snapshot.tap),
            );
        }
        if self.info.natural_scroll {
            report(
                "natural scrolling",
                self.device
                    .config_scroll_set_natural_scroll_enabled(snapshot.natural),
            );
        }
        if self.info.disable_while_typing {
            report(
                "disable while typing",
                self.device.config_dwt_set_enabled(snapshot.dwt),
            );
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DeviceFixture {
        speed: f64,
        reject: bool,
        reject_rollback: bool,
    }
    impl ConfigurableDevice for DeviceFixture {
        type Snapshot = f64;
        fn snapshot(&self) -> f64 {
            self.speed
        }
        fn apply(&mut self, config: &InputConfig) -> Vec<String> {
            self.speed = config.mouse.sensitivity;
            if self.reject {
                vec!["device rejected sensitivity".into()]
            } else {
                Vec::new()
            }
        }
        fn restore(&mut self, previous: f64) -> Vec<String> {
            self.speed = previous;
            if self.reject_rollback {
                vec!["device rejected rollback".into()]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn failures_restore_every_device_and_include_rollback_failures() {
        let mut devices = [
            DeviceFixture {
                speed: 0.1,
                reject: false,
                reject_rollback: false,
            },
            DeviceFixture {
                speed: -0.3,
                reject: true,
                reject_rollback: true,
            },
        ];
        let mut config = InputConfig::default();
        config.mouse.sensitivity = 0.8;
        let errors = configure_transaction(&mut devices, &config).unwrap_err();
        assert_eq!(devices[0].speed, 0.1);
        assert_eq!(devices[1].speed, -0.3);
        assert!(errors[0].contains("sensitivity"));
        assert!(errors[1].contains("Rollback failed"));
        devices[1].reject = false;
        assert!(configure_transaction(&mut devices, &config).is_ok());
        assert!(devices.iter().all(|device| device.speed == 0.8));
    }

    #[test]
    fn successful_device_updates_can_restore_the_actual_previous_state() {
        let mut devices = [DeviceFixture {
            speed: 0.35,
            reject: false,
            reject_rollback: false,
        }];
        let mut config = InputConfig::default();
        config.mouse.sensitivity = 0.8;
        let rollback = configure_transaction(&mut devices, &config).unwrap();
        assert_eq!(devices[0].speed, 0.8);
        assert!(restore_transaction(&mut devices, rollback).is_empty());
        assert_eq!(devices[0].speed, 0.35);
    }

    #[test]
    fn nested_sessions_report_host_control_without_claiming_native_devices() {
        let mut devices = InputDevices::default();
        assert_eq!(devices.capabilities().backend, "winit");
        assert!(devices.capabilities().devices.is_empty());
        let previous = InputConfig::default();
        let mut next = previous.clone();
        next.mouse.sensitivity = 0.5;
        assert!(devices
            .configure(&next, &previous)
            .unwrap_err()
            .contains("host compositor"));
        assert!(!devices.capabilities().errors.is_empty());
        next.mouse = previous.mouse.clone();
        next.keyboard.layout = "es".into();
        assert!(devices.configure(&next, &previous).is_ok());
    }
}
