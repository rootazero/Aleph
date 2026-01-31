//! Layered Retrieval Strategies
//!
//! Defines strategies for searching facts vs memories in the dual-layer
//! memory architecture.
//!
//! ## Strategies
//!
//! - **FactsOnly**: Only search Layer 2 (compressed facts). Fastest mode.
//! - **FactsFirst**: Search facts first, then memories if not enough results. Default mode.
//! - **BothLayers**: Search both layers simultaneously and merge results. Most thorough.

use serde::{Deserialize, Serialize};

/// Layered retrieval strategy
///
/// Determines how the retrieval engine searches across the dual-layer
/// memory architecture (Layer 1: raw memories, Layer 2: compressed facts).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RetrievalStrategy {
    /// Only search Layer 2 (facts) - fast mode
    ///
    /// Use this when you need quick retrieval and facts are sufficient.
    /// This is the most efficient strategy but may miss relevant context
    /// from raw memories.
    FactsOnly,

    /// Search facts first, then memories if not enough - default mode
    ///
    /// This is the recommended strategy for most use cases. It prioritizes
    /// compressed facts (which are more semantic) and falls back to raw
    /// memories when the fact pool is insufficient.
    FactsFirst {
        /// Minimum number of facts required before searching memories
        ///
        /// If fewer than `min_facts` are found, the retriever will also
        /// search raw memories to supplement the results.
        min_facts: usize,
    },

    /// Search both layers simultaneously, merge results - deep mode
    ///
    /// Use this for thorough retrieval when you need maximum recall.
    /// Results from both layers are merged and re-ranked by combined score.
    BothLayers,
}

impl Default for RetrievalStrategy {
    fn default() -> Self {
        Self::FactsFirst { min_facts: 3 }
    }
}

impl RetrievalStrategy {
    /// Create FactsFirst strategy with default min_facts (3)
    pub fn facts_first() -> Self {
        Self::FactsFirst { min_facts: 3 }
    }

    /// Create FactsFirst strategy with custom min_facts
    pub fn facts_first_with_min(min_facts: usize) -> Self {
        Self::FactsFirst { min_facts }
    }

    /// Check if this strategy searches facts
    pub fn searches_facts(&self) -> bool {
        // All strategies search facts
        true
    }

    /// Check if this strategy may search raw memories
    pub fn may_search_memories(&self) -> bool {
        match self {
            Self::FactsOnly => false,
            Self::FactsFirst { .. } => true,
            Self::BothLayers => true,
        }
    }

    /// Check if this is the FactsOnly strategy
    pub fn is_facts_only(&self) -> bool {
        matches!(self, Self::FactsOnly)
    }

    /// Check if this is the BothLayers strategy
    pub fn is_both_layers(&self) -> bool {
        matches!(self, Self::BothLayers)
    }

    /// Get the min_facts value if this is a FactsFirst strategy
    pub fn min_facts(&self) -> Option<usize> {
        match self {
            Self::FactsFirst { min_facts } => Some(*min_facts),
            _ => None,
        }
    }

    /// Determine if memory search should be performed based on fact count
    ///
    /// # Arguments
    /// * `fact_count` - Number of facts already retrieved
    ///
    /// # Returns
    /// `true` if raw memory search should be performed
    pub fn should_search_memories(&self, fact_count: usize) -> bool {
        match self {
            Self::FactsOnly => false,
            Self::FactsFirst { min_facts } => fact_count < *min_facts,
            Self::BothLayers => true,
        }
    }
}

impl std::fmt::Display for RetrievalStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FactsOnly => write!(f, "FactsOnly"),
            Self::FactsFirst { min_facts } => write!(f, "FactsFirst(min_facts={})", min_facts),
            Self::BothLayers => write!(f, "BothLayers"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_strategy() {
        let strategy = RetrievalStrategy::default();
        assert!(matches!(
            strategy,
            RetrievalStrategy::FactsFirst { min_facts: 3 }
        ));
    }

    #[test]
    fn test_facts_first_default() {
        let strategy = RetrievalStrategy::facts_first();
        if let RetrievalStrategy::FactsFirst { min_facts } = strategy {
            assert_eq!(min_facts, 3);
        } else {
            panic!("Expected FactsFirst");
        }
    }

    #[test]
    fn test_facts_first_custom() {
        let strategy = RetrievalStrategy::facts_first_with_min(5);
        if let RetrievalStrategy::FactsFirst { min_facts } = strategy {
            assert_eq!(min_facts, 5);
        } else {
            panic!("Expected FactsFirst");
        }
    }

    #[test]
    fn test_searches_facts() {
        assert!(RetrievalStrategy::FactsOnly.searches_facts());
        assert!(RetrievalStrategy::facts_first().searches_facts());
        assert!(RetrievalStrategy::BothLayers.searches_facts());
    }

    #[test]
    fn test_may_search_memories() {
        assert!(!RetrievalStrategy::FactsOnly.may_search_memories());
        assert!(RetrievalStrategy::facts_first().may_search_memories());
        assert!(RetrievalStrategy::BothLayers.may_search_memories());
    }

    #[test]
    fn test_is_facts_only() {
        assert!(RetrievalStrategy::FactsOnly.is_facts_only());
        assert!(!RetrievalStrategy::facts_first().is_facts_only());
        assert!(!RetrievalStrategy::BothLayers.is_facts_only());
    }

    #[test]
    fn test_is_both_layers() {
        assert!(!RetrievalStrategy::FactsOnly.is_both_layers());
        assert!(!RetrievalStrategy::facts_first().is_both_layers());
        assert!(RetrievalStrategy::BothLayers.is_both_layers());
    }

    #[test]
    fn test_min_facts() {
        assert_eq!(RetrievalStrategy::FactsOnly.min_facts(), None);
        assert_eq!(RetrievalStrategy::facts_first().min_facts(), Some(3));
        assert_eq!(
            RetrievalStrategy::facts_first_with_min(7).min_facts(),
            Some(7)
        );
        assert_eq!(RetrievalStrategy::BothLayers.min_facts(), None);
    }

    #[test]
    fn test_should_search_memories_facts_only() {
        let strategy = RetrievalStrategy::FactsOnly;
        assert!(!strategy.should_search_memories(0));
        assert!(!strategy.should_search_memories(5));
        assert!(!strategy.should_search_memories(100));
    }

    #[test]
    fn test_should_search_memories_facts_first() {
        let strategy = RetrievalStrategy::facts_first_with_min(3);
        assert!(strategy.should_search_memories(0));
        assert!(strategy.should_search_memories(1));
        assert!(strategy.should_search_memories(2));
        assert!(!strategy.should_search_memories(3));
        assert!(!strategy.should_search_memories(5));
    }

    #[test]
    fn test_should_search_memories_both_layers() {
        let strategy = RetrievalStrategy::BothLayers;
        assert!(strategy.should_search_memories(0));
        assert!(strategy.should_search_memories(5));
        assert!(strategy.should_search_memories(100));
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", RetrievalStrategy::FactsOnly), "FactsOnly");
        assert_eq!(
            format!("{}", RetrievalStrategy::facts_first()),
            "FactsFirst(min_facts=3)"
        );
        assert_eq!(format!("{}", RetrievalStrategy::BothLayers), "BothLayers");
    }

    #[test]
    fn test_equality() {
        assert_eq!(RetrievalStrategy::FactsOnly, RetrievalStrategy::FactsOnly);
        assert_eq!(
            RetrievalStrategy::facts_first(),
            RetrievalStrategy::facts_first()
        );
        assert_eq!(RetrievalStrategy::BothLayers, RetrievalStrategy::BothLayers);
        assert_ne!(RetrievalStrategy::FactsOnly, RetrievalStrategy::BothLayers);
    }

    #[test]
    fn test_serialization() {
        let strategy = RetrievalStrategy::facts_first_with_min(5);
        let json = serde_json::to_string(&strategy).unwrap();
        let deserialized: RetrievalStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(strategy, deserialized);
    }
}
