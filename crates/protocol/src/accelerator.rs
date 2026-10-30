#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShortcutModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutAccelerator {
    pub modifiers: ShortcutModifiers,
    /// Linux evdev codes, without the XKB offset.
    pub keys: Vec<u32>,
}

impl ShortcutAccelerator {
    pub fn parse(accelerator: &str) -> Result<Self, String> {
        let mut modifiers = ShortcutModifiers::default();
        let mut keys = Vec::new();
        for token in accelerator.replace(',', "+").split('+') {
            let token = token.trim().to_ascii_lowercase();
            match token.as_str() {
                "" => continue,
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" | "mod1" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "super" | "logo" | "meta" | "mod4" | "win" => modifiers.logo = true,
                key => {
                    let code =
                        shortcut_keycode(key).ok_or_else(|| format!("unsupported key `{key}`"))?;
                    if keys.len() == 3 {
                        return Err("shortcut has more than three non-modifier keys".into());
                    }
                    if keys.contains(&code) {
                        return Err("shortcut repeats a non-modifier key".into());
                    }
                    keys.push(code);
                }
            }
        }
        if modifiers == ShortcutModifiers::default()
            && !(keys.len() == 1 && is_media_keycode(keys[0]))
        {
            return Err("shortcut has no modifier key".into());
        }
        Ok(Self { modifiers, keys })
    }
}

pub fn is_modifier_keycode(code: u32) -> bool {
    matches!(code, 29 | 97 | 42 | 54 | 56 | 100 | 125 | 126)
}

fn is_media_keycode(code: u32) -> bool {
    // 99 is Print Screen: like media keys it is safe to bind without a
    // modifier because no application uses it as regular text input.
    matches!(code, 99 | 113..=115 | 163..=166 | 224 | 225 | 248)
}

pub fn shortcut_keycode(name: &str) -> Option<u32> {
    let normalized = name.to_ascii_lowercase();
    let name = normalized.as_str();
    if let Some(number) = name
        .strip_prefix('f')
        .and_then(|number| number.parse::<u32>().ok())
    {
        return match number {
            1..=10 => Some(58 + number),
            11 => Some(87),
            12 => Some(88),
            _ => None,
        };
    }
    Some(match name {
        "1" => 2,
        "2" => 3,
        "3" => 4,
        "4" => 5,
        "5" => 6,
        "6" => 7,
        "7" => 8,
        "8" => 9,
        "9" => 10,
        "0" => 11,
        "minus" | "-" => 12,
        "equal" | "=" | "plus" => 13,
        "backspace" => 14,
        "tab" => 15,
        "q" => 16,
        "w" => 17,
        "e" => 18,
        "r" => 19,
        "t" => 20,
        "y" => 21,
        "u" => 22,
        "i" => 23,
        "o" => 24,
        "p" => 25,
        "bracketleft" | "[" => 26,
        "bracketright" | "]" => 27,
        "return" | "enter" => 28,
        "a" => 30,
        "s" => 31,
        "d" => 32,
        "f" => 33,
        "g" => 34,
        "h" => 35,
        "j" => 36,
        "k" => 37,
        "l" => 38,
        "semicolon" | ";" => 39,
        "apostrophe" | "'" => 40,
        "grave" | "`" => 41,
        "backslash" | "\\" => 43,
        "z" => 44,
        "x" => 45,
        "c" => 46,
        "v" => 47,
        "b" => 48,
        "n" => 49,
        "m" => 50,
        "comma" => 51,
        "period" | "." => 52,
        "slash" | "/" => 53,
        "space" => 57,
        "esc" | "escape" => 1,
        "home" => 102,
        "up" => 103,
        "pageup" | "prior" => 104,
        "left" => 105,
        "right" => 106,
        "end" => 107,
        "down" => 108,
        "pagedown" | "next" => 109,
        "insert" => 110,
        "delete" | "del" => 111,
        "print" | "printscreen" => 99,
        "xf86audiomute" => 113,
        "xf86audiolowervolume" => 114,
        "xf86audioraisevolume" => 115,
        "xf86audionext" => 163,
        "xf86audioplay" | "xf86audiopause" => 164,
        "xf86audioprev" => 165,
        "xf86audiostop" => 166,
        "xf86monbrightnessdown" => 224,
        "xf86monbrightnessup" => 225,
        "xf86audiomicmute" => 248,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_defaults_and_navigation_keys_have_physical_mappings() {
        for accelerator in [
            "Super",
            "Super+D",
            "Alt+Tab",
            "Alt+Shift+Tab",
            "Alt+F4",
            "XF86AudioRaiseVolume",
            "XF86AudioLowerVolume",
            "XF86MonBrightnessUp",
            "XF86MonBrightnessDown",
            "Super+Left",
            "Ctrl+PageDown",
            "Ctrl+Comma",
        ] {
            assert!(
                ShortcutAccelerator::parse(accelerator).is_ok(),
                "{accelerator}"
            );
        }
        assert_eq!(
            ShortcutAccelerator::parse("Alt+Tab").unwrap().keys,
            vec![15]
        );
    }

    #[test]
    fn unsupported_and_ambiguous_keys_are_rejected_before_registration() {
        for accelerator in [
            "Ctrl+ñ",
            "Ctrl+é",
            "Ctrl+F13",
            "Alt+Tab+tab",
            "",
            "K",
            "Ctrl+Q+W+E+R",
        ] {
            assert!(
                ShortcutAccelerator::parse(accelerator).is_err(),
                "{accelerator}"
            );
        }
    }
}
