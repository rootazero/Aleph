//! Spec B — fetch the canonical /end-summary RawMemory for a session, if it exists.
//!
//! Used by `session_search` and the SessionEndSummarizer to decide whether
//! the lazy synthesizer must run.
//!
//! Naming note: the plan calls this "retrieve_summary_fact" / "MemoryFact"
//! but actual storage is `RawMemory` in the `raw_memories` table.

use std::sync::Arc;

use crate::error::AlephError;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

/// Returns the canonical `/end-summary` row for `(agent_id, session_id)` if
/// one has already been written. Returns `Ok(None)` for sessions without a
/// summary yet, or for sessions whose only entry at that path has the wrong
/// source variant (defensive guard).
pub async fn retrieve_summary_fact(
    store: &Arc<dyn RawMemoryStore>,
    agent_id: &str,
    session_id: &str,
) -> Result<Option<RawMemory>, AlephError> {
    let path = format!("aleph://session/{session_id}/end-summary");
    let raw = store.find_by_path(&path, agent_id).await?;
    Ok(raw.filter(|r| matches!(r.source, RawMemorySource::SessionCompressed)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    async fn build_in_memory_store() -> Arc<dyn RawMemoryStore> {
        Arc::new(SqliteMemoryBackend::in_memory().expect("in-memory backend"))
    }

    #[tokio::test]
    async fn returns_none_when_no_summary_exists() {
        let store = build_in_memory_store().await;
        let result = retrieve_summary_fact(&store, "agent-1", "sess-empty")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn returns_raw_when_summary_exists() {
        let store = build_in_memory_store().await;
        let raw = RawMemory::new("Test summary".to_string(), RawMemorySource::SessionCompressed)
            .with_agent("agent-1")
            .with_session("sess-found")
            .with_path("aleph://session/sess-found/end-summary");
        store.insert_raw_memory(&raw).await.unwrap();

        let result = retrieve_summary_fact(&store, "agent-1", "sess-found")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.content, "Test summary");
        assert_eq!(
            result.path.as_deref(),
            Some("aleph://session/sess-found/end-summary")
        );
        assert!(matches!(result.source, RawMemorySource::SessionCompressed));
    }

    #[tokio::test]
    async fn ignores_non_session_compressed_at_same_path() {
        // Defensive — if some bug ever puts a non-SessionCompressed RawMemory
        // at /end-summary, we don't accidentally serve it as a summary.
        let store = build_in_memory_store().await;
        let raw = RawMemory::new("Wrong".to_string(), RawMemorySource::Transcript)
            .with_agent("agent-1")
            .with_session("sess-bad")
            .with_path("aleph://session/sess-bad/end-summary");
        store.insert_raw_memory(&raw).await.unwrap();

        let result = retrieve_summary_fact(&store, "agent-1", "sess-bad")
            .await
            .unwrap();
        assert!(result.is_none(), "non-SessionCompressed must not be served");
    }
}
