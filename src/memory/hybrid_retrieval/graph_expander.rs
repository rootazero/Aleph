//! Graph-augmented retrieval expansion.
//!
//! Previously expanded retrieval results by traversing the knowledge graph
//! (graph_nodes/graph_edges/memory_entities). That graph system has been
//! replaced by Knowledge Notes with native wikilink-based linking.
//!
//! This module is retained as a no-op stub so that existing call sites
//! compile. Link-based expansion will be reimplemented via notes_links.

use crate::error::AlephError;
use crate::memory::store::types::ScoredFact;
use crate::memory::store::MemoryBackend;

/// Configuration for graph expansion.
#[derive(Debug, Clone)]
pub struct GraphExpansionConfig {
    pub enabled: bool,
    pub max_hops: usize,
    pub max_expanded_per_seed: usize,
    pub max_total_expanded: usize,
    pub min_weight: f32,
    pub hop_decay: f32,
}

impl Default for GraphExpansionConfig {
    fn default() -> Self {
        Self {
            enabled: false, // disabled — graph tables are deprecated
            max_hops: 1,
            max_expanded_per_seed: 3,
            max_total_expanded: 10,
            min_weight: 0.3,
            hop_decay: 0.7,
        }
    }
}

/// A fact discovered through graph expansion.
#[derive(Debug, Clone)]
pub struct ExpandedFact {
    pub scored_fact: ScoredFact,
    pub seed_fact_id: String,
    pub expansion_path: String,
}

/// No-op graph expander (deprecated — Knowledge Notes use wikilink-based linking).
pub struct GraphExpander {
    _database: MemoryBackend,
    config: GraphExpansionConfig,
}

impl GraphExpander {
    pub fn new(database: MemoryBackend, config: GraphExpansionConfig) -> Self {
        Self {
            _database: database,
            config,
        }
    }

    /// Always returns empty — graph expansion is deprecated.
    pub async fn expand(
        &self,
        _seeds: &[ScoredFact],
        _workspace: &str,
    ) -> Result<Vec<ExpandedFact>, AlephError> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }
        // Graph tables are deprecated; return empty
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        let config = GraphExpansionConfig::default();
        assert!(!config.enabled);
    }
}
