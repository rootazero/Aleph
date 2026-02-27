//! Tag extraction from user intent for anchor retrieval
//!
//! Uses heuristic rules for fast extraction, with optional LLM fallback
//! for complex cases (currently stubbed).
//! Extracted from core/src/memory/cortex/meta_cognition/injection.rs.

use super::reactive::LLMConfig;
use crate::error::AlephError;
use std::collections::HashSet;

/// Extracts relevant tags from user intent
///
/// Uses heuristic rules for fast extraction, with optional LLM fallback
/// for complex cases (currently stubbed).
pub struct TagExtractor {
    _llm_config: LLMConfig,
}

impl TagExtractor {
    /// Create a new TagExtractor with the given LLM configuration
    ///
    /// # Arguments
    ///
    /// * `llm_config` - Configuration for LLM-based tag extraction (fallback)
    pub fn new(llm_config: LLMConfig) -> Self {
        Self { _llm_config: llm_config }
    }

    /// Extract tags from user intent using heuristic rules
    ///
    /// # Arguments
    ///
    /// * `intent` - The user's intent string
    ///
    /// # Returns
    ///
    /// * `Result<Vec<String>>` - Extracted tags
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use alephcore::poe::meta_cognition::tag_extractor::TagExtractor;
    /// # use alephcore::poe::meta_cognition::reactive::LLMConfig;
    /// # let llm_config = LLMConfig { model: "claude-3-5-sonnet-20241022".to_string(), temperature: 0.0 };
    /// let extractor = TagExtractor::new(llm_config);
    /// let tags = extractor.extract_tags("Run Python script on macOS").unwrap();
    /// assert!(tags.contains(&"Python".to_string()));
    /// assert!(tags.contains(&"macOS".to_string()));
    /// ```
    pub fn extract_tags(&self, intent: &str) -> Result<Vec<String>, AlephError> {
        let mut tags = HashSet::new();
        let intent_lower = intent.to_lowercase();

        // Programming languages
        if intent_lower.contains("python") {
            tags.insert("Python".to_string());
        }
        if intent_lower.contains("rust") {
            tags.insert("Rust".to_string());
        }
        if intent_lower.contains("javascript") || intent_lower.contains("js") {
            tags.insert("JavaScript".to_string());
        }
        if intent_lower.contains("typescript") || intent_lower.contains("ts") {
            tags.insert("TypeScript".to_string());
        }
        if intent_lower.contains("go") || intent_lower.contains("golang") {
            tags.insert("Go".to_string());
        }
        if intent_lower.contains("java") {
            tags.insert("Java".to_string());
        }
        if intent_lower.contains("c++") || intent_lower.contains("cpp") {
            tags.insert("C++".to_string());
        }

        // Operating systems
        if intent_lower.contains("macos") || intent_lower.contains("mac os") {
            tags.insert("macOS".to_string());
        }
        if intent_lower.contains("linux") {
            tags.insert("Linux".to_string());
        }
        if intent_lower.contains("windows") {
            tags.insert("Windows".to_string());
        }

        // Tools and technologies
        if intent_lower.contains("shell") || intent_lower.contains("bash") || intent_lower.contains("zsh") {
            tags.insert("shell".to_string());
        }
        if intent_lower.contains("git") {
            tags.insert("git".to_string());
        }
        if intent_lower.contains("docker") {
            tags.insert("Docker".to_string());
        }
        if intent_lower.contains("kubernetes") || intent_lower.contains("k8s") {
            tags.insert("Kubernetes".to_string());
        }
        if intent_lower.contains("database") || intent_lower.contains("sql") {
            tags.insert("database".to_string());
        }
        if intent_lower.contains("api") || intent_lower.contains("rest") || intent_lower.contains("http") {
            tags.insert("API".to_string());
        }

        // Task types
        if intent_lower.contains("test") || intent_lower.contains("testing") {
            tags.insert("testing".to_string());
        }
        if intent_lower.contains("debug") || intent_lower.contains("debugging") {
            tags.insert("debugging".to_string());
        }
        if intent_lower.contains("deploy") || intent_lower.contains("deployment") {
            tags.insert("deployment".to_string());
        }
        if intent_lower.contains("refactor") || intent_lower.contains("refactoring") {
            tags.insert("refactoring".to_string());
        }

        // If no tags found, use LLM fallback (stubbed for now)
        if tags.is_empty() {
            // TODO: Implement LLM-based tag extraction
            // For now, return a generic tag
            tags.insert("general".to_string());
        }

        Ok(tags.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tag_extraction_programming_languages() {
        let llm_config = LLMConfig {
            model: "claude-3-5-sonnet-20241022".to_string(),
            temperature: 0.0,
        };
        let extractor = TagExtractor::new(llm_config);

        let tags = extractor.extract_tags("Run Python script").unwrap();
        assert!(tags.contains(&"Python".to_string()));

        let tags = extractor.extract_tags("Compile Rust code").unwrap();
        assert!(tags.contains(&"Rust".to_string()));

        let tags = extractor.extract_tags("Debug JavaScript application").unwrap();
        assert!(tags.contains(&"JavaScript".to_string()));
    }

    #[test]
    fn test_tag_extraction_operating_systems() {
        let llm_config = LLMConfig {
            model: "claude-3-5-sonnet-20241022".to_string(),
            temperature: 0.0,
        };
        let extractor = TagExtractor::new(llm_config);

        let tags = extractor.extract_tags("Install package on macOS").unwrap();
        assert!(tags.contains(&"macOS".to_string()));

        let tags = extractor.extract_tags("Configure Linux server").unwrap();
        assert!(tags.contains(&"Linux".to_string()));
    }

    #[test]
    fn test_tag_extraction_tools() {
        let llm_config = LLMConfig {
            model: "claude-3-5-sonnet-20241022".to_string(),
            temperature: 0.0,
        };
        let extractor = TagExtractor::new(llm_config);

        let tags = extractor.extract_tags("Run shell command").unwrap();
        assert!(tags.contains(&"shell".to_string()));

        let tags = extractor.extract_tags("Commit changes with git").unwrap();
        assert!(tags.contains(&"git".to_string()));
    }

    #[test]
    fn test_tag_extraction_fallback() {
        let llm_config = LLMConfig {
            model: "claude-3-5-sonnet-20241022".to_string(),
            temperature: 0.0,
        };
        let extractor = TagExtractor::new(llm_config);

        let tags = extractor.extract_tags("Do something random").unwrap();
        assert!(tags.contains(&"general".to_string()));
    }
}
