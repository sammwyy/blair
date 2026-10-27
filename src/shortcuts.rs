pub use blair_protocol::ShortcutModifiers as ShortcutMods;
use blair_protocol::{is_modifier_keycode, ShortcutAccelerator};
use std::collections::HashSet;

#[derive(Debug, Default)]
pub struct ShortcutRegistry {
    bindings: Vec<ShortcutBinding>,
    pressed_keys: HashSet<u32>,
    active_bindings: HashSet<(String, String)>,
    pending_modifiers: Option<PhysicalMods>,
    modifier_chord_used: bool,
    last_key_released: bool,
}

#[derive(Debug, Clone)]
struct ShortcutBinding {
    client: String,
    id: String,
    mods: ShortcutMods,
    keys: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivatedShortcut {
    pub client: String,
    pub id: String,
}

pub type PhysicalMods = ShortcutMods;

pub(crate) const MAX_BINDINGS_PER_CLIENT: usize = 256;

impl ShortcutRegistry {
    pub fn bind(&mut self, client: &str, id: &str, accelerator: &str) -> bool {
        if self
            .bindings
            .iter()
            .filter(|existing| existing.client == client)
            .count()
            >= MAX_BINDINGS_PER_CLIENT
            && !self
                .bindings
                .iter()
                .any(|existing| existing.client == client && existing.id == id)
        {
            tracing::warn!(client, "shortcut limit reached");
            return false;
        }
        let binding = match parse_binding(client, id, accelerator) {
            Ok(binding) => binding,
            Err(err) => {
                tracing::warn!(id, accelerator, error = %err, "ignoring invalid shortcut binding");
                return false;
            }
        };
        self.bindings
            .retain(|existing| existing.client != client || existing.id != id);
        self.active_bindings
            .remove(&(client.to_string(), id.to_string()));
        self.bindings.push(binding);
        tracing::info!(id, accelerator, "shortcut bound");
        true
    }

    pub fn unbind(&mut self, client: &str, id: &str) {
        self.bindings
            .retain(|existing| existing.client != client || existing.id != id);
        self.active_bindings
            .remove(&(client.to_string(), id.to_string()));
    }

    pub fn unregister_client(&mut self, client: &str) {
        let previous = self.bindings.len();
        self.bindings.retain(|binding| binding.client != client);
        self.active_bindings
            .retain(|(binding_client, _)| binding_client != client);
        let removed = previous - self.bindings.len();
        if removed > 0 {
            tracing::info!(client, removed, "client shortcuts unregistered");
        }
    }

    /// Drops the physical key state, used when the compositor stops
    /// receiving key events (VT switch, host focus loss).
    pub fn clear_pressed(&mut self) {
        self.pressed_keys.clear();
        self.active_bindings.clear();
        self.pending_modifiers = None;
        self.modifier_chord_used = false;
    }

    pub fn update_key(&mut self, keycode: u32, pressed: bool) {
        self.last_key_released = !pressed;
        if pressed {
            self.pressed_keys.insert(keycode);
        } else {
            self.pressed_keys.remove(&keycode);
        }
    }

    pub fn modifiers(&self) -> PhysicalMods {
        let mut modifiers = PhysicalMods::default();
        for code in &self.pressed_keys {
            update_physical_mods(&mut modifiers, *code, true);
        }
        modifiers
    }

    pub fn maybe_activate_physical(&mut self, mods: PhysicalMods) -> Vec<ActivatedShortcut> {
        if self
            .pressed_keys
            .iter()
            .any(|code| !is_modifier_keycode(code.saturating_sub(8)))
        {
            self.modifier_chord_used = true;
        }
        let released = if mods == PhysicalMods::default() {
            let pending = self
                .pending_modifiers
                .take()
                .filter(|_| !self.modifier_chord_used);
            self.modifier_chord_used = false;
            pending
        } else {
            if self
                .pending_modifiers
                .is_none_or(|pending| modifier_count(mods) > modifier_count(pending))
            {
                self.pending_modifiers = Some(mods);
            }
            None
        };
        let mut activated: Vec<_> = self
            .bindings
            .iter()
            .filter(|binding| binding.keys.is_empty() && released == Some(binding.mods))
            .map(|binding| ActivatedShortcut {
                client: binding.client.clone(),
                id: binding.id.clone(),
            })
            .collect();
        let matching: HashSet<_> = self
            .bindings
            .iter()
            .filter(|binding| {
                !binding.keys.is_empty() && binding.matches_physical(mods, &self.pressed_keys)
            })
            .map(|binding| (binding.client.clone(), binding.id.clone()))
            .collect();
        self.active_bindings
            .retain(|binding| matching.contains(binding));
        if !self.last_key_released {
            activated.extend(
                matching
                    .into_iter()
                    .filter(|binding| self.active_bindings.insert(binding.clone()))
                    .map(|(client, id)| ActivatedShortcut { client, id }),
            );
        }
        activated
    }
}

fn modifier_count(mods: PhysicalMods) -> u8 {
    u8::from(mods.ctrl) + u8::from(mods.alt) + u8::from(mods.shift) + u8::from(mods.logo)
}

pub fn validate_accelerator(accelerator: &str) -> Result<(), String> {
    parse_binding("validation", "validation", accelerator).map(|_| ())
}

impl ShortcutBinding {
    fn matches_physical(&self, mods: PhysicalMods, pressed_keys: &HashSet<u32>) -> bool {
        self.mods == mods
            && self
                .keys
                .iter()
                .all(|key| pressed_keys.contains(&evdev_to_smithay(*key)))
    }
}

fn parse_binding(client: &str, id: &str, accelerator: &str) -> Result<ShortcutBinding, String> {
    let accelerator = ShortcutAccelerator::parse(accelerator)?;
    Ok(ShortcutBinding {
        client: client.into(),
        id: id.into(),
        mods: accelerator.modifiers,
        keys: accelerator.keys,
    })
}

pub fn update_physical_mods(mods: &mut PhysicalMods, keycode: u32, pressed: bool) {
    match keycode {
        x if x == evdev_to_smithay(29) || x == evdev_to_smithay(97) => mods.ctrl = pressed,
        x if x == evdev_to_smithay(56) || x == evdev_to_smithay(100) => mods.alt = pressed,
        x if x == evdev_to_smithay(42) || x == evdev_to_smithay(54) => mods.shift = pressed,
        x if x == evdev_to_smithay(125) || x == evdev_to_smithay(126) => mods.logo = pressed,
        _ => {}
    }
}

pub fn physical_vt_from_keycode(keycode: u32) -> Option<i32> {
    match keycode {
        x if x == evdev_to_smithay(59) => Some(1),
        x if x == evdev_to_smithay(60) => Some(2),
        x if x == evdev_to_smithay(61) => Some(3),
        x if x == evdev_to_smithay(62) => Some(4),
        x if x == evdev_to_smithay(63) => Some(5),
        x if x == evdev_to_smithay(64) => Some(6),
        x if x == evdev_to_smithay(65) => Some(7),
        x if x == evdev_to_smithay(66) => Some(8),
        x if x == evdev_to_smithay(67) => Some(9),
        x if x == evdev_to_smithay(68) => Some(10),
        x if x == evdev_to_smithay(87) => Some(11),
        x if x == evdev_to_smithay(88) => Some(12),
        _ => None,
    }
}

fn evdev_to_smithay(code: u32) -> u32 {
    code + 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activates_a_shortcut_for_its_client() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind(":1.4", "launcher", "Super+Space"));
        assert_eq!(registry.bindings.len(), 1);
        registry.update_key(evdev_to_smithay(57), true);
        assert_eq!(
            registry.maybe_activate_physical(PhysicalMods {
                logo: true,
                ..Default::default()
            }),
            vec![ActivatedShortcut {
                client: ":1.4".to_string(),
                id: "launcher".to_string(),
            }]
        );
    }

    #[test]
    fn rebinding_is_scoped_to_the_client() {
        let mut registry = ShortcutRegistry::default();
        registry.bind(":1.4", "toggle", "Ctrl+Alt+T");
        registry.bind(":1.5", "toggle", "Super+T");
        registry.bind(":1.4", "toggle", "Super+T");
        assert_eq!(registry.bindings.len(), 2);
    }

    #[test]
    fn unregistering_a_client_removes_all_its_shortcuts() {
        let mut registry = ShortcutRegistry::default();
        registry.bind(":1.4", "quit", "Ctrl+Q");
        registry.bind(":1.4", "launcher", "Super+Space");
        registry.bind(":1.5", "launcher", "Super+Space");
        registry.unregister_client(":1.4");
        assert_eq!(registry.bindings.len(), 1);
    }

    #[test]
    fn rejects_accelerators_without_modifiers() {
        let mut registry = ShortcutRegistry::default();
        assert!(!registry.bind(":1.4", "broken", "T"));
        assert!(!registry.bind(":1.4", "empty", ""));
        assert!(!registry.bind(":1.4", "chord", "XF86AudioRaiseVolume+T"));
        assert_eq!(registry.bindings.len(), 0);
    }

    #[test]
    fn media_keys_activate_without_modifiers_and_rearm_on_release() {
        for (accelerator, code) in [
            ("XF86AudioRaiseVolume", 115),
            ("XF86AudioLowerVolume", 114),
            ("XF86MonBrightnessUp", 225),
            ("XF86MonBrightnessDown", 224),
        ] {
            let mut registry = ShortcutRegistry::default();
            assert!(registry.bind(":1.4", "media", accelerator));
            registry.update_key(evdev_to_smithay(code), true);
            let expected = vec![ActivatedShortcut {
                client: ":1.4".to_string(),
                id: "media".to_string(),
            }];
            assert_eq!(
                registry.maybe_activate_physical(PhysicalMods::default()),
                expected
            );
            assert!(registry
                .maybe_activate_physical(PhysicalMods::default())
                .is_empty());
            registry.update_key(evdev_to_smithay(code), false);
            assert!(registry
                .maybe_activate_physical(PhysicalMods::default())
                .is_empty());
            registry.update_key(evdev_to_smithay(code), true);
            assert_eq!(
                registry.maybe_activate_physical(PhysicalMods::default()),
                expected
            );
        }
    }

    #[test]
    fn media_shortcuts_require_their_configured_modifiers() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind(":1.4", "media", "Ctrl+xf86audioraisevolume"));
        registry.update_key(evdev_to_smithay(115), true);
        assert!(registry
            .maybe_activate_physical(PhysicalMods::default())
            .is_empty());
        assert_eq!(
            registry
                .maybe_activate_physical(PhysicalMods {
                    ctrl: true,
                    ..Default::default()
                })
                .len(),
            1
        );
    }

    #[test]
    fn supports_modifier_only_and_three_key_shortcuts() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind(":1.4", "overview", "Super"));
        assert!(registry.bind(":1.4", "chord", "Ctrl+Alt+Q+W+E"));
        assert!(registry
            .maybe_activate_physical(PhysicalMods {
                logo: true,
                ..Default::default()
            })
            .is_empty());
        assert_eq!(
            registry
                .maybe_activate_physical(PhysicalMods::default())
                .len(),
            1
        );
    }

    #[test]
    fn modifier_only_shortcuts_do_not_fire_when_used_in_a_chord() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind("shell", "launcher", "Super"));
        let super_mods = PhysicalMods {
            logo: true,
            ..Default::default()
        };
        assert!(registry.maybe_activate_physical(super_mods).is_empty());
        registry.update_key(evdev_to_smithay(32), true);
        assert!(registry.maybe_activate_physical(super_mods).is_empty());
        registry.update_key(evdev_to_smithay(32), false);
        assert!(registry.maybe_activate_physical(super_mods).is_empty());
        assert!(registry
            .maybe_activate_physical(PhysicalMods::default())
            .is_empty());
    }

    #[test]
    fn tab_shortcuts_activate_and_rearm_after_release() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind("shell", "next", "Alt+Tab"));
        assert!(registry.bind("shell", "previous", "Alt+Shift+Tab"));
        let mut mods = PhysicalMods {
            alt: true,
            ..Default::default()
        };
        registry.update_key(evdev_to_smithay(15), true);
        assert_eq!(registry.maybe_activate_physical(mods)[0].id, "next");
        assert!(registry.maybe_activate_physical(mods).is_empty());
        registry.update_key(evdev_to_smithay(15), false);
        assert!(registry.maybe_activate_physical(mods).is_empty());
        mods.shift = true;
        registry.update_key(evdev_to_smithay(15), true);
        assert_eq!(registry.maybe_activate_physical(mods)[0].id, "previous");
    }

    #[test]
    fn supports_function_keys_combined_with_a_modifier() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind(":1.4", "screenshot", "Super+F3"));
        registry.update_key(evdev_to_smithay(61), true);
        assert_eq!(
            registry.maybe_activate_physical(PhysicalMods {
                logo: true,
                ..Default::default()
            }),
            vec![ActivatedShortcut {
                client: ":1.4".to_string(),
                id: "screenshot".to_string(),
            }]
        );
    }

    #[test]
    fn function_key_shortcuts_do_not_activate_with_the_wrong_modifier() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind(":1.4", "close", "Alt+F4"));
        registry.update_key(evdev_to_smithay(62), true);
        assert!(registry
            .maybe_activate_physical(PhysicalMods {
                logo: true,
                ..Default::default()
            })
            .is_empty());
        assert_eq!(
            registry.maybe_activate_physical(PhysicalMods {
                alt: true,
                ..Default::default()
            }),
            vec![ActivatedShortcut {
                client: ":1.4".to_string(),
                id: "close".to_string(),
            }]
        );
    }

    #[test]
    fn function_keys_up_to_f12_are_recognized() {
        assert!(validate_accelerator("Ctrl+F11").is_ok());
        assert!(validate_accelerator("Ctrl+F12").is_ok());
        assert!(validate_accelerator("Ctrl+F13").is_err());
    }

    #[test]
    fn supports_number_row_keys() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind("config", "workspace-1", "Super+1"));
        registry.update_key(evdev_to_smithay(2), true);
        assert_eq!(
            registry.maybe_activate_physical(PhysicalMods {
                logo: true,
                ..Default::default()
            }),
            vec![ActivatedShortcut {
                client: "config".to_string(),
                id: "workspace-1".to_string(),
            }]
        );
    }
}
