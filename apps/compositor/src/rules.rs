use std::collections::HashMap;

use regex::Regex;

use crate::config::WindowRuleConfig;

const MAX_RULES_PER_CLIENT: usize = 256;

pub struct CompiledRule {
    config: WindowRuleConfig,
    regex: Option<Regex>,
}

impl CompiledRule {
    pub fn new(config: WindowRuleConfig) -> Result<Self, regex::Error> {
        let regex = config.regex.as_deref().map(Regex::new).transpose()?;
        Ok(Self { config, regex })
    }

    fn matches(&self, title: &str, app_id: Option<&str>) -> bool {
        let rule = &self.config;
        if rule
            .app_id
            .as_deref()
            .is_some_and(|expected| app_id != Some(expected))
            || rule
                .title
                .as_deref()
                .is_some_and(|expected| title != expected)
        {
            return false;
        }
        // Only XDG toplevels exist, so X11-specific selectors never match.
        if rule.class.is_some() || rule.role.is_some() || rule.window_type.is_some() {
            return false;
        }
        self.regex.as_ref().is_none_or(|regex| {
            regex.is_match(title) || app_id.is_some_and(|id| regex.is_match(id))
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppliedWindowRule {
    pub floating: Option<bool>,
    pub workspace: Option<String>,
    pub output: Option<String>,
    pub size: Option<[i32; 2]>,
    pub position: Option<[i32; 2]>,
    pub opacity: Option<f32>,
    pub always_on_top: Option<bool>,
    pub decoration: Option<bool>,
}

impl AppliedWindowRule {
    fn apply(&mut self, rule: &WindowRuleConfig) {
        if let Some(floating) = rule.floating.or(rule.tiled.map(|tiled| !tiled)) {
            self.floating = Some(floating);
        }
        if rule.workspace.is_some() {
            self.workspace.clone_from(&rule.workspace);
        }
        if rule.output.is_some() {
            self.output.clone_from(&rule.output);
        }
        self.size = rule.size.or(self.size);
        self.position = rule.position.or(self.position);
        self.opacity = rule.opacity.or(self.opacity);
        self.always_on_top = rule.always_on_top.or(self.always_on_top);
        self.decoration = rule.decoration.or(self.decoration);
    }
}

/// Configured rules followed by client-owned temporary rules.
#[derive(Default)]
pub struct RuleSet {
    configured: Vec<CompiledRule>,
    temporary: HashMap<String, Vec<(String, CompiledRule)>>,
}

impl RuleSet {
    pub fn set_configured(&mut self, rules: &[WindowRuleConfig]) {
        self.configured = rules
            .iter()
            .filter_map(|rule| match CompiledRule::new(rule.clone()) {
                Ok(rule) => Some(rule),
                Err(error) => {
                    tracing::warn!(%error, "skipping window rule with invalid regex");
                    None
                }
            })
            .collect();
    }

    pub fn register(&mut self, client: &str, id: &str, rule: WindowRuleConfig) -> bool {
        let Ok(rule) = CompiledRule::new(rule) else {
            return false;
        };
        let rules = self.temporary.entry(client.to_owned()).or_default();
        if let Some(existing) = rules.iter_mut().find(|(existing, _)| existing == id) {
            existing.1 = rule;
            return true;
        }
        if rules.len() >= MAX_RULES_PER_CLIENT {
            tracing::warn!(client, "temporary window rule limit reached");
            return false;
        }
        rules.push((id.to_owned(), rule));
        true
    }

    pub fn unregister(&mut self, client: &str, id: &str) {
        if let Some(rules) = self.temporary.get_mut(client) {
            rules.retain(|(existing, _)| existing != id);
            if rules.is_empty() {
                self.temporary.remove(client);
            }
        }
    }

    pub fn unregister_client(&mut self, client: &str) {
        self.temporary.remove(client);
    }

    pub fn resolve(&self, title: &str, app_id: Option<&str>) -> AppliedWindowRule {
        let mut applied = AppliedWindowRule::default();
        let temporary = self
            .temporary
            .values()
            .flat_map(|rules| rules.iter().map(|(_, rule)| rule));
        for rule in self.configured.iter().chain(temporary) {
            if rule.matches(title, app_id) {
                applied.apply(&rule.config);
            }
        }
        applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(toml: &str) -> WindowRuleConfig {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn later_rules_override_earlier_ones() {
        let mut rules = RuleSet::default();
        rules.set_configured(&[
            rule("app_id = \"foot\"\nfloating = true\nsize = [800, 600]\n"),
            rule("regex = \"^fo+t$\"\nfloating = false\nopacity = 0.9\n"),
        ]);
        let applied = rules.resolve("shell", Some("foot"));
        assert_eq!(applied.floating, Some(false));
        assert_eq!(applied.size, Some([800, 600]));
        assert_eq!(applied.opacity, Some(0.9));
        assert_eq!(
            rules.resolve("shell", Some("kitty")),
            AppliedWindowRule::default()
        );
    }

    #[test]
    fn temporary_rules_are_scoped_to_their_client() {
        let mut rules = RuleSet::default();
        assert!(rules.register(
            ":1.7",
            "pip",
            rule("title = \"PiP\"\nalways_on_top = true\n")
        ));
        assert_eq!(rules.resolve("PiP", None).always_on_top, Some(true));
        rules.unregister_client(":1.7");
        assert_eq!(rules.resolve("PiP", None).always_on_top, None);
    }

    #[test]
    fn x11_selectors_never_match_wayland_windows() {
        let mut rules = RuleSet::default();
        rules.set_configured(&[rule("class = \"Firefox\"\nfloating = true\n")]);
        assert_eq!(rules.resolve("Firefox", Some("Firefox")).floating, None);
    }

    #[test]
    fn temporary_rule_count_is_bounded() {
        let mut rules = RuleSet::default();
        for index in 0..MAX_RULES_PER_CLIENT {
            assert!(rules.register("c", &index.to_string(), rule("title = \"x\"\n")));
        }
        assert!(!rules.register("c", "overflow", rule("title = \"x\"\n")));
        assert!(rules.register("c", "0", rule("title = \"y\"\n")));
    }
}
