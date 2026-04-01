//! Adaptive Retrieval Gate
//!
//! Determines whether a memory retrieval is needed for a given query.
//! Uses CJK-aware length thresholds, skip/force pattern matching,
//! and slash command detection to avoid unnecessary retrieval calls.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the adaptive retrieval gate.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AdaptiveRetrievalConfig {
    /// Whether the adaptive gate is enabled. When disabled, all queries go
    /// through retrieval (returns `Retrieve`).
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Minimum character count for queries containing CJK characters.
    #[serde(default = "default_min_length_cjk")]
    pub min_length_cjk: usize,

    /// Minimum character count for queries without CJK characters.
    #[serde(default = "default_min_length_other")]
    pub min_length_other: usize,

    /// Patterns that cause the gate to skip retrieval (exact match on
    /// trimmed, lowercased query).
    #[serde(default = "default_skip_patterns")]
    pub skip_patterns: Vec<String>,

    /// Patterns that force retrieval (substring match, case-insensitive).
    /// Force patterns take priority over skip patterns.
    #[serde(default = "default_force_patterns")]
    pub force_patterns: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_min_length_cjk() -> usize {
    6
}

fn default_min_length_other() -> usize {
    15
}

fn default_skip_patterns() -> Vec<String> {
    vec![
        "hello".into(),
        "hi".into(),
        "hey".into(),
        "yes".into(),
        "no".into(),
        "ok".into(),
        "thanks".into(),
        "thank you".into(),
        "bye".into(),
        "goodbye".into(),
        "你好".into(),
        "好的".into(),
        "谢谢".into(),
        "再见".into(),
    ]
}

fn default_force_patterns() -> Vec<String> {
    vec![
        "remember".into(),
        "recall".into(),
        "last time".into(),
        "previously".into(),
        "earlier".into(),
        "you said".into(),
        "you told".into(),
        "my preference".into(),
        "我记得".into(),
        "上次".into(),
        "之前".into(),
        "你说过".into(),
        "我的偏好".into(),
    ]
}

impl Default for AdaptiveRetrievalConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            min_length_cjk: default_min_length_cjk(),
            min_length_other: default_min_length_other(),
            skip_patterns: default_skip_patterns(),
            force_patterns: default_force_patterns(),
        }
    }
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// The outcome of evaluating a query against the adaptive retrieval gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrievalDecision {
    /// Normal retrieval should proceed.
    Retrieve,
    /// Retrieval can safely be skipped.
    Skip,
    /// Retrieval must be performed (memory-related query detected).
    ForceRetrieve,
}

// ---------------------------------------------------------------------------
// Gate
// ---------------------------------------------------------------------------

/// Adaptive retrieval gate that decides whether to perform memory retrieval
/// for a given user query.
pub struct AdaptiveRetrievalGate {
    config: AdaptiveRetrievalConfig,
}

impl AdaptiveRetrievalGate {
    /// Create a new gate with the given configuration.
    pub fn new(config: AdaptiveRetrievalConfig) -> Self {
        Self { config }
    }

    /// Evaluate a query and return a [`RetrievalDecision`].
    ///
    /// Priority order:
    /// 1. If gate is disabled → `Retrieve`
    /// 2. Force patterns (substring, case-insensitive) → `ForceRetrieve`
    /// 3. Skip patterns (exact match, trimmed + lowercased) → `Skip`
    /// 4. Slash commands (starts with `/`) → `Skip`
    /// 5. Length check (CJK-aware) → `Skip` if too short
    /// 6. Default → `Retrieve`
    pub fn evaluate(&self, query: &str) -> RetrievalDecision {
        if !self.config.enabled {
            return RetrievalDecision::Retrieve;
        }

        let trimmed = query.trim();
        let lowered = trimmed.to_lowercase();

        // 1. Force patterns (highest priority) — substring match
        for pattern in &self.config.force_patterns {
            if lowered.contains(&pattern.to_lowercase()) {
                return RetrievalDecision::ForceRetrieve;
            }
        }

        // 2. Skip patterns — exact match on lowered query
        for pattern in &self.config.skip_patterns {
            if lowered == pattern.to_lowercase() {
                return RetrievalDecision::Skip;
            }
        }

        // 3. Slash commands
        if trimmed.starts_with('/') {
            return RetrievalDecision::Skip;
        }

        // 4. Length check with CJK awareness
        let has_cjk = trimmed.chars().any(is_cjk);
        let min_len = if has_cjk {
            self.config.min_length_cjk
        } else {
            self.config.min_length_other
        };

        if trimmed.chars().count() < min_len {
            return RetrievalDecision::Skip;
        }

        RetrievalDecision::Retrieve
    }
}

/// Returns `true` if the character falls within the CJK Unified Ideographs block.
fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> AdaptiveRetrievalGate {
        AdaptiveRetrievalGate::new(AdaptiveRetrievalConfig::default())
    }

    // -- Force patterns -------------------------------------------------------

    #[test]
    fn force_pattern_english() {
        assert_eq!(
            gate().evaluate("Do you remember what I said?"),
            RetrievalDecision::ForceRetrieve,
        );
    }

    #[test]
    fn force_pattern_chinese() {
        assert_eq!(
            gate().evaluate("我记得你之前说过一些东西"),
            RetrievalDecision::ForceRetrieve,
        );
    }

    #[test]
    fn force_pattern_case_insensitive() {
        assert_eq!(
            gate().evaluate("RECALL what happened"),
            RetrievalDecision::ForceRetrieve,
        );
    }

    #[test]
    fn force_overrides_skip() {
        // "hello" is a skip pattern, but "remember" is a force pattern.
        assert_eq!(
            gate().evaluate("do you remember saying hello"),
            RetrievalDecision::ForceRetrieve,
        );
    }

    // -- Skip patterns --------------------------------------------------------

    #[test]
    fn skip_greeting_english() {
        assert_eq!(gate().evaluate("hello"), RetrievalDecision::Skip);
    }

    #[test]
    fn skip_greeting_chinese() {
        assert_eq!(gate().evaluate("你好"), RetrievalDecision::Skip);
    }

    #[test]
    fn skip_greeting_case_insensitive() {
        assert_eq!(gate().evaluate("Hello"), RetrievalDecision::Skip);
    }

    #[test]
    fn skip_greeting_with_whitespace() {
        assert_eq!(gate().evaluate("  ok  "), RetrievalDecision::Skip);
    }

    // -- Slash commands -------------------------------------------------------

    #[test]
    fn skip_command_new() {
        assert_eq!(gate().evaluate("/new"), RetrievalDecision::Skip);
    }

    #[test]
    fn skip_command_help() {
        assert_eq!(gate().evaluate("/help"), RetrievalDecision::Skip);
    }

    // -- Length thresholds ----------------------------------------------------

    #[test]
    fn skip_short_english() {
        // "what?" is 5 chars, below the 15-char threshold
        assert_eq!(gate().evaluate("what?"), RetrievalDecision::Skip);
    }

    #[test]
    fn retrieve_long_english() {
        assert_eq!(
            gate().evaluate("Tell me about the architecture of this system"),
            RetrievalDecision::Retrieve,
        );
    }

    #[test]
    fn retrieve_cjk_above_threshold() {
        // 10 CJK chars — above the 6-char threshold
        assert_eq!(
            gate().evaluate("这个系统的架构是什么样"),
            RetrievalDecision::Retrieve,
        );
    }

    #[test]
    fn skip_short_cjk() {
        // 3 CJK chars — below the 6-char threshold
        assert_eq!(gate().evaluate("怎么办"), RetrievalDecision::Skip);
    }

    // -- Disabled gate --------------------------------------------------------

    #[test]
    fn disabled_always_retrieves() {
        let g = AdaptiveRetrievalGate::new(AdaptiveRetrievalConfig {
            enabled: false,
            ..Default::default()
        });
        // Even a greeting should go through when disabled
        assert_eq!(g.evaluate("hello"), RetrievalDecision::Retrieve);
    }

    // -- Default config -------------------------------------------------------

    #[test]
    fn default_config_values() {
        let cfg = AdaptiveRetrievalConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.min_length_cjk, 6);
        assert_eq!(cfg.min_length_other, 15);
        assert!(!cfg.skip_patterns.is_empty());
        assert!(!cfg.force_patterns.is_empty());
        assert!(cfg.skip_patterns.contains(&"hello".to_string()));
        assert!(cfg.force_patterns.contains(&"remember".to_string()));
    }
}
