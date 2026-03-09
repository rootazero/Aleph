//! CRUD operations for group_chat tables.
//!
//! Provides database operations for group chat session persistence
//! and conversation turn tracking.

use crate::error::AlephError;
use super::StateDatabase;
use rusqlite::params;

/// A single conversation turn: (round, sequence, speaker_type, speaker_id, speaker_name, content, timestamp).
pub type GroupChatTurn = (u32, u32, String, Option<String>, String, String, i64);

/// An active session summary: (id, topic, source_channel, created_at).
pub type GroupChatSessionSummary = (String, Option<String>, String, i64);

impl StateDatabase {
    // =========================================================================
    // Group Chat Sessions CRUD
    // =========================================================================

    /// Insert a new group chat session.
    pub fn insert_group_chat_session(
        &self,
        id: &str,
        topic: Option<&str>,
        source_channel: &str,
        source_session_key: &str,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO group_chat_sessions (
                id, topic, status, source_channel, source_session_key,
                created_at, updated_at
            ) VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6)
            "#,
            params![id, topic, source_channel, source_session_key, now, now],
        )
        .map_err(|e| {
            AlephError::config(format!("Failed to insert group chat session: {}", e))
        })?;
        Ok(())
    }

    /// Update the status of a group chat session.
    pub fn update_group_chat_session_status(
        &self,
        session_id: &str,
        status: &str,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            UPDATE group_chat_sessions
            SET status = ?1, updated_at = ?2
            WHERE id = ?3
            "#,
            params![status, now, session_id],
        )
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to update group chat session status: {}",
                e
            ))
        })?;
        Ok(())
    }

    /// Insert a conversation turn into a group chat session.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_group_chat_turn(
        &self,
        session_id: &str,
        round: u32,
        sequence: u32,
        speaker_type: &str,
        speaker_id: Option<&str>,
        speaker_name: &str,
        content: &str,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO group_chat_turns (
                session_id, round, sequence, speaker_type, speaker_id,
                speaker_name, content, timestamp
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                session_id,
                round,
                sequence,
                speaker_type,
                speaker_id,
                speaker_name,
                content,
                now
            ],
        )
        .map_err(|e| {
            AlephError::config(format!("Failed to insert group chat turn: {}", e))
        })?;
        Ok(())
    }

    /// Get all turns for a group chat session, ordered by round and sequence.
    ///
    /// Returns tuples of (round, sequence, speaker_type, speaker_id, speaker_name, content, timestamp).
    pub fn get_group_chat_turns(
        &self,
        session_id: &str,
    ) -> Result<Vec<GroupChatTurn>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT round, sequence, speaker_type, speaker_id,
                       speaker_name, content, timestamp
                FROM group_chat_turns
                WHERE session_id = ?1
                ORDER BY round ASC, sequence ASC
                "#,
            )
            .map_err(|e| {
                AlephError::config(format!("Failed to prepare group chat turns query: {}", e))
            })?;

        let turns = stmt
            .query_map(params![session_id], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(|e| {
                AlephError::config(format!("Failed to query group chat turns: {}", e))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                AlephError::config(format!("Failed to collect group chat turns: {}", e))
            })?;

        Ok(turns)
    }

    /// List all active group chat sessions.
    ///
    /// Returns tuples of (id, topic, source_channel, created_at).
    pub fn list_active_group_chats(
        &self,
    ) -> Result<Vec<GroupChatSessionSummary>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, topic, source_channel, created_at
                FROM group_chat_sessions
                WHERE status = 'active'
                ORDER BY created_at DESC
                "#,
            )
            .map_err(|e| {
                AlephError::config(format!(
                    "Failed to prepare active group chats query: {}",
                    e
                ))
            })?;

        let sessions = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| {
                AlephError::config(format!("Failed to query active group chats: {}", e))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                AlephError::config(format!("Failed to collect active group chats: {}", e))
            })?;

        Ok(sessions)
    }
}
