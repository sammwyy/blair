use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use smithay::{
    delegate_xdg_system_bell,
    reexports::calloop::timer::{TimeoutAction, Timer},
    wayland::xdg_system_bell::XdgSystemBellHandler,
};
use wayland_server::protocol::wl_surface::WlSurface;

use crate::state::BlairState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccessibilityConfig {
    pub sticky_keys: bool,
    pub visual_alerts: bool,
    pub zoom: bool,
    pub zoom_factor: f64,
}

impl Default for AccessibilityConfig {
    fn default() -> Self {
        Self {
            sticky_keys: false,
            visual_alerts: false,
            zoom: false,
            zoom_factor: 2.0,
        }
    }
}

impl AccessibilityConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.zoom_factor.is_finite() || !(1.0..=8.0).contains(&self.zoom_factor) {
            anyhow::bail!("accessibility.zoom_factor must be between 1 and 8");
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Previous {
    Off,
    Latched,
    Locked,
}

enum Modifier {
    Held { previous: Previous, used: bool },
    Latched { consumed: bool },
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTransition {
    pub code: u32,
    pub pressed: bool,
}

#[derive(Default)]
pub struct StickyKeys {
    modifiers: BTreeMap<u32, Modifier>,
    keys: HashSet<u32>,
}

impl StickyKeys {
    pub fn transform(&mut self, code: u32, pressed: bool) -> Vec<KeyTransition> {
        let event = KeyTransition { code, pressed };
        if blair_protocol::is_modifier_keycode(code.saturating_sub(8)) {
            if pressed {
                let previous = match self.modifiers.remove(&code) {
                    None => Previous::Off,
                    Some(Modifier::Latched { .. }) => Previous::Latched,
                    Some(Modifier::Locked) => Previous::Locked,
                    Some(held @ Modifier::Held { .. }) => {
                        self.modifiers.insert(code, held);
                        return vec![];
                    }
                };
                self.modifiers.insert(
                    code,
                    Modifier::Held {
                        previous,
                        used: !self.keys.is_empty(),
                    },
                );
                return if matches!(previous, Previous::Off) {
                    vec![event]
                } else {
                    vec![]
                };
            }
            match self.modifiers.remove(&code) {
                Some(Modifier::Held {
                    previous,
                    used: false,
                }) => {
                    match previous {
                        Previous::Off => {
                            self.modifiers
                                .insert(code, Modifier::Latched { consumed: false });
                        }
                        Previous::Latched => {
                            self.modifiers.insert(code, Modifier::Locked);
                        }
                        Previous::Locked => return vec![event],
                    }
                    vec![]
                }
                Some(Modifier::Held {
                    previous: Previous::Locked,
                    used: true,
                }) => {
                    self.modifiers.insert(code, Modifier::Locked);
                    vec![]
                }
                _ => vec![event],
            }
        } else {
            let mut events = vec![event];
            if pressed {
                self.keys.insert(code);
                for modifier in self.modifiers.values_mut() {
                    match modifier {
                        Modifier::Held { used, .. } => *used = true,
                        Modifier::Latched { consumed } => *consumed = true,
                        Modifier::Locked => {}
                    }
                }
            } else {
                self.keys.remove(&code);
                if self.keys.is_empty() {
                    self.modifiers.retain(|code, modifier| {
                        if matches!(modifier, Modifier::Latched { consumed: true }) {
                            events.push(KeyTransition {
                                code: *code,
                                pressed: false,
                            });
                            false
                        } else {
                            true
                        }
                    });
                }
            }
            events
        }
    }

    pub fn release_virtual(&mut self) -> Vec<u32> {
        let releases = self
            .modifiers
            .iter()
            .filter(|(_, modifier)| !matches!(modifier, Modifier::Held { .. }))
            .map(|(code, _)| *code)
            .collect();
        self.modifiers.clear();
        self.keys.clear();
        releases
    }
}

impl XdgSystemBellHandler for BlairState {
    fn ring(&mut self, _surface: Option<WlSurface>) {
        if !self.config.accessibility.visual_alerts
            || self
                .visual_bell_until
                .is_some_and(|deadline| deadline > Instant::now())
        {
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(160);
        self.visual_bell_until = Some(deadline);
        self.request_redraw();
        if let Err(error) = self.loop_handle.insert_source(
            Timer::from_duration(Duration::from_millis(160)),
            move |_, _, state: &mut BlairState| {
                if state.visual_bell_until == Some(deadline) {
                    state.visual_bell_until = None;
                }
                state.request_redraw();
                TimeoutAction::Drop
            },
        ) {
            tracing::warn!(%error, "failed to schedule visual bell cleanup");
            self.visual_bell_until = None;
        }
    }
}

delegate_xdg_system_bell!(BlairState);

#[cfg(test)]
mod tests {
    use super::*;

    const SHIFT: u32 = 42 + 8;
    const CTRL: u32 = 29 + 8;
    const A: u32 = 30 + 8;

    #[test]
    fn a_tapped_modifier_is_latched_for_exactly_one_key() {
        let mut keys = StickyKeys::default();
        assert_eq!(
            keys.transform(SHIFT, true),
            vec![KeyTransition {
                code: SHIFT,
                pressed: true
            }]
        );
        assert!(keys.transform(SHIFT, false).is_empty());
        assert_eq!(
            keys.transform(A, true),
            vec![KeyTransition {
                code: A,
                pressed: true
            }]
        );
        assert_eq!(
            keys.transform(A, false),
            vec![
                KeyTransition {
                    code: A,
                    pressed: false
                },
                KeyTransition {
                    code: SHIFT,
                    pressed: false
                }
            ]
        );
        assert!(keys.release_virtual().is_empty());
    }

    #[test]
    fn a_double_tap_locks_and_a_third_tap_unlocks() {
        let mut keys = StickyKeys::default();
        keys.transform(SHIFT, true);
        keys.transform(SHIFT, false);
        assert!(keys.transform(SHIFT, true).is_empty());
        assert!(keys.transform(SHIFT, false).is_empty());
        keys.transform(A, true);
        assert_eq!(keys.transform(A, false).len(), 1);
        assert!(keys.transform(SHIFT, true).is_empty());
        assert_eq!(
            keys.transform(SHIFT, false),
            vec![KeyTransition {
                code: SHIFT,
                pressed: false
            }]
        );
    }

    #[test]
    fn held_chords_release_normally_and_multiple_latches_are_released_together() {
        let mut keys = StickyKeys::default();
        keys.transform(CTRL, true);
        keys.transform(A, true);
        assert_eq!(
            keys.transform(CTRL, false),
            vec![KeyTransition {
                code: CTRL,
                pressed: false
            }]
        );
        keys.transform(A, false);
        for code in [CTRL, SHIFT] {
            keys.transform(code, true);
            keys.transform(code, false);
        }
        keys.transform(A, true);
        assert_eq!(keys.transform(A, false).len(), 3);
    }
}
