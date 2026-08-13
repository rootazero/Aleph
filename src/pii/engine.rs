//! Core PII detection and replacement engine

use crate::config::PiiAction;
use crate::config::PrivacyConfig;
use crate::pii::allowlist::PiiAllowlist;
use crate::pii::rules::PiiRule;
use crate::sync_primitives::{Arc, RwLock};
use std::collections::{HashMap, HashSet};
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
    /// Number of PII matches that were detected but skipped due to
    /// invalid offsets (the offsets returned by `regex.find_iter` were
    /// outside the text or on a non-char boundary). Non-zero indicates
    /// a bug in offset tracking or upstream mutation; the audit pipeline
    /// (`runtime_guard`) can use this as a triage signal.
    pub skipped_count: usize,
}

impl FilterResult {
    #[must_use]
    pub fn unchanged(text: &str) -> Self {
        Self {
            text: text.to_string(),
            blocked_count: 0,
            warned_count: 0,
            skipped_count: 0,
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
    /// Pre-computed `name → action` map for custom rules so the per-match
    /// action lookup is O(1) instead of an O(M) linear scan over
    /// `config.custom_rules`. Built-ins still resolve through `action_for_rule`'s
    /// match arm.
    custom_rule_actions: HashMap<String, PiiAction>,
    /// Lower-cased provider names excluded from filtering, so the per-call
    /// `is_provider_excluded` check is O(1).
    excluded_providers: HashSet<String>,
}

impl PiiEngine {
    /// Create a new PII engine with the given configuration
    #[must_use]
    pub fn new(config: PrivacyConfig) -> Self {
        let configured_custom = config.custom_rules.len();
        let rules = crate::pii::rules::build_rules(&config.custom_rules);
        let loaded_custom = rules.len().saturating_sub(7); // 7 built-ins
        if loaded_custom < configured_custom {
            // `build_rules` already warns on each invalid pattern; this
            // summary surfaces a single operator-facing signal so a
            // dashboard / health check can flag a half-loaded config.
            warn!(
                configured_custom_rules = configured_custom,
                loaded_custom_rules = loaded_custom,
                skipped = configured_custom - loaded_custom,
                "Custom PII rules partially loaded; some patterns failed to compile"
            );
        }
        let allowlist = PiiAllowlist::default();
        let custom_rule_actions = config
            .custom_rules
            .iter()
            .map(|r| (r.name.clone(), r.action.clone()))
            .collect();
        let excluded_providers = config
            .exclude_providers
            .iter()
            .map(|p| p.to_ascii_lowercase())
            .collect();
        Self {
            rules,
            allowlist,
            config,
            custom_rule_actions,
            excluded_providers,
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
            // Build rules and lookup tables outside the lock to avoid blocking readers.
            let new_rules = crate::pii::rules::build_rules(&config.custom_rules);
            let custom_rule_actions = config
                .custom_rules
                .iter()
                .map(|r| (r.name.clone(), r.action.clone()))
                .collect();
            let excluded_providers = config
                .exclude_providers
                .iter()
                .map(|p| p.to_ascii_lowercase())
                .collect();
            let mut guard = engine.write().unwrap_or_else(|e| e.into_inner());
            guard.rules = new_rules;
            guard.config = config;
            guard.custom_rule_actions = custom_rule_actions;
            guard.excluded_providers = excluded_providers;
        } else {
            warn!("PiiEngine::reload called but engine not initialized, ignoring");
        }
    }

    /// Check if a specific provider should be excluded from filtering
    #[must_use]
    pub fn is_provider_excluded(&self, provider_name: &str) -> bool {
        let lower = provider_name.to_ascii_lowercase();
        self.excluded_providers.contains(&lower)
    }

    /// Look up a platform policy by key, case-insensitively.
    ///
    /// Operators may write `[platform_policies.Telegram]` while the runtime
    /// passes `"telegram"` (or vice versa). Without a case-insensitive
    /// lookup the policy is silently skipped, falling back to the global
    /// config — which can mean PII that the operator explicitly relaxed on
    /// that platform is now blocked, or vice versa. The literal key is tried
    /// first as a fast path; otherwise both sides are folded.
    fn lookup_platform_policy(&self, platform: &str) -> Option<&crate::config::PlatformPiiPolicy> {
        if let Some(p) = self.config.platform_policies.get(platform) {
            return Some(p);
        }
        // The fallback used to lower-case the *runtime* key and look that up,
        // which only folds one of the two directions the doc above promises:
        // it found `[platform_policies.telegram]` for a runtime `"TELEGRAM"`,
        // and missed `[platform_policies.Telegram]` for a runtime `"telegram"`
        // — the direction an operator actually writes, because a capitalised
        // platform name is how the product spells it everywhere else.
        //
        // Both sides have to be folded, so the comparison is over the keys
        // rather than a second lookup. `platform_policies` holds one entry per
        // configured platform, so the scan is over a handful of strings on a
        // path that already allocates.
        //
        // The tie-break is not decoration: a config carrying both `Telegram`
        // and `telegram` has two matches, and picking whichever the map
        // happens to yield first would make the effective policy re-roll on
        // every process start. Smallest key wins, deterministically.
        self.config
            .platform_policies
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(platform))
            .min_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, policy)| policy)
    }

    /// Check whether a provider is excluded, considering platform overrides.
    #[must_use]
    pub fn is_platform_excluded(&self, platform: Option<&str>, provider: &str) -> bool {
        if self.is_provider_excluded(provider) {
            return true;
        }
        if let Some(p) = platform {
            if let Some(policy) = self.lookup_platform_policy(p) {
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
    ///
    /// Built-in rule names hit the `match` arm (O(1)). Custom rule names
    /// are resolved through the engine's pre-computed `custom_rule_actions`
    /// HashMap (also O(1)) instead of an O(M) linear scan over
    /// `config.custom_rules`. An unknown rule defaults to [`PiiAction::Block`].
    fn action_for_rule<'a>(
        config: &'a PrivacyConfig,
        custom_actions: &'a HashMap<String, PiiAction>,
        rule_name: &str,
    ) -> &'a PiiAction {
        match rule_name {
            "phone" => &config.phone,
            "id_card" => &config.id_card,
            "bank_card" => &config.bank_card,
            "email" => &config.email,
            "ip_address" => &config.ip_address,
            "api_key" => &config.api_key,
            "ssh_key" => &config.ssh_key,
            _ => custom_actions.get(rule_name).unwrap_or(&PiiAction::Block),
        }
    }

    /// Compute an effective `PrivacyConfig` by applying platform overrides.
    fn effective_config(&self, platform: Option<&str>) -> PrivacyConfig {
        let Some(p) = platform else {
            // No platform override requested; the global config stands.
            return self.config.clone();
        };
        let Some(policy) = self.lookup_platform_policy(p) else {
            // Platform named but no policy defined for it (case-insensitive
            // lookup already attempted) — fall back to global.
            return self.config.clone();
        };
        if !policy_has_any_override(policy) {
            // Policy exists but every field is `None`; treat as identity.
            return self.config.clone();
        }
        // Owned copy is required because at least one override mutates the config.
        // rust-doctor-disable-next-line excessive-clone
        let mut cfg = self.config.clone();
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
        cfg
    }

    /// Filter PII from text using a specific config.
    fn filter_with_config(&self, text: &str, config: &PrivacyConfig) -> FilterResult {
        if !config.pii_filtering {
            return FilterResult::unchanged(text);
        }

        let mut all_matches: Vec<PiiMatch> = Vec::new();

        for rule in &self.rules {
            let action = Self::action_for_rule(config, &self.custom_rule_actions, rule.name());
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
            let blocks = *Self::action_for_rule(config, &self.custom_rule_actions, &m.rule_name)
                == PiiAction::Block;
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
        let mut skipped_count = 0;

        for detection in &sorted {
            let action =
                Self::action_for_rule(config, &self.custom_rule_actions, &detection.rule_name);
            match action {
                PiiAction::Off => {}
                PiiAction::Block => {
                    if detection.start < detection.end
                        && detection.end <= result.len()
                        && result.is_char_boundary(detection.start)
                        && result.is_char_boundary(detection.end)
                    {
                        result
                            .replace_range(detection.start..detection.end, &detection.placeholder);
                        blocked_count += 1;
                        // Per-match lines go to `debug!` to avoid log floods
                        // at normal traffic (1–3 PII matches per outbound
                        // message). The audit pipeline (`runtime_guard`) emits
                        // a single `PiiDetected` entry per call, which is the
                        // operator-facing signal.
                        tracing::debug!(
                            rule = %detection.rule_name,
                            severity = %detection.severity,
                            "PII detected and blocked before API call"
                        );
                    } else {
                        skipped_count += 1;
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
                    // Warn-mode detections are operator-visible; keep at
                    // `warn!`. Volume is bounded by user opt-in to the
                    // Warn action — not on by default for most categories.
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
            skipped_count,
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

/// True when at least one platform-policy field is populated.
///
/// Used by [`PiiEngine::effective_config`] to skip the full
/// `PrivacyConfig` clone when the named platform policy has no effect
/// (every field is `None`), or when no policy is defined for the
/// requested platform.
fn policy_has_any_override(policy: &crate::config::PlatformPiiPolicy) -> bool {
    policy.pii_filtering.is_some()
        || policy.id_card.is_some()
        || policy.bank_card.is_some()
        || policy.phone.is_some()
        || policy.api_key.is_some()
        || policy.ssh_key.is_some()
        || policy.email.is_some()
        || policy.ip_address.is_some()
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
    fn test_platform_lookup_is_case_insensitive() {
        // Operator writes `[platform_policies.Telegram]` (capital T);
        // runtime passes "telegram". The policy MUST apply — otherwise
        // the operator's per-platform PII overrides are silently dropped
        // when case drifts between config and runtime.
        let mut config = PrivacyConfig::default();
        let policy = PlatformPiiPolicy {
            exclude_providers: Some(vec!["local-llm".to_string()]),
            ..Default::default()
        };
        config
            .platform_policies
            .insert("Telegram".to_string(), policy);
        let engine = PiiEngine::new(config);

        // Lower-case runtime key against upper-case config key.
        assert!(engine.is_platform_excluded(Some("telegram"), "local-llm"));
        assert!(!engine.is_platform_excluded(Some("telegram"), "anthropic"));
        // Upper-case runtime key against lower-case config key (reverse).
        let mut config = PrivacyConfig::default();
        let policy = PlatformPiiPolicy {
            exclude_providers: Some(vec!["local-llm".to_string()]),
            ..Default::default()
        };
        config
            .platform_policies
            .insert("telegram".to_string(), policy);
        let engine = PiiEngine::new(config);
        assert!(engine.is_platform_excluded(Some("TELEGRAM"), "local-llm"));
    }

    #[test]
    fn test_filter_with_platform_override_case_insensitive() {
        // Same regression as `test_platform_lookup_is_case_insensitive`
        // but exercised through the production `filter_with_platform` path.
        let mut config = PrivacyConfig {
            phone: PiiAction::Block,
            ..Default::default()
        };
        let policy = PlatformPiiPolicy {
            phone: Some(PiiAction::Warn),
            ..Default::default()
        };
        // Operator writes the policy in upper case.
        config
            .platform_policies
            .insert("Discord".to_string(), policy);
        let engine = PiiEngine::new(config);

        // Runtime passes "discord" (lower case) — the override MUST take
        // effect; otherwise the phone would be redacted as `[PHONE]`.
        let result = engine.filter_with_platform("Call 13812345678", Some("discord"));
        assert_eq!(
            result.text, "Call 13812345678",
            "platform override must apply despite case drift between config and runtime"
        );
        assert!(result.warned_count > 0);
    }

    #[test]
    fn test_effective_config_no_override_avoids_clone() {
        // Sanity check that the no-platform-override fast path still
        // returns a usable config (this exercises the `return self.config.clone()`
        // short-circuit added alongside the case-insensitive lookup).
        let config = PrivacyConfig::default();
        let engine = PiiEngine::new(config.clone());
        let result = engine.filter("Phone: 13812345678");
        assert!(result.text.contains("[PHONE]"));
    }

    /// `PiiEngine::init` is meant to run once at boot; a second call must warn
    /// and leave the installed engine alone, because every concurrent reader of
    /// the global holds a handle to it.
    ///
    /// The engine is a **process** global and libtest runs in parallel, so the
    /// test cannot be written as "install mine first, then check mine survived"
    /// — `test_reload_updates_rules` initialises the same `OnceLock`, and
    /// whichever test the scheduler starts first decides whose config is live.
    /// Written that way it passed alone and failed at random in a full run,
    /// which is the worst shape a guard can have: the isolated run is the one
    /// telling the comforting story.
    ///
    /// The contract is order-independent if it is stated as a negative. Claim
    /// the lock first with a config that asserts nothing (idempotent — it
    /// either installs or is ignored), then init a marked config and require
    /// that the marker is *not* live. Both orders, and a concurrent `reload`,
    /// satisfy it for the same reason: a second `init` never installs.
    #[test]
    fn a_second_init_never_replaces_the_installed_engine() {
        PiiEngine::init(PrivacyConfig::default());

        let mut second = PrivacyConfig::default();
        second
            .custom_rules
            .push(crate::config::types::CustomPiiRule {
                name: "init_test_second".to_string(),
                pattern: r"INIT_SECOND_[A-Z0-9]{4}".to_string(),
                placeholder: "[SECOND]".to_string(),
                severity: crate::config::types::CustomPiiSeverity::High,
                action: PiiAction::Block,
            });
        PiiEngine::init(second);

        let engine = PiiEngine::global().expect("the init above installs one if nothing had");
        let guard = engine.read().unwrap_or_else(|e| e.into_inner());
        let miss = guard.filter("hit INIT_SECOND_AB12");
        assert_eq!(
            miss.blocked_count, 0,
            "the second init's rule became live, so init replaced the engine \
             under every handle already held"
        );
        assert!(!miss.text.contains("[SECOND]"));
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
