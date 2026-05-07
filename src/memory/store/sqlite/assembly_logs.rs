//! Assembly log persistence (Memory Evolution Spec 1).
//!
//! Optional per-assembly decision rows. Default-disabled; Spec 2 consumes
//! these rows to correlate assembly decisions with citation / re-retrieval
//! signals. Schema DDL is installed by `init_schema` in `schema.rs`.

use rusqlite::params;

use crate::error::AlephError;

use super::SqliteMemoryBackend;

macro_rules! lock_conn {
    ($self:expr) => {
        $self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))
    };
}

/// Row written to the `assembly_logs` table.
#[derive(Debug, Clone)]
pub struct AssemblyLogRow {
    pub id: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub query_hash: String,
    pub strategy: String,
    pub used_fallback: bool,
    pub fallback_reason: Option<String>,
    pub candidates_count: i64,
    pub selected_item_ids: String,
    pub total_tokens: i64,
    pub rerank_latency_ms: Option<i64>,
    pub total_latency_ms: i64,
    pub created_at: i64,
}

impl SqliteMemoryBackend {
    /// Insert a single row into `assembly_logs`. Caller decides whether to
    /// call this based on `AssemblyLogConfig.enabled`.
    pub fn insert_assembly_log(&self, row: &AssemblyLogRow) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        conn.execute(
            "INSERT INTO assembly_logs (\
                 id, agent_id, session_id, query_hash, strategy, used_fallback, \
                 fallback_reason, candidates_count, selected_item_ids, total_tokens, \
                 rerank_latency_ms, total_latency_ms, created_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                row.id,
                row.agent_id,
                row.session_id,
                row.query_hash,
                row.strategy,
                row.used_fallback as i64,
                row.fallback_reason,
                row.candidates_count,
                row.selected_item_ids,
                row.total_tokens,
                row.rerank_latency_ms,
                row.total_latency_ms,
                row.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("insert_assembly_log: {e}")))?;
        Ok(())
    }

    /// Count rows in `assembly_logs`. Helper for tests and introspection.
    pub fn assembly_log_count(&self) -> Result<i64, AlephError> {
        let conn = lock_conn!(self)?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM assembly_logs", [], |r| r.get(0))
            .map_err(|e| AlephError::config(format!("assembly_log_count: {e}")))?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use std::path::PathBuf;

    fn tmp_backend() -> SqliteMemoryBackend {
        let tmp = tempfile::tempdir().unwrap();
        let path: PathBuf = tmp.path().join("mem.db");
        // Leak the tempdir so the file stays alive for the test duration.
        Box::leak(Box::new(tmp));
        SqliteMemoryBackend::new(&path).unwrap()
    }

    #[test]
    fn insert_and_count_round_trip() {
        let backend = tmp_backend();
        assert_eq!(backend.assembly_log_count().unwrap(), 0);
        let row = AssemblyLogRow {
            id: "id-1".into(),
            agent_id: "default".into(),
            session_id: Some("sess".into()),
            query_hash: "hash".into(),
            strategy: "hybrid_v1".into(),
            used_fallback: false,
            fallback_reason: None,
            candidates_count: 7,
            selected_item_ids: "[\"note://a\"]".into(),
            total_tokens: 123,
            rerank_latency_ms: Some(250),
            total_latency_ms: 400,
            created_at: 1_700_000_000,
        };
        backend.insert_assembly_log(&row).unwrap();
        assert_eq!(backend.assembly_log_count().unwrap(), 1);
    }
}
