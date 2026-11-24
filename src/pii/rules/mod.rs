//! PII detection rules
//!
//! Each rule detects a specific type of PII with precision-tuned patterns.
//! The order in which the rules are built is irrelevant: the engine sorts
//! every match by `(blocked, severity)` before overlap deduplication, so the
//! higher-severity match wins the overlap no matter where its rule sits in
//! the list.

mod api_key;
mod bank_card;
mod custom;
mod email;
mod id_card;
mod ip_address;
mod phone;
mod ssh_key;

use crate::config::types::CustomPiiRule;
use crate::pii::engine::{PiiMatch, PiiSeverity};
use std::collections::HashSet;

/// Number of built-in rules prepended by [`build_rules`] before any custom
/// rules. Custom rules are appended after, so `rules.len() - BUILTIN_COUNT`
/// is the number of custom rules that actually compiled.
///
/// This constant lives here (next to `build_rules`) so the two stay coupled
/// by source; engine code that wants the count imports it rather than
/// duplicating the literal.
pub(crate) const BUILTIN_COUNT: usize = 7;

/// Names of the built-in rules, in the order [`build_rules`] builds them.
///
/// The engine resolves a rule's action from its *name*
/// (`engine.rs::action_for_rule`), so these names are the built-in rule
/// identity: a built-in rule whose name is missing from this list would
/// silently resolve to a different action. The list is asserted against the
/// built-in rules at build time.
pub(crate) const BUILTIN_RULE_NAMES: [&str; BUILTIN_COUNT] = [
    "api_key",
    "ssh_key",
    "id_card",
    "phone",
    "bank_card",
    "email",
    "ip_address",
];

/// Trait for PII detection rules
pub trait PiiRule: Send + Sync {
    /// Rule identifier (matches config field name)
    fn name(&self) -> &str;

    /// Severity level of this PII type
    fn severity(&self) -> PiiSeverity;

    /// Placeholder text for replacement
    fn placeholder(&self) -> &str;

    /// Detect PII in text, returning all matches
    fn detect(&self, text: &str) -> Vec<PiiMatch>;
}

/// Build all rules with built-ins first, then custom rules.
///
/// Built-in rules are always included. Custom rules from config are
/// appended after built-ins. Invalid custom regex patterns are logged
/// and skipped.
///
/// All rules (built-in + custom) are returned in build order, and that order
/// does not matter: `filter_with_config` sorts every match by
/// `(blocked, severity)` before overlap deduplication, so the
/// higher-severity match wins the overlap regardless of the rule order.
pub(crate) fn build_rules(custom_configs: &[CustomPiiRule]) -> Vec<Box<dyn PiiRule>> {
    let mut rules: Vec<Box<dyn PiiRule>> = vec![
        Box::new(api_key::ApiKeyRule::new()),
        Box::new(ssh_key::SshKeyRule::new()),
        Box::new(id_card::IdCardRule::new()),
        Box::new(phone::PhoneRule::new()),
        Box::new(bank_card::BankCardRule::new()),
        Box::new(email::EmailRule::new()),
        Box::new(ip_address::IpAddressRule::new()),
    ];
    debug_assert_eq!(
        rules.len(),
        BUILTIN_COUNT,
        "BUILTIN_COUNT drifted from the literal rule list"
    );
    // The built-in names are what the engine resolves a rule's action from,
    // so the name list and the built-in rules must agree, and no name may be
    // used twice (the engine's action lookup is a name match).
    for (index, rule) in rules.iter().enumerate() {
        debug_assert!(
            BUILTIN_RULE_NAMES.contains(&rule.name()),
            "built-in rule `{}` is missing from BUILTIN_RULE_NAMES",
            rule.name()
        );
        debug_assert!(
            !rules[..index]
                .iter()
                .any(|other| other.name() == rule.name()),
            "built-in rule name `{}` is used more than once",
            rule.name()
        );
    }

    // A custom rule must own its name outright. The engine resolves a rule's
    // action from the name and looks the built-in names up FIRST, so a
    // custom rule named `email` would silently run under the built-in email
    // action — and one named `ip_address` would run under a category that
    // defaults to `Off`, i.e. it would never run at all. Two custom rules
    // with one name collapse into a single entry in the engine's action map,
    // so the earlier one would inherit the later one's action. Both are
    // rejected here, at load time, instead of being papered over in the
    // engine's lookup.
    let mut custom_names: HashSet<&str> = HashSet::new();

    for config in custom_configs {
        if BUILTIN_RULE_NAMES.contains(&config.name.as_str()) {
            tracing::warn!(
                rule_name = %config.name,
                "Skipping custom PII rule: name collides with a built-in rule"
            );
            continue;
        }
        // The name is reserved by the config entry, not by the compiled
        // rule: the engine builds its name → action map from the config, so
        // a name is taken even if its pattern never compiles.
        if !custom_names.insert(config.name.as_str()) {
            tracing::warn!(
                rule_name = %config.name,
                "Skipping custom PII rule: duplicate rule name"
            );
            continue;
        }
        match custom::CustomRegexRule::new(config.clone()) {
            Ok(rule) => rules.push(Box::new(rule)),
            Err(e) => {
                tracing::warn!(
                    rule_name = %config.name,
                    pattern = %config.pattern,
                    error = %e,
                    "Skipping invalid custom PII rule regex"
                );
            }
        }
    }

    // Match-level priority (Block > severity) is decided inside
    // `filter_with_config` — engine.rs sorts `all_matches` by
    // `(Reverse(blocked), Reverse(severity))` before overlap dedup, so the
    // rule insertion order here is irrelevant to the outcome.

    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{CustomPiiSeverity, PiiAction};

    fn custom_rule(name: &str, pattern: &str) -> CustomPiiRule {
        CustomPiiRule {
            name: name.to_string(),
            pattern: pattern.to_string(),
            placeholder: "[REDACTED]".to_string(),
            severity: CustomPiiSeverity::High,
            action: PiiAction::Block,
        }
    }

    /// The engine resolves a rule's action from its name, so the name list
    /// and the built-in rules must not drift apart.
    #[test]
    fn builtin_names_match_the_builtin_rules() {
        let rules = build_rules(&[]);
        assert_eq!(rules.len(), BUILTIN_COUNT);
        for rule in &rules {
            assert!(
                BUILTIN_RULE_NAMES.contains(&rule.name()),
                "built-in rule `{}` is missing from BUILTIN_RULE_NAMES",
                rule.name()
            );
        }
    }

    /// A custom rule named after a built-in would run under the built-in
    /// action (the engine looks built-in names up first) — or, for
    /// `ip_address`, under a category that defaults to `Off`, i.e. it would
    /// never run at all.
    #[test]
    fn a_custom_rule_may_not_reuse_a_builtin_name() {
        let rules = build_rules(&[custom_rule("email", r"secret-\d+")]);
        assert_eq!(
            rules.len(),
            BUILTIN_COUNT,
            "the colliding custom rule must be rejected"
        );
        assert_eq!(
            rules.iter().filter(|rule| rule.name() == "email").count(),
            1,
            "only the built-in email rule may remain"
        );
    }

    /// Two custom rules with one name collapse into a single entry in the
    /// engine's name → action map, so the earlier one would silently inherit
    /// the later one's action.
    #[test]
    fn a_duplicate_custom_rule_name_is_rejected() {
        let rules = build_rules(&[
            custom_rule("my_rule", r"first-\d+"),
            custom_rule("my_rule", r"second-\d+"),
        ]);
        assert_eq!(
            rules.len(),
            BUILTIN_COUNT + 1,
            "only one of the two same-named custom rules may load"
        );
        assert_eq!(
            rules.iter().filter(|rule| rule.name() == "my_rule").count(),
            1,
            "the duplicate custom rule must be rejected"
        );
    }
}
