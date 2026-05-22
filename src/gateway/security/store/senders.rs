use super::{current_timestamp_ms, SecurityStore};
use rusqlite::{params, Result as SqliteResult};

impl SecurityStore {
    /// Approve a channel sender
    pub fn approve_sender(&self, channel: &str, sender_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO approved_senders (channel, sender_id, approved_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(channel, sender_id) DO UPDATE SET
               approved_at = excluded.approved_at,
               revoked_at = NULL",
            params![channel, sender_id, current_timestamp_ms()],
        )?;
        Ok(())
    }

    /// Check if sender is approved
    pub fn is_sender_approved(&self, channel: &str, sender_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT 1 FROM approved_senders WHERE channel = ?1 AND sender_id = ?2 AND revoked_at IS NULL",
        )?;
        let exists: Result<i32, _> = stmt.query_row(params![channel, sender_id], |row| row.get(0));
        Ok(exists.is_ok())
    }

    /// Revoke a sender
    pub fn revoke_sender(&self, channel: &str, sender_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute(
            "UPDATE approved_senders SET revoked_at = ?1 WHERE channel = ?2 AND sender_id = ?3 AND revoked_at IS NULL",
            params![current_timestamp_ms(), channel, sender_id],
        )?;
        Ok(rows > 0)
    }

    /// List approved senders for a channel
    pub fn list_senders(&self, channel: &str) -> SqliteResult<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT sender_id, approved_at FROM approved_senders
             WHERE channel = ?1 AND revoked_at IS NULL
             ORDER BY approved_at DESC",
        )?;

        let rows = stmt.query_map(params![channel], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect()
    }

    // ========== Channel Policy Operations ==========

    /// Get DM policy for a channel. Returns None if not persisted (use config default).
    pub fn get_channel_dm_policy(
        &self,
        channel_id: &str,
    ) -> SqliteResult<Option<(String, Option<String>)>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT policy, allowlist FROM channel_policies
             WHERE channel_id = ?1 AND policy_type = 'dm_policy'
             ORDER BY updated_at DESC LIMIT 1",
        )?;

        let result = stmt.query_row(params![channel_id], |row| {
            let policy: String = row.get(0)?;
            let allowlist: Option<String> = row.get(1)?;
            Ok((policy, allowlist))
        });

        match result {
            Ok(policy) => Ok(Some(policy)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Set DM policy for a channel.
    pub fn set_channel_dm_policy(
        &self,
        channel_id: &str,
        policy: &str,
        allowlist: Option<&str>,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO channel_policies (channel_id, policy_type, policy, allowlist, updated_at)
             VALUES (?1, 'dm_policy', ?2, ?3, ?4)
             ON CONFLICT(channel_id, policy_type) DO UPDATE SET
               policy = excluded.policy,
               allowlist = excluded.allowlist,
               updated_at = excluded.updated_at",
            params![channel_id, policy, allowlist, current_timestamp_ms()],
        )?;
        Ok(())
    }
}
