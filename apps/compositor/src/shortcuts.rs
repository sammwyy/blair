use std::collections::HashMap;

use smithay::input::keyboard::{keysyms, Keysym, ModifiersState};

#[derive(Debug, Default)]
pub struct ShortcutRegistry {
    bindings: Vec<ShortcutBinding>,
}

#[derive(Debug, Clone)]
struct ShortcutBinding {
    id: String,
    mods: ShortcutMods,
    key: ShortcutKey,
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

impl ShortcutRegistry {
    pub fn bind(&mut self, id: &str, accelerator: &str) -> bool {
        let binding = match parse_binding(id, accelerator) {
            Ok(binding) => binding,
            Err(err) => {
                tracing::warn!(id, accelerator, error = %err, "ignoring invalid shortcut binding");
                return false;
            }
        };
        self.bindings.retain(|existing| existing.id != id);
        self.bindings.push(binding);
        tracing::info!(id, accelerator, "shortcut bound");
        true
    }

    pub fn unbind(&mut self, id: &str) {
        self.bindings.retain(|existing| existing.id != id);
    }

    pub fn binding_count(&self) -> usize {
        self.bindings.len()
    }

    pub fn maybe_activate(&self, mods: &ModifiersState, raw_sym: Keysym) -> Option<&str> {
        self.bindings
            .iter()
            .find(|binding| binding.matches(mods, raw_sym))
            .map(|binding| binding.id.as_str())
    }

    pub fn maybe_activate_physical(&self, mods: PhysicalMods, keycode: u32) -> Option<&str> {
        self.bindings
            .iter()
            .find(|binding| binding.matches_physical(mods, keycode))
            .map(|binding| binding.id.as_str())
    }
}

impl ShortcutBinding {
    fn matches(&self, mods: &ModifiersState, raw_sym: Keysym) -> bool {
        self.mods.ctrl == mods.ctrl
            && self.mods.alt == mods.alt
            && self.mods.shift == mods.shift
            && self.mods.logo == mods.logo
            && self.key.matches(raw_sym)
    }

    fn matches_physical(&self, mods: PhysicalMods, keycode: u32) -> bool {
        self.mods == mods && self.key.matches_physical(keycode)
    }
}

impl ShortcutKey {
    fn matches(&self, raw_sym: Keysym) -> bool {
        match self {
            ShortcutKey::Char(ch) => raw_sym
                .key_char()
                .map(|raw| raw.eq_ignore_ascii_case(ch))
                .unwrap_or(false),
            ShortcutKey::Return => u32::from(raw_sym) == keysyms::KEY_Return,
            ShortcutKey::Space => u32::from(raw_sym) == keysyms::KEY_space,
            ShortcutKey::Escape => u32::from(raw_sym) == keysyms::KEY_Escape,
            ShortcutKey::F(n) => f_key_number(raw_sym) == Some(*n),
        }
    }

    fn matches_physical(&self, keycode: u32) -> bool {
        match self {
            ShortcutKey::Char(' ') | ShortcutKey::Space => keycode == evdev_to_smithay(57),
            ShortcutKey::Char(ch) => physical_letter_keycode(*ch) == Some(keycode),
            ShortcutKey::Return => keycode == evdev_to_smithay(28),
            ShortcutKey::Escape => keycode == evdev_to_smithay(1),
            ShortcutKey::F(n) => physical_f_keycode(*n) == Some(keycode),
        }
    }
}

fn parse_binding(id: &str, accelerator: &str) -> Result<ShortcutBinding, String> {
    let mut mods = ShortcutMods::default();
    let mut key = None;
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
                if key.is_some() {
                    return Err("shortcut has more than one non-modifier key".to_string());
                }
                key = Some(parse_key(value, &aliases)?);
            }
        }
    }

    let key = key.ok_or_else(|| "shortcut has no non-modifier key".to_string())?;
    Ok(ShortcutBinding {
        id: id.to_string(),
        mods,
        key,
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

fn f_key_number(sym: Keysym) -> Option<u8> {
    match u32::from(sym) {
        keysyms::KEY_F1 => Some(1),
        keysyms::KEY_F2 => Some(2),
        keysyms::KEY_F3 => Some(3),
        keysyms::KEY_F4 => Some(4),
        keysyms::KEY_F5 => Some(5),
        keysyms::KEY_F6 => Some(6),
        keysyms::KEY_F7 => Some(7),
        keysyms::KEY_F8 => Some(8),
        keysyms::KEY_F9 => Some(9),
        keysyms::KEY_F10 => Some(10),
        keysyms::KEY_F11 => Some(11),
        keysyms::KEY_F12 => Some(12),
        _ => None,
    }
}

pub fn vt_from_keysym(sym: Keysym) -> Option<i32> {
    f_key_number(sym).map(i32::from)
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

    fn mods(ctrl: bool, alt: bool, shift: bool, logo: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            alt,
            shift,
            logo,
            ..Default::default()
        }
    }

    #[test]
    fn binds_and_activates_a_char_shortcut() {
        let mut registry = ShortcutRegistry::default();
        assert!(registry.bind("launcher", "Super+Space"));
        assert_eq!(registry.binding_count(), 1);

        let activated = registry.maybe_activate(
            &mods(false, false, false, true),
            Keysym::from(keysyms::KEY_space),
        );
        assert_eq!(activated, Some("launcher"));

        let not_activated = registry.maybe_activate(
            &mods(false, false, false, false),
            Keysym::from(keysyms::KEY_space),
        );
        assert_eq!(not_activated, None);
    }

    #[test]
    fn rebinding_the_same_id_replaces_the_old_binding() {
        let mut registry = ShortcutRegistry::default();
        registry.bind("toggle", "Ctrl+Alt+T");
        registry.bind("toggle", "Super+T");
        assert_eq!(registry.binding_count(), 1);

        let activated = registry.maybe_activate(
            &mods(false, false, false, true),
            Keysym::from(keysyms::KEY_T),
        );
        assert_eq!(activated, Some("toggle"));
    }

    #[test]
    fn unbind_removes_the_shortcut() {
        let mut registry = ShortcutRegistry::default();
        registry.bind("quit", "Ctrl+Q");
        registry.unbind("quit");
        assert_eq!(registry.binding_count(), 0);
    }

    #[test]
    fn rejects_accelerators_without_a_key() {
        let mut registry = ShortcutRegistry::default();
        assert!(!registry.bind("broken", "Ctrl+Shift"));
        assert_eq!(registry.binding_count(), 0);
    }

    #[test]
    fn physical_shortcut_matches_by_keycode_and_mods() {
        let mut registry = ShortcutRegistry::default();
        registry.bind("launcher", "Super+Space");
        let keycode = evdev_to_smithay(57);
        let physical_mods = PhysicalMods {
            logo: true,
            ..Default::default()
        };
        assert_eq!(
            registry.maybe_activate_physical(physical_mods, keycode),
            Some("launcher")
        );
    }
}
