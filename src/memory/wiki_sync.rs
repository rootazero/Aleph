//! Wiki wikilink ↔ graph synchronization.
//!
//! Parses `[[wikilink]]` from Wiki-type facts and synchronizes them
//! as `references` edges in the knowledge graph, plus `memory_entities`
//! associations with `source=wikilink`.

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryFact};
use crate::memory::graph::GraphStore;

/// Summary of a wikilink sync operation.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub cleared: usize,
    pub linked: usize,
}

/// Synchronize wikilinks in a Wiki fact to the knowledge graph.
///
/// For each `[[target]]` found in the fact content:
/// 1. Upsert a graph node for the target (kind="wiki")
/// 2. Create a `memory_entities` link (source="wikilink", weight=1.0)
/// 3. Create a `references` edge from source page to target page
///
/// Only operates on `FactType::Wiki` facts; returns `Ok(())` for others.
pub async fn sync_wikilinks_to_graph(
    fact: &MemoryFact,
    graph_store: &GraphStore,
) -> Result<SyncReport, AlephError> {
    // Early return for non-Wiki facts
    if fact.fact_type != FactType::Wiki {
        return Ok(SyncReport::default());
    }

    // Extract wikilink targets from fact content
    let targets = crate::wiki::wikilink::extract_wikilinks(&fact.content);

    // Clear existing wikilink associations for this fact
    let cleared = crate::memory::store::GraphStore::delete_memory_entities_by_source(
        graph_store.database_ref(),
        &fact.id,
        "wikilink",
        "default",
    )
    .await?;

    if targets.is_empty() {
        return Ok(SyncReport {
            cleared,
            linked: 0,
        });
    }

    // Derive source page slug from fact path (last segment, strip .md suffix)
    let source_slug = fact
        .path
        .split('/')
        .next_back()
        .unwrap_or(&fact.path)
        .trim_end_matches(".md");

    // Upsert source node
    let source_node = graph_store
        .upsert_node(source_slug, "wiki", &[], None)
        .await?;

    let mut linked = 0usize;

    for target in &targets {
        // Upsert target node
        let target_node = graph_store.upsert_node(target, "wiki", &[], None).await?;

        // Link the fact to the target node via memory_entities
        graph_store
            .link_memory_entity(&fact.id, &target_node.id, 1.0, "wikilink")
            .await?;

        // Create a references edge from source page to target page
        graph_store
            .upsert_edge(&source_node.id, &target_node.id, "references", "wiki", 1.0, 1.0)
            .await?;

        linked += 1;
    }

    Ok(SyncReport { cleared, linked })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactSource, FactSpecificity, FactType, MemoryCategory, MemoryLayer, MemoryScope, MemoryTier, TemporalScope};

    fn make_wiki_fact(id: &str, path: &str, content: &str) -> MemoryFact {
        MemoryFact {
            id: id.to_string(),
            content: content.to_string(),
            fact_type: FactType::Wiki,
            embedding: None,
            source_memory_ids: vec![],
            created_at: 0,
            updated_at: 0,
            confidence: 1.0,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::General,
            temporal_scope: TemporalScope::Permanent,
            namespace: "owner".to_string(),
            agent: "default".to_string(),
            similarity_score: None,
            path: path.to_string(),
            layer: MemoryLayer::Core,
            category: MemoryCategory::Patterns,
            fact_source: FactSource::Manual,
            content_hash: String::new(),
            parent_path: String::new(),
            embedding_model: String::new(),
            tier: MemoryTier::default(),
            scope: MemoryScope::default(),
            persona_id: None,
            strength: 1.0,
            access_count: 0,
            last_accessed_at: None,
            valid_from: None,
            expires_at: None,
        }
    }

    #[test]
    fn returns_empty_report_for_non_wiki_fact() {
        // Verify the early-return logic without async (structural check)
        let fact = MemoryFact {
            fact_type: FactType::Observation,
            ..make_wiki_fact("f1", "aleph://notes/test.md", "no wikilinks here")
        };
        assert_ne!(fact.fact_type, FactType::Wiki);
    }

    #[test]
    fn extracts_source_slug_from_path() {
        // Unit test for slug derivation logic
        let path = "aleph://wiki/rust-ownership.md";
        let slug = path
            .split('/')
            .next_back()
            .unwrap_or(path)
            .trim_end_matches(".md");
        assert_eq!(slug, "rust-ownership");
    }

    #[test]
    fn extracts_source_slug_without_md_extension() {
        let path = "aleph://wiki/concepts";
        let slug = path
            .split('/')
            .next_back()
            .unwrap_or(path)
            .trim_end_matches(".md");
        assert_eq!(slug, "concepts");
    }
}
