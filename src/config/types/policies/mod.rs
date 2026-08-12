//! Policy configuration types for mechanism-policy separation
//!
//! This module implements the Linux philosophy of "Separate mechanism from policy"
//! by extracting configurable behavioral parameters from mechanism code.
//!
//! All policies have sensible defaults for backward compatibility - existing
//! configurations without a `[policies]` section will work unchanged.
//!
//! # Example Configuration
//!
//! ```toml
//! [policies]
//!
//! [policies.tool_safety]
//! high_risk_keywords = ["delete", "remove", "drop", "shell"]
//! builtin_fallback = "readonly"
//!
//! [policies.intent]
//! confidence_threshold = 0.75
//! timeout_ms = 2500
//!
//! [policies.memory.compression]
//! turn_threshold = 15
//! ```

pub mod exec_tier;
pub mod memory;
pub mod metrics;
pub mod session_mode;
pub mod tool_permissions;
pub mod web_fetch;

pub use exec_tier::{
    builtin_tiers, effective_permission, ExecTier, ToolFacts, EXEC_TIER_SESSION_KEY,
};
pub use memory::{CompressionPolicy, MemoryPolicies};
pub use metrics::MetricsPolicy;
pub use session_mode::{builtin_modes, SessionMode, MODE_SESSION_KEY};
pub use tool_permissions::{PermissionMatch, ToolPermissionsConfig};
pub use web_fetch::{Crawl4aiConfig, WebFetchPolicy};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One selectable position of a session dial, as offered to a user surface
/// (Panel / CLI / bot).
///
/// Core owns the dial's IDENTITY — which ids exist and in what order — because
/// every surface has to offer the same choices with the same meaning (R6). It
/// does NOT own the COPY: a label is presentation and has to follow the
/// reader's locale, so a surface that cannot author its own words is
/// structurally unable to be localized (R4). Ship ids; let the surface write
/// the sentence.
///
/// One type for all five dials rather than one struct per dial: the third and
/// fourth copies (thinking depth, memory mode) are what made the duplication
/// worth removing, and a shared shape means the Panel decodes every dial with
/// the same `{ id }` reader.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DialPreset {
    /// Canonical id — the value that goes on the wire and into session
    /// metadata.
    pub id: &'static str,
}

/// Root policies configuration
///
/// Aggregates all policy types. All fields are optional with defaults,
/// ensuring backward compatibility with existing configs that don't
/// have a `[policies]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PoliciesConfig {
    /// Memory module policies (compression + retrieval)
    #[serde(default)]
    pub memory: MemoryPolicies,

    /// Web fetch policy
    #[serde(default)]
    pub web_fetch: WebFetchPolicy,

    /// Performance metrics policy
    #[serde(default)]
    pub metrics: MetricsPolicy,

    /// Execution permission tier (Ask / Auto / Full).
    ///
    /// The user-facing dial. Projects onto `tool_permissions` at run time;
    /// explicit `tool_permissions` entries win over the tier's preset.
    #[serde(default)]
    pub exec_tier: ExecTier,

    /// Default session usage mode (chat / work / code) for sessions with no
    /// per-session override. Orthogonal to `exec_tier`: the mode partitions
    /// the tool *presentation* surface; the tier governs approvals.
    #[serde(default)]
    pub mode: SessionMode,

    /// LLM risk triage in front of human approval prompts (codex Guardian
    /// port, escalate-don't-deny variant): actions the judge finds clearly
    /// safe (low risk) auto-approve without interrupting the human;
    /// everything else — including judge errors and timeouts — still reaches
    /// the human exactly as before. Off by default; needs a configured
    /// default provider.
    #[serde(default)]
    pub guardian_review: bool,

    /// Tool permission levels (Allow / Ask / Deny)
    #[serde(default)]
    pub tool_permissions: ToolPermissionsConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_policies_uses_defaults() {
        let config: PoliciesConfig = toml::from_str("").unwrap();

        assert_eq!(config.memory.compression.turn_threshold, 20);
        assert_eq!(config.metrics.warning_multiplier, 2.0);
    }

    #[test]
    fn test_partial_policies_config() {
        let config: PoliciesConfig = toml::from_str("").unwrap();

        // Defaults for unspecified policies
        assert_eq!(config.memory.compression.turn_threshold, 20);
    }

    #[test]
    fn test_full_policies_config() {
        let toml = r#"
            [memory.compression]
            turn_threshold = 30

            [web_fetch]
            max_content_length = 50000
            user_agent = "TestBot/1.0"

            [metrics]
            warning_multiplier = 3.0
        "#;
        let config: PoliciesConfig = toml::from_str(toml).unwrap();

        // Verify all specified values
        assert_eq!(config.memory.compression.turn_threshold, 30);
        assert_eq!(config.web_fetch.max_content_length, 50000);
        assert_eq!(config.metrics.warning_multiplier, 3.0);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let config = PoliciesConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: PoliciesConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(
            config.metrics.warning_multiplier,
            parsed.metrics.warning_multiplier
        );
    }
}
