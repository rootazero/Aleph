//! `RawMemoryStore` implementation for `SqliteMemoryBackend`.
//!
//! Stores ephemeral raw memory records in the `raw_memories` table.
//! Records are consumed by `CompressionService` and marked processed.

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
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))
    };
}

/// Build a `LIKE` prefix pattern with `%`/`_`/`\` escaped so a literal
/// underscore in a session key or agent id cannot act as a single-char
/// wildcard and match another session's rows. Pair with `ESCAPE '\'`.
fn like_prefix_pattern(prefix: &str) -> String {
    let escaped = prefix
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("{escaped}%")
}

fn row_to_raw_memory(row: &rusqlite::Row) -> rusqlite::Result<RawMemory> {
    let source_str: String = row.get("source")?;
    let source_detail: Option<String> = row.get("source_detail")?;
    let is_processed_int: i64 = row.get("is_processed")?;

    Ok(RawMemory {
        id: row.get("id")?,
        content: row.get("content")?,
        source: RawMemorySource::from_persisted(&source_str, source_detail.as_deref()),
        agent_id: row.get("agent_id")?,
        session_id: row.get("session_id")?,
        path: row.get("path")?,
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
        let (src_token, src_detail) = raw.source.to_persisted();

        conn.execute(
            "INSERT INTO raw_memories \
             (id, content, source, source_detail, agent_id, session_id, path, attachment_text, is_processed, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                raw.id,
                raw.content,
                src_token,
                src_detail,
                raw.agent_id,
                raw.session_id,
                raw.path,
                raw.attachment_text,
                i64::from(raw.is_processed),
                raw.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("insert_raw_memory failed: {e}")))?;

        Ok(())
    }

    async fn insert_raw_memory_or_ignore(&self, raw: &RawMemory) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        let (src_token, src_detail) = raw.source.to_persisted();

        conn.execute(
            "INSERT OR IGNORE INTO raw_memories \
             (id, content, source, source_detail, agent_id, session_id, path, attachment_text, is_processed, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                raw.id,
                raw.content,
                src_token,
                src_detail,
                raw.agent_id,
                raw.session_id,
                raw.path,
                raw.attachment_text,
                i64::from(raw.is_processed),
                raw.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("insert_raw_memory_or_ignore failed: {e}")))?;

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
                "SELECT id, content, source, source_detail, agent_id, session_id, path, attachment_text, \
                 is_processed, created_at \
                 FROM raw_memories \
                 WHERE is_processed = 0 AND agent_id = ?1 \
                 ORDER BY created_at ASC \
                 LIMIT ?2",
            )
            .map_err(|e| {
                AlephError::config(format!("get_unprocessed_raw_memories prepare: {e}"))
            })?;

        let rows = stmt
            .query_map(params![agent_id, limit as i64], row_to_raw_memory)
            .map_err(|e| AlephError::config(format!("get_unprocessed_raw_memories query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| {
                AlephError::config(format!("get_unprocessed_raw_memories row: {e}"))
            })?);
        }
        Ok(results)
    }

    async fn mark_raw_as_processed(&self, ids: &[String]) -> Result<usize, AlephError> {
        if ids.is_empty() {
            return Ok(0);
        }

        let conn = lock_conn!(self)?;
        // SQLite places a hard limit on the number of host parameters in a
        // single statement (commonly 999). Chunk large ID lists to stay below it.
        const CHUNK_SIZE: usize = 900;
        let mut total = 0usize;

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("mark_raw_as_processed transaction: {e}")))?;

        for chunk in ids.chunks(CHUNK_SIZE) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "UPDATE raw_memories SET is_processed = 1 WHERE id IN ({})",
                placeholders.join(", ")
            );

            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|id| id as &dyn rusqlite::types::ToSql)
                .collect();

            let affected = tx
                .execute(&sql, params.as_slice())
                .map_err(|e| AlephError::config(format!("mark_raw_as_processed failed: {e}")))?;
            total += affected;
        }

        tx.commit()
            .map_err(|e| AlephError::config(format!("mark_raw_as_processed commit: {e}")))?;

        Ok(total)
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

    async fn unprocessed_agent_ids(&self) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT agent_id FROM raw_memories \
                 WHERE is_processed = 0 AND agent_id IS NOT NULL \
                 ORDER BY agent_id ASC",
            )
            .map_err(|e| AlephError::config(format!("unprocessed_agent_ids prepare: {e}")))?;

        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("unprocessed_agent_ids query: {e}")))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(
                row.map_err(|e| AlephError::config(format!("unprocessed_agent_ids row: {e}")))?,
            );
        }
        Ok(ids)
    }

    async fn get_raw_by_path_prefix(
        &self,
        path_prefix: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let conn = lock_conn!(self)?;

        let pattern = like_prefix_pattern(path_prefix);
        let mut stmt = conn
            .prepare(
                "SELECT id, content, source, source_detail, agent_id, session_id, path, attachment_text, \
                 is_processed, created_at \
                 FROM raw_memories \
                 WHERE path LIKE ?1 ESCAPE '\\' AND agent_id = ?2 \
                 ORDER BY created_at ASC \
                 LIMIT ?3",
            )
            .map_err(|e| AlephError::config(format!("get_raw_by_path_prefix prepare: {e}")))?;

        let rows = stmt
            .query_map(params![pattern, agent_id, limit as i64], row_to_raw_memory)
            .map_err(|e| AlephError::config(format!("get_raw_by_path_prefix query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| AlephError::config(format!("get_raw_by_path_prefix row: {e}")))?,
            );
        }
        Ok(results)
    }

    async fn get_raw_by_path_prefix_since(
        &self,
        path_prefix: &str,
        agent_id: &str,
        since_created_at: i64,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let conn = lock_conn!(self)?;

        let pattern = like_prefix_pattern(path_prefix);
        let mut stmt = conn
            .prepare(
                "SELECT id, content, source, source_detail, agent_id, session_id, path, attachment_text, \
                 is_processed, created_at \
                 FROM raw_memories \
                 WHERE path LIKE ?1 ESCAPE '\\' AND agent_id = ?2 AND created_at > ?3 \
                 ORDER BY created_at ASC \
                 LIMIT ?4",
            )
            .map_err(|e| AlephError::config(format!("get_raw_by_path_prefix_since prepare: {e}")))?;

        let rows = stmt
            .query_map(
                params![pattern, agent_id, since_created_at, limit as i64],
                row_to_raw_memory,
            )
            .map_err(|e| AlephError::config(format!("get_raw_by_path_prefix_since query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| {
                AlephError::config(format!("get_raw_by_path_prefix_since row: {e}"))
            })?);
        }
        Ok(results)
    }

    async fn find_by_path(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Option<RawMemory>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT id, content, source, source_detail, agent_id, session_id, path, attachment_text, \
                 is_processed, created_at \
                 FROM raw_memories \
                 WHERE path = ?1 AND agent_id = ?2 \
                 LIMIT 1",
            )
            .map_err(|e| AlephError::config(format!("find_by_path prepare: {e}")))?;

        let mut rows = stmt
            .query_map(params![path, agent_id], row_to_raw_memory)
            .map_err(|e| AlephError::config(format!("find_by_path query: {e}")))?;

        match rows.next() {
            Some(row) => {
                Ok(Some(row.map_err(|e| {
                    AlephError::config(format!("find_by_path row: {e}"))
                })?))
            }
            None => Ok(None),
        }
    }

    async fn get_raw_by_source(
        &self,
        source: RawMemorySource,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let conn = lock_conn!(self)?;
        let (src_token, _) = source.to_persisted();

        let mut stmt = conn
            .prepare(
                "SELECT id, content, source, source_detail, agent_id, session_id, path, attachment_text, \
                 is_processed, created_at \
                 FROM raw_memories \
                 WHERE source = ?1 AND agent_id = ?2 \
                 ORDER BY created_at DESC \
                 LIMIT ?3",
            )
            .map_err(|e| AlephError::config(format!("get_raw_by_source prepare: {e}")))?;

        let rows = stmt
            .query_map(
                params![src_token, agent_id, limit as i64],
                row_to_raw_memory,
            )
            .map_err(|e| AlephError::config(format!("get_raw_by_source query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results
                .push(row.map_err(|e| AlephError::config(format!("get_raw_by_source row: {e}")))?);
        }
        Ok(results)
    }

    async fn get_raws_by_ids(
        &self,
        agent_id: &str,
        ids: &[String],
    ) -> Result<Vec<RawMemory>, AlephError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = lock_conn!(self)?;
        // ?1 = agent_id; ?2 .. ?N+1 = ids
        let placeholders: Vec<String> = (0..ids.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "SELECT id, content, source, source_detail, agent_id, session_id, path, attachment_text, \
             is_processed, created_at \
             FROM raw_memories \
             WHERE agent_id = ?1 AND id IN ({})",
            placeholders.join(", ")
        );
        let mut param_values: Vec<&dyn rusqlite::types::ToSql> =
            vec![&agent_id as &dyn rusqlite::types::ToSql];
        for id in ids {
            param_values.push(id as &dyn rusqlite::types::ToSql);
        }
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_raws_by_ids prepare: {e}")))?;
        let rows = stmt
            .query_map(param_values.as_slice(), row_to_raw_memory)
            .map_err(|e| AlephError::config(format!("get_raws_by_ids query: {e}")))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AlephError::config(format!("get_raws_by_ids row: {e}")))?);
        }
        Ok(results)
    }

    async fn get_raws_by_session(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, content, source, source_detail, agent_id, session_id, path, attachment_text, \
                 is_processed, created_at \
                 FROM raw_memories \
                 WHERE agent_id = ?1 AND session_id = ?2 \
                 ORDER BY created_at ASC",
            )
            .map_err(|e| AlephError::config(format!("get_raws_by_session prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id, session_id], row_to_raw_memory)
            .map_err(|e| AlephError::config(format!("get_raws_by_session query: {e}")))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| AlephError::config(format!("get_raws_by_session row: {e}")))?,
            );
        }
        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Inherent audit readers (governance metrics)
// ---------------------------------------------------------------------------

impl SqliteMemoryBackend {
    /// Count raw memories whose `path` starts with `path_prefix` and were
    /// created after `since_created_at` (`created_at > since`, epoch **seconds**).
    /// Counts across all agents — this is the audit-wide reality probe behind
    /// the governance loop's "real corrections in last N days" check
    /// (`path LIKE 'aleph://correction/%'`), read in-core so the audit sensor
    /// never has to reach `~/.aleph/data` through the workspace sandbox. A
    /// `SELECT count(*)` (no row materialisation), sibling to the agent-scoped
    /// [`RawMemoryStore::get_raw_by_path_prefix_since`] fetch.
    pub fn count_raw_by_path_prefix_since(
        &self,
        path_prefix: &str,
        since_created_at: i64,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;
        let pattern = like_prefix_pattern(path_prefix);
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM raw_memories \
                 WHERE path LIKE ?1 ESCAPE '\\' AND created_at > ?2",
                params![pattern, since_created_at],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("count_raw_by_path_prefix_since: {e}")))?;
        Ok(count.max(0) as usize)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, SessionEndReason};

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
        let raw =
            RawMemory::new("data".to_string(), RawMemorySource::ToolOutput).with_agent("agent2");
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
        let mut raw = RawMemory::new("body".to_string(), RawMemorySource::Attachment);
        raw.attachment_text = Some("attachment content here".to_string());
        let raw = raw.with_agent("agent3");

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

    #[tokio::test]
    async fn get_raw_by_path_prefix_filters_by_prefix_and_agent() {
        let backend = make_backend();

        let r1 = RawMemory::new(
            "session a msg1".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_path("aleph://session/sess-a/d0/1")
        .with_agent("default");
        let r2 = RawMemory::new(
            "session b msg1".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_path("aleph://session/sess-b/d0/1")
        .with_agent("default");
        let r3 = RawMemory::new(
            "other agent".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_path("aleph://session/sess-a/d0/2")
        .with_agent("other");

        backend.insert_raw_memory(&r1).await.unwrap();
        backend.insert_raw_memory(&r2).await.unwrap();
        backend.insert_raw_memory(&r3).await.unwrap();

        let results = backend
            .get_raw_by_path_prefix("aleph://session/sess-a/", "default", 10)
            .await
            .unwrap();

        assert_eq!(
            results.len(),
            1,
            "Should only find sess-a for default agent"
        );
        assert_eq!(results[0].content, "session a msg1");
    }

    #[tokio::test]
    async fn get_raw_by_path_prefix_empty_result() {
        let backend = make_backend();

        let results = backend
            .get_raw_by_path_prefix("aleph://session/nonexistent/", "default", 10)
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn get_raw_by_path_prefix_ordered_by_created_at() {
        let backend = make_backend();

        let mut r1 = RawMemory::new("second".to_string(), RawMemorySource::SessionCompressed)
            .with_path("aleph://session/s/1")
            .with_agent("default");
        r1.created_at = 2000;

        let mut r2 = RawMemory::new("first".to_string(), RawMemorySource::SessionCompressed)
            .with_path("aleph://session/s/2")
            .with_agent("default");
        r2.created_at = 1000;

        backend.insert_raw_memory(&r1).await.unwrap();
        backend.insert_raw_memory(&r2).await.unwrap();

        let results = backend
            .get_raw_by_path_prefix("aleph://session/s/", "default", 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].content, "first",
            "Earlier created_at should come first"
        );
        assert_eq!(results[1].content, "second");
    }

    #[tokio::test]
    async fn get_raw_by_path_prefix_since_excludes_rows_at_or_before_watermark() {
        let backend = make_backend();

        let mut early = RawMemory::new("early".to_string(), RawMemorySource::SessionCompressed)
            .with_path("aleph://correction/c1")
            .with_agent("main");
        early.created_at = 1000;

        let mut at = RawMemory::new("at".to_string(), RawMemorySource::SessionCompressed)
            .with_path("aleph://correction/c2")
            .with_agent("main");
        at.created_at = 2000;

        let mut late = RawMemory::new("late".to_string(), RawMemorySource::SessionCompressed)
            .with_path("aleph://correction/c3")
            .with_agent("main");
        late.created_at = 3000;

        backend.insert_raw_memory(&early).await.unwrap();
        backend.insert_raw_memory(&at).await.unwrap();
        backend.insert_raw_memory(&late).await.unwrap();

        // since_created_at = 2000 means strictly greater — only `late` qualifies.
        let results = backend
            .get_raw_by_path_prefix_since("aleph://correction/", "main", 2000, 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "watermark filter is strictly greater");
        assert_eq!(results[0].content, "late");

        // since_created_at = 0 (fresh DB / first cycle) sees everything.
        let all = backend
            .get_raw_by_path_prefix_since("aleph://correction/", "main", 0, 10)
            .await
            .unwrap();
        assert_eq!(all.len(), 3);

        // Different agent is isolated.
        let other = backend
            .get_raw_by_path_prefix_since("aleph://correction/", "other-agent", 0, 10)
            .await
            .unwrap();
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn count_by_path_prefix_since_counts_across_agents_after_watermark() {
        let backend = make_backend();

        let mut old = RawMemory::new("old".to_string(), RawMemorySource::SessionCompressed)
            .with_path("aleph://correction/c1")
            .with_agent("main");
        old.created_at = 1000;

        let mut fresh_main =
            RawMemory::new("fresh".to_string(), RawMemorySource::SessionCompressed)
                .with_path("aleph://correction/c2")
                .with_agent("main");
        fresh_main.created_at = 3000;

        // A second agent's correction — the audit-wide count must include it
        // (the count reader is deliberately NOT agent-scoped, unlike the fetch).
        let mut fresh_other = RawMemory::new(
            "fresh-other".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_path("aleph://correction/c3")
        .with_agent("other-agent");
        fresh_other.created_at = 3000;

        // A non-correction path after the watermark must be excluded by prefix.
        let mut noise = RawMemory::new("noise".to_string(), RawMemorySource::SessionCompressed)
            .with_path("aleph://note/n1")
            .with_agent("main");
        noise.created_at = 3000;

        for m in [&old, &fresh_main, &fresh_other, &noise] {
            backend.insert_raw_memory(m).await.unwrap();
        }

        // Watermark 2000 (strictly greater): both fresh corrections across the
        // two agents count; the old one and the note are excluded.
        assert_eq!(
            backend
                .count_raw_by_path_prefix_since("aleph://correction/", 2000)
                .unwrap(),
            2
        );
        // From epoch: all three corrections; note still excluded by prefix.
        assert_eq!(
            backend
                .count_raw_by_path_prefix_since("aleph://correction/", 0)
                .unwrap(),
            3
        );
    }

    #[tokio::test]
    async fn raw_memory_source_detail_round_trips() {
        let backend = make_backend();
        let raw = RawMemory::new(
            "user: x\nassistant: y".into(),
            RawMemorySource::SessionEnd {
                reason: SessionEndReason::TaskDone,
            },
        )
        .with_agent("agent-1")
        .with_session("sess-1");

        backend.insert_raw_memory(&raw).await.unwrap();
        let fetched = backend
            .get_unprocessed_raw_memories("agent-1", 10)
            .await
            .unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(
            fetched[0].source,
            RawMemorySource::SessionEnd {
                reason: SessionEndReason::TaskDone
            }
        );
    }

    #[tokio::test]
    async fn legacy_null_source_detail_decodes_gracefully() {
        // Simulates a row inserted before the source_detail column existed.
        // Insert via raw SQL with source_detail = NULL, verify it round-trips
        // to the simple token-only variant (ToolOutput in this case).
        let backend = make_backend();
        {
            let conn = backend.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO raw_memories \
                 (id, content, source, source_detail, agent_id, session_id, path, attachment_text, is_processed, created_at) \
                 VALUES ('legacy-1', 'legacy content', 'tool_output', NULL, 'agent-legacy', NULL, NULL, NULL, 0, 1000)",
                [],
            )
            .unwrap();
        }
        let fetched = backend
            .get_unprocessed_raw_memories("agent-legacy", 10)
            .await
            .unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].source, RawMemorySource::ToolOutput);
        assert_eq!(fetched[0].content, "legacy content");
    }

    #[tokio::test]
    async fn dashboard_offset_paginates_stably() {
        let backend = make_backend();
        // Insert 5 rows with increasing created_at so DESC order is deterministic.
        for i in 0..5 {
            let mut raw = RawMemory::new(format!("entry-{i}"), RawMemorySource::Transcript)
                .with_agent("pager");
            raw.created_at = 1000 + i as i64;
            backend.insert_raw_memory(&raw).await.unwrap();
        }

        // Page 1 (offset 0, limit 2): newest two.
        let p1 = backend
            .get_raw_memories_dashboard(Some("pager"), None, 2, 0)
            .unwrap();
        assert_eq!(p1.len(), 2);
        assert_eq!(p1[0].content, "entry-4");
        assert_eq!(p1[1].content, "entry-3");

        // Page 2 (offset 2): next two, no overlap with page 1.
        let p2 = backend
            .get_raw_memories_dashboard(Some("pager"), None, 2, 2)
            .unwrap();
        assert_eq!(p2.len(), 2);
        assert_eq!(p2[0].content, "entry-2");
        assert_eq!(p2[1].content, "entry-1");

        // Page 3 (offset 4): final partial page.
        let p3 = backend
            .get_raw_memories_dashboard(Some("pager"), None, 2, 4)
            .unwrap();
        assert_eq!(p3.len(), 1);
        assert_eq!(p3[0].content, "entry-0");
    }

    #[tokio::test]
    async fn delete_raw_memory_removes_only_target() {
        let backend = make_backend();
        let keep =
            RawMemory::new("keep me".to_string(), RawMemorySource::Transcript).with_agent("del");
        let drop =
            RawMemory::new("drop me".to_string(), RawMemorySource::Transcript).with_agent("del");
        let drop_id = drop.id.clone();
        backend.insert_raw_memory(&keep).await.unwrap();
        backend.insert_raw_memory(&drop).await.unwrap();

        // Deleting an existing id reports success and removes exactly one row.
        assert!(backend.delete_raw_memory(&drop_id).unwrap());
        let remaining = backend
            .get_raw_memories_dashboard(Some("del"), None, 10, 0)
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, keep.id);

        // Deleting a non-existent id is a no-op reporting `false`.
        assert!(!backend.delete_raw_memory("ghost").unwrap());
    }

    #[tokio::test]
    async fn dashboard_and_count_exclude_tool_invocation_telemetry() {
        let backend = make_backend();

        // One genuine memory + three per-call tool-telemetry rows.
        let real = RawMemory::new("real conversation".to_string(), RawMemorySource::Transcript)
            .with_agent("noise");
        backend.insert_raw_memory(&real).await.unwrap();
        for i in 0..3 {
            let tele = RawMemory::new(
                format!("tool file_read ok in {i}ms"),
                RawMemorySource::ToolInvocation {
                    tool_name: "file_read".to_string(),
                    success: true,
                    duration_ms: i,
                },
            )
            .with_agent("noise");
            backend.insert_raw_memory(&tele).await.unwrap();
        }

        // Dashboard (both agent-scoped and global) hides telemetry.
        let scoped = backend
            .get_raw_memories_dashboard(Some("noise"), None, 100, 0)
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].content, "real conversation");

        let global = backend
            .get_raw_memories_dashboard(None, None, 100, 0)
            .unwrap();
        assert_eq!(global.len(), 1);

        // The user-facing count matches the dashboard (1, not 4).
        assert_eq!(backend.count_raw_memories(None, None).unwrap(), 1);

        // Count and list must agree under EVERY filter combination — a global
        // count paired with a scoped list is what produced phantom pages.
        for (agent, query) in [
            (None, None),
            (Some("noise"), None),
            (None, Some("real")),
            (Some("noise"), Some("real")),
            (Some("noise"), Some("nonexistent-needle")),
        ] {
            let listed = backend
                .get_raw_memories_dashboard(agent, query, 1000, 0)
                .unwrap()
                .len() as i64;
            let counted = backend.count_raw_memories(agent, query).unwrap();
            assert_eq!(
                listed, counted,
                "count/list disagree for agent={agent:?} query={query:?}"
            );
        }

        // Telemetry is still retrievable by source for the Dream metrics path.
        let tele_rows = backend
            .get_raw_by_source(
                RawMemorySource::ToolInvocation {
                    tool_name: "file_read".to_string(),
                    success: true,
                    duration_ms: 0,
                },
                "noise",
                100,
            )
            .await
            .unwrap();
        assert_eq!(tele_rows.len(), 3);
    }

    #[tokio::test]
    async fn raw_fetch_by_id_and_session() {
        let dir = tempfile::tempdir().unwrap();
        let backend = SqliteMemoryBackend::new(&dir.path().join("m.db")).unwrap();
        let mut r = RawMemory::new("hello".into(), RawMemorySource::Transcript);
        r.agent_id = "default".into();
        r.session_id = Some("ses_x".into());
        backend.insert_raw_memory(&r).await.unwrap();

        let by_id = backend
            .get_raws_by_ids("default", &[r.id.clone()])
            .await
            .unwrap();
        assert_eq!(by_id.len(), 1);
        let by_ses = backend
            .get_raws_by_session("default", "ses_x")
            .await
            .unwrap();
        assert_eq!(by_ses.len(), 1);
    }
}
