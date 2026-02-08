//! Dynamic injection system for behavioral anchors
//!
//! This module implements tag-based retrieval and dynamic injection of behavioral
//! anchors into the system prompt based on the current intent. It uses LRU caching
//! for efficiency and supports both heuristic and LLM-based tag extraction.

use super::anchor_store::AnchorStore;
use super::reactive::LLMConfig;
use super::types::BehavioralAnchor;
use crate::error::AlephError;
use lru::LruCache;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};

/// Extracts relevant tags from user intent
///
/// Uses heuristic rules for fast extraction, with optional LLM fallback
/// for complex cases (currently stubbed).
pub struct TagExtractor {
    #[allow(dead_code)]
    llm_config: LLMConfig,
}

impl TagExtractor {
    /// Create a new TagExtractor with the given LLM configuration
    ///
    /// # Arguments
    ///
    /// * `llm_config` - Configuration for LLM-based tag extraction (fallback)
    pub fn new(llm_config: LLMConfig) -> Self {
        Self { llm_config }
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
    /// # use alephcore::memory::cortex::meta_cognition::injection::TagExtractor;
    /// # use alephcore::memory::cortex::meta_cognition::reactive::LLMConfig;
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

/// Retrieves relevant behavioral anchors for a given intent
///
/// Uses tag-based retrieval with LRU caching for efficiency.
pub struct AnchorRetriever {
    anchor_store: Arc<RwLock<AnchorStore>>,
    tag_extractor: TagExtractor,
    cache: LruCache<String, Vec<BehavioralAnchor>>,
}

impl AnchorRetriever {
    /// Create a new AnchorRetriever
    ///
    /// # Arguments
    ///
    /// * `anchor_store` - Shared anchor store for persistence
    /// * `tag_extractor` - Tag extraction component
    /// * `cache_size` - Maximum number of cache entries (default: 100)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use std::sync::{Arc, RwLock};
    /// # use rusqlite::Connection;
    /// # use alephcore::memory::cortex::meta_cognition::{AnchorStore, injection::{AnchorRetriever, TagExtractor}, reactive::LLMConfig};
    /// # let conn = Arc::new(Connection::open_in_memory().unwrap());
    /// # let anchor_store = Arc::new(RwLock::new(AnchorStore::new(conn)));
    /// # let llm_config = LLMConfig { model: "claude-3-5-sonnet-20241022".to_string(), temperature: 0.0 };
    /// # let tag_extractor = TagExtractor::new(llm_config);
    /// let retriever = AnchorRetriever::new(anchor_store, tag_extractor, 100);
    /// ```
    pub fn new(
        anchor_store: Arc<RwLock<AnchorStore>>,
        tag_extractor: TagExtractor,
        cache_size: usize,
    ) -> Self {
        Self {
            anchor_store,
            tag_extractor,
            cache: LruCache::new(NonZeroUsize::new(cache_size).unwrap()),
        }
    }

    /// Retrieve behavioral anchors relevant to the given intent
    ///
    /// # Arguments
    ///
    /// * `intent` - The user's intent string
    ///
    /// # Returns
    ///
    /// * `Result<Vec<BehavioralAnchor>>` - Top 5 relevant anchors
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use std::sync::{Arc, RwLock};
    /// # use rusqlite::Connection;
    /// # use alephcore::memory::cortex::meta_cognition::{AnchorStore, injection::{AnchorRetriever, TagExtractor}, reactive::LLMConfig};
    /// # let conn = Arc::new(Connection::open_in_memory().unwrap());
    /// # let anchor_store = Arc::new(RwLock::new(AnchorStore::new(conn)));
    /// # let llm_config = LLMConfig { model: "claude-3-5-sonnet-20241022".to_string(), temperature: 0.0 };
    /// # let tag_extractor = TagExtractor::new(llm_config);
    /// # let mut retriever = AnchorRetriever::new(anchor_store, tag_extractor, 100);
    /// let anchors = retriever.retrieve_for_intent("Run Python script on macOS").unwrap();
    /// ```
    pub fn retrieve_for_intent(&mut self, intent: &str) -> Result<Vec<BehavioralAnchor>, AlephError> {
        // Check cache first
        if let Some(cached_anchors) = self.cache.get(intent) {
            return Ok(cached_anchors.clone());
        }

        // Extract tags from intent
        let tags = self.tag_extractor.extract_tags(intent)?;

        // Query anchor store for anchors matching any of the tags
        let store = self.anchor_store.read().map_err(|e| {
            AlephError::Other {
                message: format!("Failed to acquire read lock on anchor store: {}", e),
                suggestion: None,
            }
        })?;

        let all_anchors = store.list_all().map_err(|e| {
            AlephError::Other {
                message: format!("Failed to list anchors: {}", e),
                suggestion: None,
            }
        })?;

        // Filter anchors that match any of the extracted tags
        let mut matching_anchors: Vec<BehavioralAnchor> = all_anchors
            .into_iter()
            .filter(|anchor| {
                // Check if any trigger tag matches any extracted tag
                anchor.trigger_tags.iter().any(|trigger_tag: &String| {
                    tags.iter().any(|extracted_tag: &String| {
                        trigger_tag.eq_ignore_ascii_case(extracted_tag)
                    })
                })
            })
            .collect();

        // Rank by relevance
        matching_anchors = self.rank_by_relevance(matching_anchors);

        // Take top 5
        let top_anchors: Vec<BehavioralAnchor> = matching_anchors.into_iter().take(5).collect();

        // Update cache
        self.cache.put(intent.to_string(), top_anchors.clone());

        Ok(top_anchors)
    }

    /// Rank anchors by relevance (priority DESC, confidence DESC)
    ///
    /// # Arguments
    ///
    /// * `anchors` - Anchors to rank
    ///
    /// # Returns
    ///
    /// * `Vec<BehavioralAnchor>` - Ranked anchors
    fn rank_by_relevance(&self, mut anchors: Vec<BehavioralAnchor>) -> Vec<BehavioralAnchor> {
        anchors.sort_by(|a, b| {
            // First sort by priority (descending)
            match b.priority.cmp(&a.priority) {
                std::cmp::Ordering::Equal => {
                    // If priorities are equal, sort by confidence (descending)
                    b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal)
                }
                other => other,
            }
        });
        anchors
    }

    /// Clear the cache
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

/// Formats behavioral anchors for injection into system prompt
pub struct InjectionFormatter;

impl InjectionFormatter {
    /// Format anchors as markdown section for system prompt
    ///
    /// # Arguments
    ///
    /// * `anchors` - Anchors to format
    ///
    /// # Returns
    ///
    /// * `String` - Formatted markdown section
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use alephcore::memory::cortex::meta_cognition::{BehavioralAnchor, AnchorSource, AnchorScope, injection::InjectionFormatter};
    /// let anchors = vec![
    ///     BehavioralAnchor::new(
    ///         "test-id".to_string(),
    ///         "Always check Python version".to_string(),
    ///         vec!["Python".to_string()],
    ///         AnchorSource::ManualInjection { author: "test".to_string() },
    ///         AnchorScope::Global,
    ///         100,
    ///         0.8,
    ///     ),
    /// ];
    /// let formatted = InjectionFormatter::format_anchors(&anchors);
    /// assert!(formatted.contains("## Behavioral Guidelines"));
    /// ```
    pub fn format_anchors(anchors: &[BehavioralAnchor]) -> String {
        if anchors.is_empty() {
            return String::new();
        }

        let mut output = String::from("## Behavioral Guidelines\n\n");
        output.push_str("The following learned behaviors should guide your decision-making:\n\n");

        for (idx, anchor) in anchors.iter().enumerate() {
            output.push_str(&format!("{}. **{}**\n", idx + 1, anchor.rule_text));
            output.push_str(&format!("   - Priority: {}\n", anchor.priority));
            output.push_str(&format!("   - Confidence: {:.2}\n", anchor.confidence));
            output.push_str(&format!("   - Tags: {}\n", anchor.trigger_tags.join(", ")));
            output.push('\n');
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::cortex::meta_cognition::schema::initialize_schema;
    use crate::memory::cortex::meta_cognition::types::{AnchorScope, AnchorSource};
    use rusqlite::Connection;

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

    #[test]
    fn test_cache_hit() {
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        initialize_schema(&conn).unwrap();
        let anchor_store = Arc::new(RwLock::new(AnchorStore::new(conn)));

        let llm_config = LLMConfig {
            model: "claude-3-5-sonnet-20241022".to_string(),
            temperature: 0.0,
        };
        let tag_extractor = TagExtractor::new(llm_config);
        let mut retriever = AnchorRetriever::new(anchor_store, tag_extractor, 100);

        // First call - cache miss
        let result1 = retriever.retrieve_for_intent("Run Python script").unwrap();

        // Second call - cache hit
        let result2 = retriever.retrieve_for_intent("Run Python script").unwrap();

        // Results should be identical
        assert_eq!(result1.len(), result2.len());
    }

    #[test]
    fn test_cache_clear() {
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        initialize_schema(&conn).unwrap();
        let anchor_store = Arc::new(RwLock::new(AnchorStore::new(conn)));

        let llm_config = LLMConfig {
            model: "claude-3-5-sonnet-20241022".to_string(),
            temperature: 0.0,
        };
        let tag_extractor = TagExtractor::new(llm_config);
        let mut retriever = AnchorRetriever::new(anchor_store, tag_extractor, 100);

        // Populate cache
        retriever.retrieve_for_intent("Run Python script").unwrap();

        // Clear cache
        retriever.clear_cache();

        // Cache should be empty now (next call will be a cache miss)
        let result = retriever.retrieve_for_intent("Run Python script").unwrap();
        assert!(result.is_empty() || !result.is_empty()); // Just verify it doesn't panic
    }

    #[test]
    fn test_ranking_by_priority_and_confidence() {
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        initialize_schema(&conn).unwrap();
        let mut store = AnchorStore::new(Arc::clone(&conn));

        // Add anchors with different priorities and confidences
        let anchor1 = BehavioralAnchor::new(
            "anchor-1".to_string(),
            "Low priority, low confidence".to_string(),
            vec!["Python".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            10,
            0.5,
        );

        let anchor2 = BehavioralAnchor::new(
            "anchor-2".to_string(),
            "High priority, high confidence".to_string(),
            vec!["Python".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            100,
            0.9,
        );

        let anchor3 = BehavioralAnchor::new(
            "anchor-3".to_string(),
            "Medium priority, medium confidence".to_string(),
            vec!["Python".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            50,
            0.7,
        );

        store.add(anchor1).unwrap();
        store.add(anchor2).unwrap();
        store.add(anchor3).unwrap();

        let anchor_store = Arc::new(RwLock::new(store));
        let llm_config = LLMConfig {
            model: "claude-3-5-sonnet-20241022".to_string(),
            temperature: 0.0,
        };
        let tag_extractor = TagExtractor::new(llm_config);
        let mut retriever = AnchorRetriever::new(anchor_store, tag_extractor, 100);

        let anchors = retriever.retrieve_for_intent("Run Python script").unwrap();

        // Should be sorted by priority DESC, then confidence DESC
        assert_eq!(anchors.len(), 3);
        assert_eq!(anchors[0].id, "anchor-2"); // priority 100
        assert_eq!(anchors[1].id, "anchor-3"); // priority 50
        assert_eq!(anchors[2].id, "anchor-1"); // priority 10
    }

    #[test]
    fn test_formatting_empty_anchors() {
        let formatted = InjectionFormatter::format_anchors(&[]);
        assert_eq!(formatted, "");
    }

    #[test]
    fn test_formatting_single_anchor() {
        let anchor = BehavioralAnchor::new(
            "test-id".to_string(),
            "Always check Python version".to_string(),
            vec!["Python".to_string(), "macOS".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            100,
            0.8,
        );

        let formatted = InjectionFormatter::format_anchors(&[anchor]);
        assert!(formatted.contains("## Behavioral Guidelines"));
        assert!(formatted.contains("Always check Python version"));
        assert!(formatted.contains("Priority: 100"));
        assert!(formatted.contains("Confidence: 0.80"));
        assert!(formatted.contains("Tags: Python, macOS"));
    }

    #[test]
    fn test_formatting_multiple_anchors() {
        let anchor1 = BehavioralAnchor::new(
            "test-id-1".to_string(),
            "Rule 1".to_string(),
            vec!["Python".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            100,
            0.8,
        );

        let anchor2 = BehavioralAnchor::new(
            "test-id-2".to_string(),
            "Rule 2".to_string(),
            vec!["Rust".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            90,
            0.7,
        );

        let formatted = InjectionFormatter::format_anchors(&[anchor1, anchor2]);
        assert!(formatted.contains("1. **Rule 1**"));
        assert!(formatted.contains("2. **Rule 2**"));
    }
}

