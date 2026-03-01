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
#[derive(Debug, Clone)]
pub struct PiiMatch {
    pub rule_name: String,
    pub start: usize,
    pub end: usize,
    pub matched_text: String,
    pub severity: PiiSeverity,
    pub placeholder: String,
}

/// Result of PII filtering
#[derive(Debug, Clone)]
pub struct FilterResult {
    /// The filtered text (with PII replaced by placeholders if blocked)
    pub text: String,
    /// Number of PII matches that were blocked (replaced)
    pub blocked_count: usize,
    /// Number of PII matches that were warned (not replaced)
    pub warned_count: usize,
}

impl FilterResult {
    pub fn unchanged(text: &str) -> Self {
        Self {
            text: text.to_string(),
            blocked_count: 0,
            warned_count: 0,
        }
    }

    /// True if any PII was detected (blocked or warned)
    pub fn has_detections(&self) -> bool {
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
    pub fn new(config: PrivacyConfig) -> Self {
        let rules = crate::pii::rules::build_rules();
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
    pub fn global() -> Option<Arc<RwLock<PiiEngine>>> {
        PII_ENGINE.get().cloned()
    }

    /// Reload configuration (hot-reload support)
    pub fn reload(config: PrivacyConfig) {
        if let Some(engine) = PII_ENGINE.get() {
            if let Ok(mut guard) = engine.write() {
                guard.config = config;
            }
        }
    }

    /// Check if a specific provider should be excluded from filtering
    pub fn is_provider_excluded(&self, provider_name: &str) -> bool {
        self.config
            .exclude_providers
            .iter()
            .any(|p| p == provider_name)
    }

    /// Get the configured action for a rule by name
    fn action_for_rule(&self, rule_name: &str) -> &PiiAction {
        match rule_name {
            "phone" => &self.config.phone,
            "id_card" => &self.config.id_card,
            "bank_card" => &self.config.bank_card,
            "email" => &self.config.email,
            "ip_address" => &self.config.ip_address,
            "api_key" => &self.config.api_key,
            "ssh_key" => &self.config.ssh_key,
            _ => &PiiAction::Block,
        }
    }

    /// Filter PII from text
    pub fn filter(&self, text: &str) -> FilterResult {
        if !self.config.pii_filtering {
            return FilterResult::unchanged(text);
        }

        let mut all_matches: Vec<PiiMatch> = Vec::new();

        // Run all rules
        for rule in &self.rules {
            let action = self.action_for_rule(rule.name());
            if *action == PiiAction::Off {
                continue;
            }

            let matches = rule.detect(text);

            // Filter through allowlist
            for m in matches {
                if !self.allowlist.is_allowed(&m.matched_text, rule.name()) {
                    all_matches.push(m);
                }
            }
        }

        if all_matches.is_empty() {
            return FilterResult::unchanged(text);
        }

        // Sort by position (reverse) for safe in-place replacement
        all_matches.sort_by(|a, b| b.start.cmp(&a.start));

        // Deduplicate overlapping matches (keep first = higher severity due to rule ordering)
        let deduped = dedup_overlapping(all_matches);

        // Apply replacements
        let mut result = text.to_string();
        let mut blocked_count = 0;
        let mut warned_count = 0;

        for detection in &deduped {
            let action = self.action_for_rule(&detection.rule_name);
            match action {
                PiiAction::Block => {
                    // Safety: ensure indices are valid UTF-8 boundaries
                    if detection.start <= detection.end
                        && detection.end <= result.len()
                        && result.is_char_boundary(detection.start)
                        && result.is_char_boundary(detection.end)
                    {
                        result
                            .replace_range(detection.start..detection.end, &detection.placeholder);
                        blocked_count += 1;
                    }
                    warn!(
                        rule = %detection.rule_name,
                        severity = %detection.severity,
                        "PII detected and blocked before API call"
                    );
                }
                PiiAction::Warn => {
                    warned_count += 1;
                    warn!(
                        rule = %detection.rule_name,
                        severity = %detection.severity,
                        "PII detected in outbound message (warn mode)"
                    );
                }
                PiiAction::Off => {}
            }
        }

        FilterResult {
            text: result,
            blocked_count,
            warned_count,
        }
    }
}

/// Remove overlapping matches, keeping the one encountered first (rules are ordered by severity)
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
        // If overlapping, the already-added one wins (higher severity rule ran first)
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PiiAction, PrivacyConfig};

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
}
