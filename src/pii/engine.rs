//! Core PII detection and replacement engine

use crate::config::PiiAction;
use crate::config::PrivacyConfig;
use crate::pii::allowlist::PiiAllowlist;
use crate::pii::rules::PiiRule;
use crate::sync_primitives::{Arc, RwLock};
use std::sync::OnceLock;
use tracing::warn;

/// Severity level for PII detections
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum PiiSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for PiiSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// A single PII detection result
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PiiMatch {
    pub rule_name: String,
    pub start: usize,
    pub end: usize,
    pub matched_text: String,
    pub severity: PiiSeverity,
    pub placeholder: String,
}

/// Result of PII filtering
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FilterResult {
    /// The filtered text (with PII replaced by placeholders if blocked)
    pub text: String,
    /// Number of PII matches that were blocked (replaced)
    pub blocked_count: usize,
    /// Number of PII matches that were warned (not replaced)
    pub warned_count: usize,
}

impl FilterResult {
    #[must_use]
    pub fn unchanged(text: &str) -> Self {
        Self {
            text: text.to_string(),
            blocked_count: 0,
            warned_count: 0,
        }
    }

    /// True if any PII was detected (blocked or warned)
    #[must_use]
    pub const fn has_detections(&self) -> bool {
        self.blocked_count > 0 || self.warned_count > 0
    }
}

/// Global PII engine singleton
static PII_ENGINE: OnceLock<Arc<RwLock<PiiEngine>>> = OnceLock::new();

/// Main PII filtering engine
pub struct PiiEngine {
    rules: Vec<Box<dyn PiiRule>>,
    allowlist: PiiAllowlist,
    config: PrivacyConfig,
}

impl PiiEngine {
    /// Create a new PII engine with the given configuration
    #[must_use]
    pub fn new(config: PrivacyConfig) -> Self {
        let rules = crate::pii::rules::build_rules(&config.custom_rules);
        let allowlist = PiiAllowlist::default();
        Self {
            rules,
            allowlist,
            config,
        }
    }

    /// Initialize the global PII engine
    pub fn init(config: PrivacyConfig) {
        let engine = Arc::new(RwLock::new(Self::new(config)));
        if PII_ENGINE.set(engine).is_err() {
            warn!("PiiEngine already initialized, ignoring duplicate init call");
        }
    }

    /// Get the global PII engine (returns None if not initialized)
    pub fn global() -> Option<Arc<RwLock<Self>>> {
        PII_ENGINE.get().cloned()
    }

    /// Reload configuration (hot-reload support)
    pub fn reload(config: PrivacyConfig) {
        if let Some(engine) = PII_ENGINE.get() {
            // Build rules outside the lock to avoid blocking readers.
            let new_rules = crate::pii::rules::build_rules(&config.custom_rules);
            let mut guard = engine.write().unwrap_or_else(|e| e.into_inner());
            guard.rules = new_rules;
            guard.config = config;
        } else {
            warn!("PiiEngine::reload called but engine not initialized, ignoring");
        }
    }

    /// Check if a specific provider should be excluded from filtering
    #[must_use]
    pub fn is_provider_excluded(&self, provider_name: &str) -> bool {
        self.config
            .exclude_providers
            .iter()
            .any(|p| p.eq_ignore_ascii_case(provider_name))
    }

    /// Check whether a provider is excluded, considering platform overrides.
    #[must_use]
    pub fn is_platform_excluded(&self, platform: Option<&str>, provider: &str) -> bool {
        if self.is_provider_excluded(provider) {
            return true;
        }
        if let Some(p) = platform {
            if let Some(policy) = self.config.platform_policies.get(p) {
                if let Some(ref excluded) = policy.exclude_providers {
                    if excluded.iter().any(|e| e.eq_ignore_ascii_case(provider)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Get the configured action for a rule by name from the given config.
    fn action_for_rule<'a>(config: &'a PrivacyConfig, rule_name: &str) -> &'a PiiAction {
        match rule_name {
            "phone" => &config.phone,
            "id_card" => &config.id_card,
            "bank_card" => &config.bank_card,
            "email" => &config.email,
            "ip_address" => &config.ip_address,
            "api_key" => &config.api_key,
            "ssh_key" => &config.ssh_key,
            _ => {
                // Look up custom rule action by name
                config
                    .custom_rules
                    .iter()
                    .find(|r| r.name == rule_name)
                    .map_or_else(|| &PiiAction::Block, |r| &r.action)
            }
        }
    }

    /// Compute an effective `PrivacyConfig` by applying platform overrides.
    fn effective_config(&self, platform: Option<&str>) -> PrivacyConfig {
        // Owned copy is required because platform overrides mutate the config.
        // rust-doctor-disable-next-line excessive-clone
        let mut cfg = self.config.clone();
        if let Some(p) = platform {
            if let Some(policy) = self.config.platform_policies.get(p) {
                if let Some(v) = policy.pii_filtering {
                    cfg.pii_filtering = v;
                }
                if let Some(ref v) = policy.id_card {
                    // rust-doctor-disable-next-line excessive-clone
                    cfg.id_card = v.clone();
                }
                if let Some(ref v) = policy.bank_card {
                    // rust-doctor-disable-next-line excessive-clone
                    cfg.bank_card = v.clone();
                }
                if let Some(ref v) = policy.phone {
                    // rust-doctor-disable-next-line excessive-clone
                    cfg.phone = v.clone();
                }
                if let Some(ref v) = policy.api_key {
                    // rust-doctor-disable-next-line excessive-clone
                    cfg.api_key = v.clone();
                }
                if let Some(ref v) = policy.ssh_key {
                    // rust-doctor-disable-next-line excessive-clone
                    cfg.ssh_key = v.clone();
                }
                if let Some(ref v) = policy.email {
                    // rust-doctor-disable-next-line excessive-clone
                    cfg.email = v.clone();
                }
                if let Some(ref v) = policy.ip_address {
                    // rust-doctor-disable-next-line excessive-clone
                    cfg.ip_address = v.clone();
                }
            }
        }
        cfg
    }

    /// Filter PII from text using a specific config.
    fn filter_with_config(&self, text: &str, config: &PrivacyConfig) -> FilterResult {
        if !config.pii_filtering {
            return FilterResult::unchanged(text);
        }

        let mut all_matches: Vec<PiiMatch> = Vec::new();

        for rule in &self.rules {
            let action = Self::action_for_rule(config, rule.name());
            if *action == PiiAction::Off {
                continue;
            }

            let matches = rule.detect(text);

            for m in matches {
                if !self.allowlist.is_allowed(&m.matched_text, rule.name()) {
                    all_matches.push(m);
                }
            }
        }

        if all_matches.is_empty() {
            return FilterResult::unchanged(text);
        }

        // Rank matches before overlap dedup: a match that will actually be
        // redacted (Block) must always win an overlap over one that won't
        // (Warn), regardless of severity. Otherwise a higher-severity Warn
        // rule (e.g. api_key=warn) silently suppresses a lower-severity Block
        // rule (e.g. phone=block) that overlaps it, leaking the lower-severity
        // PII in plaintext. Within equal block-ness, higher severity wins.
        all_matches.sort_by_key(|m| {
            let blocks = *Self::action_for_rule(config, &m.rule_name) == PiiAction::Block;
            (std::cmp::Reverse(blocks), std::cmp::Reverse(m.severity))
        });

        let deduped = dedup_overlapping(all_matches);

        // Sort by start descending so we can replace from back to front
        // without invalidating earlier offsets.
        let mut sorted = deduped;
        sorted.sort_by_key(|x| std::cmp::Reverse(x.start));

        let mut result = text.to_string();
        let mut blocked_count = 0;
        let mut warned_count = 0;

        for detection in &sorted {
            let action = Self::action_for_rule(config, &detection.rule_name);
            match action {
                PiiAction::Block => {
                    if detection.start < detection.end
                        && detection.end <= result.len()
                        && result.is_char_boundary(detection.start)
                        && result.is_char_boundary(detection.end)
                    {
                        result
                            .replace_range(detection.start..detection.end, &detection.placeholder);
                        blocked_count += 1;
                        warn!(
                            rule = %detection.rule_name,
                            severity = %detection.severity,
                            "PII detected and blocked before API call"
                        );
                    } else {
                        warn!(
                            rule = %detection.rule_name,
                            start = detection.start,
                            end = detection.end,
                            text_len = result.len(),
                            "PII match has invalid offsets, skipping replacement"
                        );
                    }
                }
                PiiAction::Warn => {
                    warned_count += 1;
                    warn!(
                        rule = %detection.rule_name,
                        severity = %detection.severity,
                        "PII detected in outbound message (warn mode)"
                    );
                }
            }
        }

        FilterResult {
            text: result,
            blocked_count,
            warned_count,
        }
    }

    /// Filter PII from text using the global config.
    #[must_use]
    pub fn filter(&self, text: &str) -> FilterResult {
        self.filter_with_config(text, &self.config)
    }

    /// Filter PII from text, applying platform-specific overrides if provided.
    #[must_use]
    pub fn filter_with_platform(&self, text: &str, platform: Option<&str>) -> FilterResult {
        let config = self.effective_config(platform);
        self.filter_with_config(text, &config)
    }
}

/// Remove overlapping matches, keeping the highest-priority one.
///
/// The caller must pre-sort `matches` by priority descending — redacting
/// (Block) matches first, then severity descending. When two matches overlap,
/// the earlier (higher-priority) one in the pre-sorted input is retained.
fn dedup_overlapping(matches: Vec<PiiMatch>) -> Vec<PiiMatch> {
    if matches.len() <= 1 {
        return matches;
    }

    let mut result: Vec<PiiMatch> = Vec::new();
    for m in matches {
        let overlaps = result
            .iter()
            .any(|existing| m.start < existing.end && m.end > existing.start);
        if !overlaps {
            result.push(m);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PiiAction, PlatformPiiPolicy, PrivacyConfig};

    fn engine() -> PiiEngine {
        PiiEngine::new(PrivacyConfig::default())
    }

    #[test]
    fn test_filter_phone_number() {
        let result = engine().filter("Call me at 13812345678");
        assert_eq!(result.text, "Call me at [PHONE]");
        assert_eq!(result.blocked_count, 1);
    }

    #[test]
    fn test_filter_multiple_pii_types() {
        let result = engine().filter("Phone: 13812345678, ID: 11010119900307002X");
        assert!(result.text.contains("[PHONE]"));
        assert!(result.text.contains("[ID_CARD]"));
        assert_eq!(result.blocked_count, 2);
    }

    #[test]
    fn test_filter_disabled() {
        let config = PrivacyConfig {
            pii_filtering: false,
            ..Default::default()
        };
        let engine = PiiEngine::new(config);
        let result = engine.filter("Phone: 13812345678");
        assert_eq!(result.text, "Phone: 13812345678");
        assert_eq!(result.blocked_count, 0);
    }

    #[test]
    fn test_filter_warn_mode_no_replacement() {
        let config = PrivacyConfig {
            phone: PiiAction::Warn,
            ..Default::default()
        };
        let engine = PiiEngine::new(config);
        let result = engine.filter("Phone: 13812345678");
        // Warn mode: original text preserved, but warned
        assert_eq!(result.text, "Phone: 13812345678");
        assert_eq!(result.warned_count, 1);
        assert_eq!(result.blocked_count, 0);
    }

    #[test]
    fn test_filter_off_mode_no_detection() {
        let config = PrivacyConfig {
            phone: PiiAction::Off,
            ..Default::default()
        };
        let engine = PiiEngine::new(config);
        let result = engine.filter("Phone: 13812345678");
        assert_eq!(result.text, "Phone: 13812345678");
        assert_eq!(result.warned_count, 0);
    }

    #[test]
    fn test_filter_no_pii() {
        let result = engine().filter("Normal text with no personal info");
        assert_eq!(result.text, "Normal text with no personal info");
        assert!(!result.has_detections());
    }

    #[test]
    fn test_filter_test_phone_allowed() {
        // 13800138000 is in the allowlist
        let result = engine().filter("Test: 13800138000");
        assert_eq!(result.blocked_count, 0);
    }

    #[test]
    fn test_filter_excluded_provider() {
        let config = PrivacyConfig {
            exclude_providers: vec!["ollama".to_string()],
            ..Default::default()
        };
        let engine = PiiEngine::new(config);
        assert!(engine.is_provider_excluded("ollama"));
        assert!(!engine.is_provider_excluded("anthropic"));
    }

    #[test]
    fn test_filter_with_platform_override() {
        let mut config = PrivacyConfig {
            phone: PiiAction::Block,
            ..Default::default()
        };
        let policy = PlatformPiiPolicy {
            phone: Some(PiiAction::Warn),
            ..Default::default()
        };
        config
            .platform_policies
            .insert("discord".to_string(), policy);

        let engine = PiiEngine::new(config);
        let default_result = engine.filter_with_platform("Call 13812345678", None);
        assert!(default_result.text.contains("[PHONE]"));

        let discord_result = engine.filter_with_platform("Call 13812345678", Some("discord"));
        assert_eq!(discord_result.text, "Call 13812345678");
        assert!(discord_result.warned_count > 0);
    }

    #[test]
    fn test_is_platform_excluded() {
        let mut config = PrivacyConfig {
            exclude_providers: vec!["ollama".to_string()],
            ..Default::default()
        };
        let policy = PlatformPiiPolicy {
            exclude_providers: Some(vec!["local-llm".to_string()]),
            ..Default::default()
        };
        config
            .platform_policies
            .insert("telegram".to_string(), policy);

        let engine = PiiEngine::new(config);
        assert!(engine.is_platform_excluded(None, "ollama"));
        assert!(engine.is_platform_excluded(Some("telegram"), "local-llm"));
        assert!(!engine.is_platform_excluded(Some("telegram"), "anthropic"));
    }

    #[test]
    fn test_reload_updates_rules() {
        let initial_config = PrivacyConfig::default();
        PiiEngine::init(initial_config);

        let mut new_config = PrivacyConfig::default();
        new_config
            .custom_rules
            .push(crate::config::types::CustomPiiRule {
                name: "test_token".to_string(),
                pattern: r"TK-[0-9]{4}".to_string(),
                placeholder: "[TK]".to_string(),
                severity: crate::config::types::CustomPiiSeverity::High,
                action: PiiAction::Block,
            });

        PiiEngine::reload(new_config);

        let engine = PiiEngine::global().unwrap();
        let guard = engine.read().unwrap_or_else(|e| e.into_inner());
        let result = guard.filter("Token: TK-1234");
        assert_eq!(result.blocked_count, 1);
        assert!(result.text.contains("[TK]"));
    }

    #[test]
    fn test_dedup_overlapping_prefers_higher_severity() {
        // Phone (High) and ID card (Critical) overlap — Critical should win.
        // 11010119900307002X is a valid ID card.
        // The digits "1990030700" also match the phone pattern.
        let result = engine().filter("ID: 11010119900307002X");
        assert!(result.text.contains("[ID_CARD]"));
    }

    #[test]
    fn test_overlap_block_wins_over_higher_severity_warn() {
        // Regression: a higher-severity Warn match must NOT suppress an
        // overlapping lower-severity Block match (would leak PII in plaintext).
        // api_key (Critical) = Warn, phone (High) = Block. The Bearer token
        // span contains a phone that passes its own boundary check.
        let config = PrivacyConfig {
            api_key: PiiAction::Warn,
            phone: PiiAction::Block,
            ..Default::default()
        };
        let engine = PiiEngine::new(config);
        let result = engine.filter("Bearer 13912345678.abcdefghijklmnop");
        assert!(
            result.text.contains("[PHONE]"),
            "phone (block) must win the overlap vs api_key (warn), got: {}",
            result.text
        );
        assert_eq!(result.blocked_count, 1);
    }

    #[test]
    fn test_dedup_overlapping_bug_low_start_greater_than_high() {
        // Regression test for severity-priority bug:
        // When a Low-severity match has a larger start offset than a High-severity
        // match they overlap with, the Low match incorrectly wins because
        // `dedup_overlapping` sorts by start descending instead of severity.
        let mut config = PrivacyConfig::default();
        config
            .custom_rules
            .push(crate::config::types::CustomPiiRule {
                name: "low_overlap".to_string(),
                pattern: r"5320151128303\d+".to_string(),
                placeholder: "[LOW]".to_string(),
                severity: crate::config::types::CustomPiiSeverity::Low,
                action: PiiAction::Block,
            });

        let engine = PiiEngine::new(config);
        // Bank card (High) matches "4532015112830366" at start=6, end=22.
        // Custom rule (Low) matches "53201511283036"   at start=7, end=21.
        // They overlap. High severity should win -> [BANK_CARD].
        let result = engine.filter("Card: 4532015112830366");
        assert!(
            result.text.contains("[BANK_CARD]"),
            "High-severity bank_card should win over Low-severity custom rule, but got: {}",
            result.text
        );
        assert!(
            !result.text.contains("[LOW]"),
            "Low-severity match should be discarded, but got: {}",
            result.text
        );
    }
}
