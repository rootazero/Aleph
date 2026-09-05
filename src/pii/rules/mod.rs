//! PII detection rules
//!
//! Each rule detects a specific type of PII with precision-tuned patterns.
//! Rules are sorted by severity descending (via [`build_rules`]) so that
//! higher-severity matches win in overlap deduplication.

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

/// Number of built-in rules prepended by [`build_rules`] before any custom
/// rules. Custom rules are appended after, so `rules.len() - BUILTIN_COUNT`
/// is the number of custom rules that actually compiled.
///
/// This constant lives here (next to `build_rules`) so the two stay coupled
/// by source; engine code that wants the count imports it rather than
/// duplicating the literal.
pub(crate) const BUILTIN_COUNT: usize = 7;

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
/// All rules (built-in + custom) are sorted by severity in descending
/// order so that higher-severity matches win during overlap
/// deduplication in `dedup_overlapping`.
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

    for config in custom_configs {
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

    // Sort by severity descending: Critical rules are processed first so they
    // win when overlapping matches are deduplicated.
    rules.sort_by_key(|r| std::cmp::Reverse(r.severity()));

    rules
}
