//! Experimental feature flags and configuration
//!
//! This module contains feature flags for experimental features that can be
//! enabled or disabled via configuration. All experimental features default
//! to disabled (false) for backward compatibility.
//!
//! # Example Configuration
//!
//! ```toml
//! [policies.experimental]
//! use_unified_intent_decider = true
//! use_new_prompt_system = true
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Experimental feature configuration
///
/// Controls experimental features that are still being tested.
/// All flags default to `false` for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[derive(Default)]
pub struct ExperimentalPolicy {
    /// Use the new unified ExecutionIntentDecider instead of legacy IntentClassifier.
    ///
    /// The new decider provides:
    /// - Single decision point for "execute vs converse"
    /// - L0-L4 layered decision logic
    /// - Default bias toward execution
    ///
    /// Default: false (use legacy IntentClassifier)
    #[serde(default)]
    pub use_unified_intent_decider: bool,

    /// Use the new streamlined prompt system from the `prompt` module.
    ///
    /// The new prompt system:
    /// - Removes negative instructions ("don't do X")
    /// - Uses ~300 tokens instead of ~2000 tokens
    /// - Separates executor and conversational prompts
    ///
    /// Default: false (use legacy AgentModePrompt)
    #[serde(default)]
    pub use_new_prompt_system: bool,

    /// Enable verbose decision logging for debugging.
    ///
    /// When enabled, logs detailed information about:
    /// - ExecutionIntentDecider decision layer and confidence
    /// - Prompt selection and token counts
    ///
    /// Default: false
    #[serde(default)]
    pub verbose_decision_logging: bool,
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_all_disabled() {
        let policy = ExperimentalPolicy::default();
        assert!(!policy.use_unified_intent_decider);
        assert!(!policy.use_new_prompt_system);
        assert!(!policy.verbose_decision_logging);
    }

    #[test]
    fn test_parse_from_toml() {
        let toml = r#"
            use_unified_intent_decider = true
            use_new_prompt_system = true
        "#;
        let policy: ExperimentalPolicy = toml::from_str(toml).unwrap();
        assert!(policy.use_unified_intent_decider);
        assert!(policy.use_new_prompt_system);
        assert!(!policy.verbose_decision_logging); // default
    }

    #[test]
    fn test_empty_uses_defaults() {
        let policy: ExperimentalPolicy = toml::from_str("").unwrap();
        assert!(!policy.use_unified_intent_decider);
        assert!(!policy.use_new_prompt_system);
    }
}
