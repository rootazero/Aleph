//! RawMemoryStore implementation for SqliteMemoryBackend.
//!
//! Stores ephemeral raw memory records in the `raw_memories` table.
//! Records are consumed by CompressionService and marked processed.

use async_trait::async_trait;
use rusqlite::params;

use crate::error::AlephError;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

use super::SqliteMemoryBackend;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

macro_rules! lock_conn {
    ($self:expr) => {
        $self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Lock: {}", e)))
    };
}

fn row_to_raw_memory(row: &rusqlite::Row) -> rusqlite::Result<RawMemory> {
    let source_str: String = row.get("source")?;
    let is_processed_int: i64 = row.get("is_processed")?;

    Ok(RawMemory {
        id: row.get("id")?,
        content: row.get("content")?,
        source: RawMemorySource::from_str(&source_str),
        agent_id: row.get("agent_id")?,
        session_id: row.get("session_id")?,
        path: row.get("path")?,
        layer: row.get("layer")?,
        attachment_text: row.get("attachment_text")?,
        is_processed: is_processed_int != 0,
        created_at: row.get("created_at")?,
    })
}

// ---------------------------------------------------------------------------
// RawMemoryStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl RawMemoryStore for SqliteMemoryBackend {
    async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        conn.execute(
            "INSERT INTO raw_memories \
             (id, content, source, agent_id, session_id, path, layer, attachment_text, is_processed, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                raw.id,
                raw.content,
                raw.source.as_str(),
                raw.agent_id,
                raw.session_id,
                raw.path,
                raw.layer,
                raw.attachment_text,
                raw.is_processed as i64,
                raw.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("insert_raw_memory failed: {e}")))?;

        Ok(())
    }

    async fn get_unprocessed_raw_memories(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT id, content, source, agent_id, session_id, path, layer, attachment_text, \
                 is_processed, created_at \
                 FROM raw_memories \
                 WHERE is_processed = 0 AND agent_id = ?1 \
                 ORDER BY created_at ASC \
                 LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("get_unprocessed_raw_memories prepare: {e}")))?;

        let rows = stmt
            .query_map(params![agent_id, limit as i64], row_to_raw_memory)
            .map_err(|e| AlephError::config(format!("get_unprocessed_raw_memories query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| {
                    AlephError::config(format!("get_unprocessed_raw_memories row: {e}"))
                })?,
            );
        }
        Ok(results)
    }

    async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError> {
        if ids.is_empty() {
            return Ok(0);
        }

        let conn = lock_conn!(self)?;

        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "UPDATE raw_memories SET is_processed = 1 WHERE id IN ({})",
            placeholders.join(", ")
        );

        let params: Vec<&dyn rusqlite::types::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();

        let affected = conn
            .execute(&sql, params.as_slice())
            .map_err(|e| AlephError::config(format!("mark_raw_as_processed failed: {e}")))?;

        Ok(affected)
    }

    async fn count_unprocessed(&self, agent_id: &str) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM raw_memories WHERE is_processed = 0 AND agent_id = ?1",
                params![agent_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("count_unprocessed failed: {e}")))?;

        Ok(count.max(0) as usize)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};

    fn make_backend() -> SqliteMemoryBackend {
        SqliteMemoryBackend::in_memory().expect("in-memory backend")
    }

    #[tokio::test]
    async fn insert_and_retrieve_raw_memory() {
        let backend = make_backend();
        let raw = RawMemory::new("hello world".to_string(), RawMemorySource::Transcript)
            .with_agent("agent1");

        backend.insert_raw_memory(&raw).await.unwrap();

        let results = backend
            .get_unprocessed_raw_memories("agent1", 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, raw.id);
        assert_eq!(results[0].content, "hello world");
        assert_eq!(results[0].agent_id, "agent1");
        assert!(!results[0].is_processed);
    }

    #[tokio::test]
    async fn mark_as_processed_excludes_from_query() {
        let backend = make_backend();
        let raw = RawMemory::new("data".to_string(), RawMemorySource::ToolOutput)
            .with_agent("agent2");
        let id = raw.id.clone();

        backend.insert_raw_memory(&raw).await.unwrap();
        let affected = backend.mark_raw_as_processed(&[id]).await.unwrap();
        assert_eq!(affected, 1);

        let results = backend
            .get_unprocessed_raw_memories("agent2", 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn empty_store_returns_empty() {
        let backend = make_backend();

        let results = backend
            .get_unprocessed_raw_memories("nobody", 10)
            .await
            .unwrap();
        assert!(results.is_empty());

        let count = backend.count_unprocessed("nobody").await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn attachment_text_preserved() {
        let backend = make_backend();
        let raw = RawMemory::new("body".to_string(), RawMemorySource::Attachment)
            .with_agent("agent3")
            .with_attachment_text("attachment content here");

        backend.insert_raw_memory(&raw).await.unwrap();

        let results = backend
            .get_unprocessed_raw_memories("agent3", 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].attachment_text.as_deref(),
            Some("attachment content here")
        );
    }
}
