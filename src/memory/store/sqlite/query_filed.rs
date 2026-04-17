//! query_filed table access — dedup lookups and inserts for the query filer.

use rusqlite::params;

use crate::error::AlephError;
use crate::memory::notes::query_filer::types::QueryFiledRow;

use super::SqliteMemoryBackend;

impl SqliteMemoryBackend {
    /// Look up an existing filed query by agent and query hash.
    ///
    /// Returns the `note_path` if the query was already filed, `None` otherwise.
    pub fn query_filed_lookup(
        &self,
        agent_id: &str,
        query_hash: &str,
    ) -> Result<Option<String>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare("SELECT note_path FROM query_filed WHERE agent_id = ?1 AND query_hash = ?2")
            .map_err(|e| AlephError::other(format!("query_filed_lookup prepare: {e}")))?;

        let result = stmt
            .query_row(params![agent_id, query_hash], |row| row.get(0))
            .ok();

        Ok(result)
    }

    /// Insert a query_filed row. Uses INSERT OR IGNORE so duplicates are silently
    /// rejected by the UNIQUE(agent_id, query_hash) constraint.
    pub fn query_filed_insert(&self, row: &QueryFiledRow) -> Result<(), AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.execute(
            "INSERT OR IGNORE INTO query_filed \
             (id, agent_id, query_hash, note_path, session_id, filed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.id,
                row.agent_id,
                row.query_hash,
                row.note_path,
                row.session_id,
                row.filed_at,
            ],
        )
        .map_err(|e| AlephError::other(format!("query_filed_insert: {e}")))?;

        Ok(())
    }
}
