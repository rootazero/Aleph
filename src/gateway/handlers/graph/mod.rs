//! Graph Query Handler
//!
//! Handles JSON-RPC requests for knowledge graph visualization.
//! These handlers query the `NoteStore` for note index data and links.

use crate::gateway::handlers::graph_types::NoteNodeDto;
use crate::memory::notes::store::NoteIndexEntry;

mod manage;
mod node_detail;
mod query;
mod search;

pub use manage::{handle_delete_note_impl, handle_rename_note_impl, handle_update_note_impl};
pub use node_detail::handle_node_detail_impl;
pub use query::handle_query_impl;
pub use search::handle_search_impl;

/// Convert a `NoteIndexEntry` into a `NoteNodeDto`.
/// `community_id` is left `None` here; callers that have the community map
/// (graph.query) fill it in a second pass.
pub(crate) fn entry_to_dto(entry: &NoteIndexEntry) -> NoteNodeDto {
    NoteNodeDto {
        id: entry.path.clone(),
        name: entry.filename.clone(),
        path: entry.path.clone(),
        category: entry.category.clone(),
        tags: entry.tags.clone(),
        link_count: entry.link_count,
        community_id: None,
        updated_at: Some(entry.updated_at),
    }
}

/// Resolve the note memory directory: `~/.aleph/memory/note/`
pub(crate) fn notes_dir() -> std::path::PathBuf {
    crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("aleph")
            .join("memory")
            .join("note")
    })
}

/// Order-independent key for an edge pair, so a real link `A->B` and a
/// materialized similarity edge `B->A` are recognized as the same pair for
/// dedup purposes.
pub(crate) fn undirected_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::store::MemoryBackend;
    use crate::sync_primitives::Arc;
    use uuid::Uuid;

    /// Create a fresh file-backed `MemoryBackend` for testing.
    pub fn make_db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("graph_handler_test_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    pub fn make_note(title: &str, category: &str, links: Vec<&str>) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: category.to_string(),
            tags: vec![],
            facts: vec!["fact".to_string()],
            links: links.into_iter().map(String::from).collect(),
            created_at: 1_700_000_000,
            updated_at: 1_700_001_000,
            content_hash: format!("hash_{title}"),
            ..Default::default()
        }
    }
}
