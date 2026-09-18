use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct ShortcutRegistry {
    bindings: Vec<ShortcutBinding>,
    pressed_keys: HashSet<u32>,
    active_bindings: HashSet<(String, String)>,
}

#[derive(Debug, Clone)]
struct ShortcutBinding {
    client: String,
    id: String,
    mods: ShortcutMods,
    keys: Vec<ShortcutKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivatedShortcut {
    pub client: String,
    pub id: String,
}

pub type PhysicalMods = ShortcutMods;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShortcutMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShortcutKey {
    Char(char),
    Return,
    Space,
    Escape,
    F(u8),
}

const MAX_BINDINGS_PER_CLIENT: usize = 256;

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
    }

    pub fn update_key(&mut self, keycode: u32, pressed: bool) {
        if pressed {
            self.pressed_keys.insert(keycode);
        } else {
            self.pressed_keys.remove(&keycode);
        }
    }

    pub fn maybe_activate_physical(&mut self, mods: PhysicalMods) -> Vec<ActivatedShortcut> {
        let matching: HashSet<_> = self
            .bindings
            .iter()
            .filter(|binding| binding.matches_physical(mods, &self.pressed_keys))
            .map(|binding| (binding.client.clone(), binding.id.clone()))
            .collect();
        self.active_bindings
            .retain(|binding| matching.contains(binding));
        matching
            .into_iter()
            .filter(|binding| self.active_bindings.insert(binding.clone()))
            .map(|(client, id)| ActivatedShortcut { client, id })
            .collect()
    }
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
                .all(|key| key.matches_physical(pressed_keys))
    }
}

impl ShortcutKey {
    fn matches_physical(&self, pressed_keys: &HashSet<u32>) -> bool {
        match self {
            ShortcutKey::Char(' ') | ShortcutKey::Space => {
                pressed_keys.contains(&evdev_to_smithay(57))
            }
            ShortcutKey::Char(ch) => {
                physical_letter_keycode(*ch).is_some_and(|keycode| pressed_keys.contains(&keycode))
            }
            ShortcutKey::Return => pressed_keys.contains(&evdev_to_smithay(28)),
            ShortcutKey::Escape => pressed_keys.contains(&evdev_to_smithay(1)),
            ShortcutKey::F(n) => {
                physical_f_keycode(*n).is_some_and(|keycode| pressed_keys.contains(&keycode))
            }
        }
    }
}

fn parse_binding(client: &str, id: &str, accelerator: &str) -> Result<ShortcutBinding, String> {
    let mut mods = ShortcutMods::default();
    let mut keys = Vec::new();
    let aliases = key_aliases();

    for token in accelerator.replace(',', "+").split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" | "mod1" => mods.alt = true,
            "shift" => mods.shift = true,
            "super" | "logo" | "meta" | "mod4" | "win" => mods.logo = true,
            value => {
                if keys.len() == 3 {
                    return Err("shortcut has more than three non-modifier keys".to_string());
                }
                let key = parse_key(value, &aliases)?;
                if keys.contains(&key) {
                    return Err("shortcut repeats a non-modifier key".to_string());
                }
                keys.push(key);
            }
        }
    }

    if !mods.ctrl && !mods.alt && !mods.shift && !mods.logo {
        return Err("shortcut has no modifier key".to_string());
    }
    Ok(ShortcutBinding {
        client: client.to_string(),
        id: id.to_string(),
        mods,
        keys,
    })
}

fn parse_key(
    value: &str,
    aliases: &HashMap<&'static str, ShortcutKey>,
) -> Result<ShortcutKey, String> {
    if let Some(key) = aliases.get(value) {
        return Ok(key.clone());
    }
    if let Some(rest) = value.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u8>() {
            if (1..=12).contains(&n) {
                return Ok(ShortcutKey::F(n));
            }
        }
    }
    let mut chars = value.chars();
    if let (Some(ch), None) = (chars.next(), chars.next()) {
        return Ok(ShortcutKey::Char(ch));
    }
    Err(format!("unsupported key `{value}`"))
}

fn key_aliases() -> HashMap<&'static str, ShortcutKey> {
    HashMap::from([
        ("return", ShortcutKey::Return),
        ("enter", ShortcutKey::Return),
        ("space", ShortcutKey::Space),
        ("esc", ShortcutKey::Escape),
        ("escape", ShortcutKey::Escape),
    ])
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

fn physical_f_keycode(n: u8) -> Option<u32> {
    match n {
        1..=10 => Some(evdev_to_smithay(58 + u32::from(n))),
        11 => Some(evdev_to_smithay(87)),
        12 => Some(evdev_to_smithay(88)),
        _ => None,
    }
}

fn physical_letter_keycode(ch: char) -> Option<u32> {
    let evdev = match ch.to_ascii_lowercase() {
        '1' => 2,
        '2' => 3,
        '3' => 4,
        '4' => 5,
        '5' => 6,
        '6' => 7,
        '7' => 8,
        '8' => 9,
        '9' => 10,
        '0' => 11,
        'q' => 16,
        'w' => 17,
        'e' => 18,
        'r' => 19,
        't' => 20,
        'y' => 21,
        'u' => 22,
        'i' => 23,
        'o' => 24,
        'p' => 25,
        'a' => 30,
        's' => 31,
        'd' => 32,
        'f' => 33,
        'g' => 34,
        'h' => 35,
        'j' => 36,
        'k' => 37,
        'l' => 38,
        'z' => 44,
        'x' => 45,
        'c' => 46,
        'v' => 47,
        'b' => 48,
        'n' => 49,
        'm' => 50,
        _ => return None,
    };
    Some(evdev_to_smithay(evdev))
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
        assert_eq!(registry.bindings.len(), 0);
    }

    #[test]
    fn supports_modifier_only_and_three_key_shortcuts() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind(":1.4", "overview", "Super"));
        assert!(registry.bind(":1.4", "chord", "Ctrl+Alt+Q+W+E"));
        assert_eq!(
            registry
                .maybe_activate_physical(PhysicalMods {
                    logo: true,
                    ..Default::default()
                })
                .len(),
            1
        );
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
