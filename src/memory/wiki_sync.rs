//! Wiki wikilink synchronization.
//!
//! Previously synchronized `[[wikilink]]` from Wiki-type facts to the
//! knowledge graph (graph_nodes/graph_edges/memory_entities). That graph
//! system has been replaced by Knowledge Notes with native wikilink support.
//!
//! This module is retained as a no-op stub so that existing call sites
//! compile without changes. It will be fully removed in a future cleanup.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;

/// Summary of a wikilink sync operation (now always zero).
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub cleared: usize,
    pub linked: usize,
}

/// No-op: wikilink sync is deprecated. Knowledge Notes handle links natively.
pub async fn sync_wikilinks_to_graph(
    _fact: &MemoryFact,
    _graph_store: &(), // placeholder — callers will be cleaned up
) -> Result<SyncReport, AlephError> {
    Ok(SyncReport::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sync_returns_empty_report() {
        let fact = crate::memory::context::MemoryFact::new(
            "test".into(),
            crate::memory::context::NoteType::Wiki,
            vec![],
        );
        let report = sync_wikilinks_to_graph(&fact, &()).await.unwrap();
        assert_eq!(report.cleared, 0);
        assert_eq!(report.linked, 0);
    }
}
