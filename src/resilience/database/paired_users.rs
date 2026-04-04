//! CRUD operations for paired_users table
//!
//! Provides persistent storage of paired (allowed) Telegram users per channel,
//! enabling pairing state to survive restarts.

use super::StateDatabase;
use crate::error::AlephError;
use rusqlite::params;

impl StateDatabase {
    // =========================================================================
    // Paired Users
    // =========================================================================

    /// Load all paired user IDs for a channel.
    ///
    /// Returns an empty `Vec` if no users are paired yet.
    pub fn load_paired_users(&self, channel_id: &str) -> Result<Vec<i64>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT user_id FROM paired_users WHERE channel_id = ?1")
            .map_err(|e| {
                AlephError::config(format!("Failed to prepare load_paired_users: {}", e))
            })?;
        let rows = stmt
            .query_map(params![channel_id], |row| row.get(0))
            .map_err(|e| {
                AlephError::config(format!("Failed to load paired users: {}", e))
            })?;
        let mut user_ids = Vec::new();
        for row in rows {
            user_ids.push(row.map_err(|e| {
                AlephError::config(format!("Failed to read paired user row: {}", e))
            })?);
        }
        Ok(user_ids)
    }

    /// Add a paired user to a channel.
    ///
    /// Uses INSERT OR IGNORE so duplicate pairs are silently skipped.
    pub fn add_paired_user(&self, channel_id: &str, user_id: i64) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR IGNORE INTO paired_users (channel_id, user_id, paired_at) VALUES (?1, ?2, datetime('now'))",
            params![channel_id, user_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to add paired user: {}", e)))?;
        Ok(())
    }

    /// Remove a paired user from a channel.
    pub fn remove_paired_user(&self, channel_id: &str, user_id: i64) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "DELETE FROM paired_users WHERE channel_id = ?1 AND user_id = ?2",
            params![channel_id, user_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to remove paired user: {}", e)))?;
        Ok(())
    }
}
