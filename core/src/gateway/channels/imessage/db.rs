//! Messages Database Reader
//!
//! Reads messages from ~/Library/Messages/chat.db (SQLite).
//!
//! # Apple Timestamps
//!
//! Apple uses "Apple Cocoa Core Data timestamp" which is:
//! - Nanoseconds since 2001-01-01 00:00:00 UTC
//! - Stored as INTEGER in SQLite
//!
//! To convert to Unix timestamp:
//! `unix_timestamp = apple_timestamp / 1_000_000_000 + 978307200`

use chrono::{DateTime, Datelike, TimeZone, Utc};
use rusqlite::{params, Connection, Result as SqliteResult};
use std::path::Path;
use tracing::{debug, trace};

use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, UserId,
};

/// Apple epoch offset (2001-01-01 00:00:00 UTC in Unix timestamp)
const APPLE_EPOCH_OFFSET: i64 = 978307200;

/// Convert Apple timestamp to DateTime<Utc>
fn apple_timestamp_to_datetime(apple_ts: i64) -> DateTime<Utc> {
    // Apple timestamps are in nanoseconds since 2001-01-01
    let unix_secs = apple_ts / 1_000_000_000 + APPLE_EPOCH_OFFSET;
    match Utc.timestamp_opt(unix_secs, 0) {
        chrono::LocalResult::Single(dt) => dt,
        _ => Utc::now(),
    }
}

/// Raw message data from the database
#[derive(Debug)]
pub struct RawMessage {
    pub rowid: i64,
    pub guid: String,
    pub text: Option<String>,
    pub handle_id: i64,
    pub date: i64,
    pub is_from_me: bool,
    pub cache_has_attachments: bool,
    pub chat_id: Option<i64>,
}

/// Chat information
#[derive(Debug, Clone)]
pub struct ChatInfo {
    pub rowid: i64,
    pub guid: String,
    pub chat_identifier: String,
    pub display_name: Option<String>,
    pub is_group: bool,
}

/// Handle (contact) information
#[derive(Debug, Clone)]
pub struct Handle {
    pub rowid: i64,
    pub id: String,       // Phone number or email
    pub service: String,  // "iMessage" or "SMS"
}

/// Attachment information
#[derive(Debug, Clone)]
pub struct AttachmentInfo {
    pub rowid: i64,
    pub guid: String,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub total_bytes: i64,
    pub transfer_name: Option<String>,
}

/// Messages database reader
pub struct MessagesDb {
    conn: Connection,
    last_message_rowid: i64,
}

impl MessagesDb {
    /// Open the Messages database
    pub fn open(path: impl AsRef<Path>) -> SqliteResult<Self> {
        let path = path.as_ref();
        debug!("Opening Messages database: {}", path.display());

        // Open in read-only mode
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        // Get the latest message ID to start from
        let last_rowid: i64 = conn
            .query_row("SELECT MAX(ROWID) FROM message", [], |row| row.get(0))
            .unwrap_or(0);

        debug!("Starting from message ROWID: {}", last_rowid);

        Ok(Self {
            conn,
            last_message_rowid: last_rowid,
        })
    }

    /// Poll for new messages since the last poll
    pub fn poll_new_messages(&mut self) -> SqliteResult<Vec<InboundMessage>> {
        let sql = r#"
            SELECT
                m.ROWID,
                m.guid,
                m.text,
                m.handle_id,
                m.date,
                m.is_from_me,
                m.cache_has_attachments,
                cmj.chat_id
            FROM message m
            LEFT JOIN chat_message_join cmj ON m.ROWID = cmj.message_id
            WHERE m.ROWID > ?1
              AND m.is_from_me = 0
            ORDER BY m.ROWID ASC
            LIMIT 100
        "#;

        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params![self.last_message_rowid], |row| {
            Ok(RawMessage {
                rowid: row.get(0)?,
                guid: row.get(1)?,
                text: row.get(2)?,
                handle_id: row.get(3)?,
                date: row.get(4)?,
                is_from_me: row.get::<_, i64>(5)? != 0,
                cache_has_attachments: row.get::<_, i64>(6)? != 0,
                chat_id: row.get(7)?,
            })
        })?;

        let mut messages = Vec::new();

        for row_result in rows {
            let raw = row_result?;

            // Update last seen ROWID
            if raw.rowid > self.last_message_rowid {
                self.last_message_rowid = raw.rowid;
            }

            // Skip empty messages (unless they have attachments)
            if raw.text.is_none() && !raw.cache_has_attachments {
                continue;
            }

            // Get sender info
            let handle = self.get_handle(raw.handle_id)?;

            // Get chat info if available
            let chat_info = if let Some(chat_id) = raw.chat_id {
                self.get_chat_info(chat_id).ok()
            } else {
                None
            };

            // Get attachments if present
            let attachments = if raw.cache_has_attachments {
                self.get_message_attachments(raw.rowid)?
            } else {
                vec![]
            };

            // Determine conversation ID
            let conversation_id = if let Some(ref chat) = chat_info {
                chat.chat_identifier.clone()
            } else {
                handle.id.clone()
            };

            // Determine if this is a group message
            let is_group = chat_info.as_ref().map(|c| c.is_group).unwrap_or(false);

            let inbound = InboundMessage {
                id: MessageId::new(&raw.guid),
                channel_id: ChannelId::new("imessage"),
                conversation_id: ConversationId::new(&conversation_id),
                sender_id: UserId::new(&handle.id),
                sender_name: None, // Would need AddressBook access
                text: raw.text.unwrap_or_default(),
                attachments: attachments
                    .into_iter()
                    .map(|a| Attachment {
                        id: a.guid,
                        mime_type: a.mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
                        filename: a.filename.or(a.transfer_name),
                        size: Some(a.total_bytes as u64),
                        url: None,
                        path: None, // Would need to construct from filename
                        data: None,
                    })
                    .collect(),
                timestamp: apple_timestamp_to_datetime(raw.date),
                reply_to: None,
                is_group,
                raw: None,
            };

            trace!("Parsed message: {:?}", inbound);
            messages.push(inbound);
        }

        if !messages.is_empty() {
            debug!("Found {} new messages", messages.len());
        }

        Ok(messages)
    }

    /// Get handle (contact) by ID
    pub fn get_handle(&self, handle_id: i64) -> SqliteResult<Handle> {
        let sql = "SELECT ROWID, id, service FROM handle WHERE ROWID = ?1";
        self.conn.query_row(sql, params![handle_id], |row| {
            Ok(Handle {
                rowid: row.get(0)?,
                id: row.get(1)?,
                service: row.get(2)?,
            })
        })
    }

    /// Get chat info by ID
    pub fn get_chat_info(&self, chat_id: i64) -> SqliteResult<ChatInfo> {
        let sql = r#"
            SELECT
                ROWID,
                guid,
                chat_identifier,
                display_name,
                group_id
            FROM chat
            WHERE ROWID = ?1
        "#;

        self.conn.query_row(sql, params![chat_id], |row| {
            let group_id: Option<String> = row.get(4)?;
            Ok(ChatInfo {
                rowid: row.get(0)?,
                guid: row.get(1)?,
                chat_identifier: row.get(2)?,
                display_name: row.get(3)?,
                is_group: group_id.is_some(),
            })
        })
    }

    /// Get attachments for a message
    pub fn get_message_attachments(&self, message_id: i64) -> SqliteResult<Vec<AttachmentInfo>> {
        let sql = r#"
            SELECT
                a.ROWID,
                a.guid,
                a.filename,
                a.mime_type,
                a.total_bytes,
                a.transfer_name
            FROM attachment a
            JOIN message_attachment_join maj ON a.ROWID = maj.attachment_id
            WHERE maj.message_id = ?1
        "#;

        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params![message_id], |row| {
            Ok(AttachmentInfo {
                rowid: row.get(0)?,
                guid: row.get(1)?,
                filename: row.get(2)?,
                mime_type: row.get(3)?,
                total_bytes: row.get(4)?,
                transfer_name: row.get(5)?,
            })
        })?;

        rows.collect()
    }

    /// Get all chats
    pub fn list_chats(&self) -> SqliteResult<Vec<ChatInfo>> {
        let sql = r#"
            SELECT
                ROWID,
                guid,
                chat_identifier,
                display_name,
                group_id
            FROM chat
            ORDER BY ROWID DESC
        "#;

        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            let group_id: Option<String> = row.get(4)?;
            Ok(ChatInfo {
                rowid: row.get(0)?,
                guid: row.get(1)?,
                chat_identifier: row.get(2)?,
                display_name: row.get(3)?,
                is_group: group_id.is_some(),
            })
        })?;

        rows.collect()
    }

    /// Get message count
    pub fn message_count(&self) -> SqliteResult<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))
    }

    /// Reset the last message marker (useful for testing)
    pub fn reset_marker(&mut self) {
        self.last_message_rowid = 0;
    }

    /// Set the marker to skip all existing messages
    pub fn skip_existing(&mut self) -> SqliteResult<()> {
        self.last_message_rowid = self
            .conn
            .query_row("SELECT MAX(ROWID) FROM message", [], |row| row.get(0))
            .unwrap_or(0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apple_timestamp_conversion() {
        // Test a known timestamp: 2024-01-01 00:00:00 UTC
        // Apple timestamp: (2024-01-01 - 2001-01-01) in nanoseconds
        let apple_ts: i64 = 725846400_000_000_000; // Approximate
        let dt = apple_timestamp_to_datetime(apple_ts);

        // Should be around 2024
        assert!(dt.year() >= 2023 && dt.year() <= 2025);
    }

    #[test]
    fn test_apple_epoch_offset() {
        // 2001-01-01 00:00:00 UTC = 978307200 Unix timestamp
        assert_eq!(APPLE_EPOCH_OFFSET, 978307200);
    }
}
