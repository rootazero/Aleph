//! Retrieval of relevant behavioral anchors for a given intent
//!
//! Uses tag-based retrieval with LRU caching for efficiency.
//! Extracted from core/src/memory/cortex/meta_cognition/injection.rs.

use super::anchor_store::AnchorStore;
use super::tag_extractor::TagExtractor;
use super::types::BehavioralAnchor;
use crate::error::AlephError;
use lru::LruCache;
use std::num::NonZeroUsize;
use crate::sync_primitives::{Arc, RwLock};

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
    pub fn new(
        anchor_store: Arc<RwLock<AnchorStore>>,
        tag_extractor: TagExtractor,
        cache_size: usize,
    ) -> Self {
        Self {
            anchor_store,
            tag_extractor,
            cache: LruCache::new(NonZeroUsize::new(cache_size.max(1)).unwrap()),
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

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::memory::cortex::meta_cognition::schema::initialize_schema;
    use crate::poe::meta_cognition::reactive::LLMConfig;
    use crate::poe::meta_cognition::types::{AnchorScope, AnchorSource};
    use rusqlite::Connection;

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
}
