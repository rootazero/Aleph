//! CRUD operations for channel_offsets table
//!
//! Provides persistent storage of polling offsets (last processed update_id)
//! per channel, enabling graceful restart without message loss or duplication.

use super::StateDatabase;
use crate::error::AlephError;
use rusqlite::params;
use rusqlite::OptionalExtension;

impl StateDatabase {
    // =========================================================================
    // Channel Offsets
    // =========================================================================

    /// Get the last processed update_id for a channel.
    ///
    /// Returns `None` if no offset has been recorded yet (first startup).
    pub fn get_channel_offset(&self, channel_id: &str) -> Result<Option<i64>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let result = conn
            .query_row(
                "SELECT last_update_id FROM channel_offsets WHERE channel_id = ?1",
                params![channel_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to get channel offset: {}", e)))?;
        Ok(result)
    }

    /// Upsert the last processed update_id for a channel.
    ///
    /// Uses INSERT OR REPLACE so the row is created on first call and
    /// updated on subsequent calls.
    pub fn set_channel_offset(
        &self,
        channel_id: &str,
        bot_id: &str,
        update_id: i64,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT OR REPLACE INTO channel_offsets (channel_id, bot_id, last_update_id, updated_at)
            VALUES (?1, ?2, ?3, datetime('now'))
            "#,
            params![channel_id, bot_id, update_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to set channel offset: {}", e)))?;
        Ok(())
    }
}
