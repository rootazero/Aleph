//! Dual-defense noise filter for memory storage and retrieval.
//!
//! Provides two filtering stages:
//! 1. **Storage-time** (`should_store`): prevents low-quality content from entering
//!    the memory store (e.g., too short, pure emoji, AI denial patterns, boilerplate).
//! 2. **Retrieval-time** (`filter_results`): removes noisy results from search results
//!    that may have entered before the filter was enabled.

use crate::memory::store::types::ScoredFact;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Default value functions
// ---------------------------------------------------------------------------

fn default_enabled() -> bool {
    true
}

fn default_min_content_length() -> usize {
    10
}

fn default_denial_patterns() -> Vec<String> {
    vec![
        "i can't help with".to_string(),
        "i'm sorry, but i".to_string(),
        "i cannot assist".to_string(),
        "as an ai".to_string(),
        "i don't have the ability".to_string(),
    ]
}

fn default_boilerplate_patterns() -> Vec<String> {
    vec![
        "<system>".to_string(),
        "</system>".to_string(),
        "<relevant-memories>".to_string(),
        "</relevant-memories>".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// NoiseFilterConfig
// ---------------------------------------------------------------------------

/// Configuration for the dual-defense noise filter.
///
/// Controls which content is considered "noise" and should be rejected
/// at storage time or filtered out at retrieval time.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoiseFilterConfig {
    /// Whether the noise filter is enabled. When disabled, all content passes through.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Minimum content length (after trimming) for a fact to be stored.
    #[serde(default = "default_min_content_length")]
    pub min_content_length: usize,

    /// Case-insensitive patterns that indicate an AI denial/refusal response.
    /// Content containing any of these patterns will be rejected.
    #[serde(default = "default_denial_patterns")]
    pub denial_patterns: Vec<String>,

    /// Case-insensitive patterns that indicate boilerplate/system markup.
    /// Content containing any of these patterns will be rejected.
    #[serde(default = "default_boilerplate_patterns")]
    pub boilerplate_patterns: Vec<String>,
}

impl Default for NoiseFilterConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            min_content_length: default_min_content_length(),
            denial_patterns: default_denial_patterns(),
            boilerplate_patterns: default_boilerplate_patterns(),
        }
    }
}

// ---------------------------------------------------------------------------
// NoiseFilter
// ---------------------------------------------------------------------------

/// Dual-defense noise filter that operates at both storage and retrieval time.
///
/// At **storage time**, `should_store()` prevents low-quality content from
/// entering the memory store. At **retrieval time**, `filter_results()` removes
/// noisy entries from search results (e.g., facts stored before the filter was
/// enabled or with relaxed settings).
#[derive(Clone)]
pub struct NoiseFilter {
    config: NoiseFilterConfig,
}

impl NoiseFilter {
    /// Create a new noise filter with the given configuration.
    pub fn new(config: NoiseFilterConfig) -> Self {
        Self { config }
    }

    /// Check whether the given content should be stored in memory.
    ///
    /// Returns `true` if the content passes all quality checks, `false` if it
    /// should be rejected as noise.
    ///
    /// Checks (in order):
    /// 1. If filter is disabled, always returns `true`.
    /// 2. Trimmed content must be at least `min_content_length` characters.
    /// 3. Content must contain at least one alphanumeric character.
    /// 4. Content must not contain any denial pattern (case-insensitive).
    /// 5. Content must not contain any boilerplate pattern (case-insensitive).
    pub fn should_store(&self, content: &str) -> bool {
        if !self.config.enabled {
            return true;
        }

        let trimmed = content.trim();

        // Too short
        if trimmed.len() < self.config.min_content_length {
            return false;
        }

        // No alphanumeric characters (pure emoji/punctuation)
        if !trimmed.chars().any(|c| c.is_alphanumeric()) {
            return false;
        }

        let lower = trimmed.to_lowercase();

        // Contains denial pattern
        for pattern in &self.config.denial_patterns {
            if lower.contains(&pattern.to_lowercase()) {
                return false;
            }
        }

        // Contains boilerplate pattern
        for pattern in &self.config.boilerplate_patterns {
            if lower.contains(&pattern.to_lowercase()) {
                return false;
            }
        }

        true
    }

    /// Filter retrieval results, removing entries that would not pass storage-time checks.
    ///
    /// This provides a second line of defense for facts that may have entered the
    /// store before the filter was enabled or with different settings.
    pub fn filter_results(&self, results: Vec<ScoredFact>) -> Vec<ScoredFact> {
        if !self.config.enabled {
            return results;
        }

        results
            .into_iter()
            .filter(|r| self.should_store(&r.fact.content))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{
        FactSource, FactType, MemoryFact,
    };

    /// Helper to create a default filter for testing.
    fn test_filter() -> NoiseFilter {
        NoiseFilter::new(NoiseFilterConfig::default())
    }

    /// Helper to create a ScoredFact with the given content.
    fn scored_fact(content: &str) -> ScoredFact {
        let mut fact = MemoryFact::new(
            content.to_string(),
            FactType::Other,
            vec![],
        );
        fact.confidence = 0.9;
        fact.fact_source = FactSource::Extracted;

        ScoredFact {
            fact,
            score: 0.85,
        }
    }

    #[test]
    fn normal_content_passes() {
        let filter = test_filter();
        assert!(filter.should_store("The user prefers dark mode for all applications."));
        assert!(filter.should_store("Rust is the primary language for this project."));
        assert!(filter.should_store("Meeting scheduled for Friday at 3pm."));
    }

    #[test]
    fn short_content_rejected() {
        let filter = test_filter();
        assert!(!filter.should_store("hi"));
        assert!(!filter.should_store("ok"));
        assert!(!filter.should_store("yes"));
        assert!(!filter.should_store("   short   ")); // "short" is 5 chars after trim
        // Exactly at the boundary
        assert!(filter.should_store("0123456789")); // 10 chars = passes
        assert!(!filter.should_store("012345678")); // 9 chars = rejected
    }

    #[test]
    fn pure_emoji_rejected() {
        let filter = test_filter();
        assert!(!filter.should_store("😀😀😀😀😀😀😀😀😀😀"));
        assert!(!filter.should_store("!@#$%^&*()!@#$%^&*()"));
        assert!(!filter.should_store("... --- ... --- ..."));
        // But emoji + text passes
        assert!(filter.should_store("Hello world! 😀"));
    }

    #[test]
    fn agent_denial_rejected() {
        let filter = test_filter();
        assert!(!filter.should_store("I can't help with that request."));
        assert!(!filter.should_store("I'm sorry, but I cannot do that."));
        assert!(!filter.should_store("I cannot assist with this task."));
        assert!(!filter.should_store("As an AI, I don't have personal opinions."));
        assert!(!filter.should_store("I don't have the ability to browse the internet."));
        // Case insensitive
        assert!(!filter.should_store("AS AN AI, I have limitations."));
        assert!(!filter.should_store("I CAN'T HELP WITH that."));
    }

    #[test]
    fn system_tags_rejected() {
        let filter = test_filter();
        assert!(!filter.should_store("<system>You are a helpful assistant.</system>"));
        assert!(!filter.should_store("Here are <relevant-memories> from the past."));
        assert!(!filter.should_store("End of </relevant-memories> block."));
        // Case insensitive
        assert!(!filter.should_store("<SYSTEM>Configuration block</SYSTEM>"));
    }

    #[test]
    fn filter_results_removes_bad_entries() {
        let filter = test_filter();

        let results = vec![
            scored_fact("The user prefers dark mode."),
            scored_fact("hi"),                                     // too short
            scored_fact("I can't help with that."),                // denial
            scored_fact("Rust is the primary language."),
            scored_fact("<system>Internal prompt</system>"),       // boilerplate
            scored_fact("😀😀😀😀😀😀😀😀😀😀"),                   // pure emoji
        ];

        let filtered = filter.filter_results(results);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].fact.content, "The user prefers dark mode.");
        assert_eq!(filtered[1].fact.content, "Rust is the primary language.");
    }

    #[test]
    fn disabled_filter_passes_everything() {
        let config = NoiseFilterConfig {
            enabled: false,
            ..Default::default()
        };
        let filter = NoiseFilter::new(config);

        // All of these would normally be rejected
        assert!(filter.should_store("hi"));
        assert!(filter.should_store("I can't help with that."));
        assert!(filter.should_store("<system>prompt</system>"));
        assert!(filter.should_store("😀😀😀😀😀😀😀😀😀😀"));

        // filter_results also passes everything through
        let results = vec![
            scored_fact("hi"),
            scored_fact("I can't help with that."),
        ];
        let filtered = filter.filter_results(results);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn custom_config_respects_settings() {
        let config = NoiseFilterConfig {
            enabled: true,
            min_content_length: 5,
            denial_patterns: vec!["no way".to_string()],
            boilerplate_patterns: vec!["[INTERNAL]".to_string()],
        };
        let filter = NoiseFilter::new(config);

        // Shorter threshold
        assert!(filter.should_store("hello")); // 5 chars = passes with min 5
        assert!(!filter.should_store("hey"));  // 3 chars = rejected

        // Custom denial pattern
        assert!(!filter.should_store("No way I can do that."));
        assert!(filter.should_store("I can't help with that.")); // default denial not in custom config

        // Custom boilerplate
        assert!(!filter.should_store("[INTERNAL] Debug data here"));
        assert!(filter.should_store("<system>This is fine now</system>")); // default boilerplate not in custom
    }

    #[test]
    fn whitespace_only_content_rejected() {
        let filter = test_filter();
        assert!(!filter.should_store("          "));
        assert!(!filter.should_store("\n\n\n\n\n\n\n\n\n\n"));
        assert!(!filter.should_store("\t\t\t\t\t\t\t\t\t\t"));
    }

    #[test]
    fn default_config_values_correct() {
        let config = NoiseFilterConfig::default();
        assert!(config.enabled);
        assert_eq!(config.min_content_length, 10);
        assert_eq!(config.denial_patterns.len(), 5);
        assert_eq!(config.boilerplate_patterns.len(), 4);
        assert!(config.denial_patterns.contains(&"as an ai".to_string()));
        assert!(config.boilerplate_patterns.contains(&"<system>".to_string()));
    }
}
