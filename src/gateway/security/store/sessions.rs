use super::{current_timestamp_ms, SecurityStore};
use rusqlite::{params, Result as SqliteResult};

impl SecurityStore {
    /// Insert a new session.
    pub fn insert_session(
        &self,
        session_id: &str,
        token_hash: &str,
        created_at: i64,
        expires_at: i64,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO sessions (session_id, token_hash, created_at, expires_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
               token_hash = excluded.token_hash,
               expires_at = excluded.expires_at",
            params![session_id, token_hash, created_at, expires_at],
        )?;
        Ok(())
    }

    /// Check if a session is still valid (not expired).
    pub fn validate_session(&self, session_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt =
            conn.prepare("SELECT 1 FROM sessions WHERE session_id = ?1 AND expires_at > ?2")?;
        let exists: Result<i32, _> = stmt
            .query_row(params![session_id, current_timestamp_ms()], |row| {
                row.get(0)
            });
        Ok(exists.is_ok())
    }

    /// Update session `last_used_at` timestamp.
    pub fn touch_session(&self, session_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE sessions SET last_used_at = ?1 WHERE session_id = ?2",
            params![current_timestamp_ms(), session_id],
        )?;
        Ok(())
    }

    /// Delete a specific session.
    pub fn delete_session(&self, session_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "DELETE FROM sessions WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Delete all expired sessions, returning the count removed.
    pub fn delete_expired_sessions(&self) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "DELETE FROM sessions WHERE expires_at <= ?1",
            params![current_timestamp_ms()],
        )?;
        Ok(rows as u64)
    }

    /// List active (non-expired) sessions as (`session_id`, `created_at`, `expires_at`, `last_used_at`).
    pub fn list_active_sessions(&self) -> SqliteResult<Vec<(String, i64, i64, i64)>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT session_id, created_at, expires_at, last_used_at
             FROM sessions WHERE expires_at > ?1
             ORDER BY last_used_at DESC",
        )?;

        let rows = stmt.query_map(params![current_timestamp_ms()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        rows.collect()
    }
}
