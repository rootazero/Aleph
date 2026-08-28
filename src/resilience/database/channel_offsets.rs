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

    /// Get the last processed `update_id` for a channel.
    ///
    /// Returns `None` if no offset has been recorded yet (first startup).
    pub async fn get_channel_offset(&self, channel_id: &str) -> Result<Option<i64>, AlephError> {
        let channel_id = channel_id.to_string();
        self.with_conn(move |conn| {
            let result = conn
                .query_row(
                    "SELECT last_update_id FROM channel_offsets WHERE channel_id = ?1",
                    params![channel_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| AlephError::config(format!("Failed to get channel offset: {e}")))?;
            Ok(result)
        })
        .await
    }

    /// Upsert the last processed `update_id` for a channel.
    ///
    /// Monotonic: the persisted `last_update_id` is the MAX of the existing
    /// and the newly supplied value. A late or out-of-order writer can NOT
    /// regress the offset (which would cause message re-processing or
    /// duplication on restart — the very failure mode this table exists to
    /// prevent). `bot_id` and `updated_at` are written unconditionally
    /// because they are metadata, not the cursor.
    pub async fn set_channel_offset(
        &self,
        channel_id: &str,
        bot_id: &str,
        update_id: i64,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let channel_id = channel_id.to_string();
        let bot_id = bot_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                r#"
                INSERT INTO channel_offsets (channel_id, bot_id, last_update_id, updated_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(channel_id) DO UPDATE SET
                    last_update_id = MAX(last_update_id, excluded.last_update_id),
                    bot_id         = excluded.bot_id,
                    updated_at     = excluded.updated_at
                "#,
                params![channel_id, bot_id, update_id, now],
            )
            .map_err(|e| AlephError::config(format!("Failed to set channel offset: {e}")))?;
            Ok(())
        })
        .await
    }
}
