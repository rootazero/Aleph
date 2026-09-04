//! CRUD operations for `group_chat` tables.
//!
//! Provides database operations for group chat session persistence
//! and conversation turn tracking.

use super::StateDatabase;
use crate::error::AlephError;
use rusqlite::params;

/// A single conversation turn.
#[derive(Debug, Clone)]
pub struct GroupChatTurn {
    pub round: u32,
    pub sequence: u32,
    pub speaker_type: String,
    pub speaker_id: Option<String>,
    pub speaker_name: String,
    pub content: String,
    pub timestamp: i64,
}

/// An active session summary.
impl StateDatabase {
    // =========================================================================
    // Group Chat Sessions CRUD
    // =========================================================================

    /// Insert a new group chat session.
    ///
    /// `owner_user_id` is the P1 ownership stamp captured by
    /// `GroupChatSession::new` from `crate::scope::current_scope()`. Pass `None`
    /// for sessions created outside any dispatch scope — this matches the
    /// in-memory `Option<String>` shape and preserves the documented
    /// operator-default visibility behavior at the DB layer.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_group_chat_session(
        &self,
        id: &str,
        topic: Option<&str>,
        source_channel: &str,
        source_session_key: &str,
        owner_user_id: Option<&str>,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let id = id.to_string();
        let topic = topic.map(str::to_string);
        let source_channel = source_channel.to_string();
        let source_session_key = source_session_key.to_string();
        let owner_user_id = owner_user_id.map(str::to_string);
        self.with_conn(move |conn| {
            conn.execute(
                r#"
                INSERT INTO group_chat_sessions (
                    id, topic, status, source_channel, source_session_key,
                    created_at, updated_at, owner_user_id
                ) VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    id,
                    topic,
                    source_channel,
                    source_session_key,
                    now,
                    now,
                    owner_user_id
                ],
            )
            .map_err(|e| AlephError::config(format!("Failed to insert group chat session: {e}")))?;
            Ok(())
        })
        .await
    }

    /// Update the status of a group chat session.
    pub async fn update_group_chat_session_status(
        &self,
        session_id: &str,
        status: &str,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let session_id = session_id.to_string();
        let status = status.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                r#"
                UPDATE group_chat_sessions
                SET status = ?1, updated_at = ?2
                WHERE id = ?3
                "#,
                params![status, now, session_id],
            )
            .map_err(|e| {
                AlephError::config(format!("Failed to update group chat session status: {e}"))
            })?;
            Ok(())
        })
        .await
    }

    /// Insert a conversation turn into a group chat session.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_group_chat_turn(
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
        let session_id = session_id.to_string();
        let speaker_type = speaker_type.to_string();
        let speaker_id = speaker_id.map(str::to_string);
        let speaker_name = speaker_name.to_string();
        let content = content.to_string();
        self.with_conn(move |conn| {
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
            .map_err(|e| AlephError::config(format!("Failed to insert group chat turn: {e}")))?;
            Ok(())
        })
        .await
    }

    /// Get all turns for a group chat session, ordered by round and sequence.
    pub async fn get_group_chat_turns(
        &self,
        session_id: &str,
    ) -> Result<Vec<GroupChatTurn>, AlephError> {
        let session_id = session_id.to_string();
        self.with_conn(move |conn| {
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
                    AlephError::config(format!("Failed to prepare group chat turns query: {e}"))
                })?;

            let turns = stmt
                .query_map(params![session_id], |row| {
                    Ok(GroupChatTurn {
                        round: row.get::<_, u32>(0)?,
                        sequence: row.get::<_, u32>(1)?,
                        speaker_type: row.get::<_, String>(2)?,
                        speaker_id: row.get::<_, Option<String>>(3)?,
                        speaker_name: row.get::<_, String>(4)?,
                        content: row.get::<_, String>(5)?,
                        timestamp: row.get::<_, i64>(6)?,
                    })
                })
                .map_err(|e| AlephError::config(format!("Failed to query group chat turns: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    AlephError::config(format!("Failed to collect group chat turns: {e}"))
                })?;

            Ok(turns)
        })
        .await
    }
}
