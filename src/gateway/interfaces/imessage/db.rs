//! Messages Database Reader
//!
//! Reads messages from ~/Library/Messages/chat.db (`SQLite`).
//!
//! # Apple Timestamps
//!
//! Apple uses "Apple Cocoa Core Data timestamp" which is:
//! - Nanoseconds since 2001-01-01 00:00:00 UTC
//! - Stored as INTEGER in `SQLite`
//!
//! To convert to Unix timestamp:
//! `unix_timestamp = apple_timestamp / 1_000_000_000 + 978307200`

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, Result as SqliteResult};
use std::path::{Path, PathBuf};
use tracing::{debug, trace, warn};

use super::reaction;
use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, MessageMeta, UserId,
};
use crate::utils::sqlite_open::open_sqlite_readonly;

/// Apple epoch offset (2001-01-01 00:00:00 UTC in Unix timestamp)
const APPLE_EPOCH_OFFSET: i64 = 978307200;

/// Convert Apple timestamp to `DateTime`<Utc>
fn apple_timestamp_to_datetime(apple_ts: i64) -> DateTime<Utc> {
    // Apple timestamps are in nanoseconds since 2001-01-01
    let unix_secs = apple_ts / 1_000_000_000 + APPLE_EPOCH_OFFSET;
    match Utc.timestamp_opt(unix_secs, 0) {
        chrono::LocalResult::Single(dt) => dt,
        _ => Utc::now(),
    }
}

/// Extract the target message GUID a tapback refers to. chat.db stores
/// `associated_message_guid` with a part prefix — `p:0/<GUID>` (indexed part) or
/// `bp:<GUID>` — so strip it down to the bare GUID. Returns `None` if empty.
fn reaction_target_guid(assoc: &str) -> Option<String> {
    let guid = if let Some((_, rest)) = assoc.rsplit_once('/') {
        rest
    } else {
        assoc
            .strip_prefix("bp:")
            .or_else(|| assoc.strip_prefix("p:"))
            .unwrap_or(assoc)
    };
    (!guid.is_empty()).then(|| guid.to_string())
}

/// Resolve a raw `attachment.filename` value (an absolute path, often with a
/// literal `~`) to an existing on-disk path. Returns `None` if the file is
/// missing or the home directory can't be resolved.
fn resolve_attachment_path(raw: &str) -> Option<PathBuf> {
    let path = if let Some(stripped) = raw.strip_prefix("~/") {
        dirs::home_dir()?.join(stripped)
    } else {
        PathBuf::from(raw)
    };
    path.exists().then_some(path)
}

/// Resolve a raw `attachment.filename` to an existing path that lives UNDER
/// `root`.
///
/// `attachment.filename` is a value read out of `chat.db`, not a path this
/// process produced: it names a file on this machine, and whatever it points
/// at is read and handed to the model. A path outside the Messages directory
/// is therefore not an attachment — it is an attempt to pull a local file
/// into the conversation.
///
/// The comparison runs on canonicalized paths, so a symlink inside the root
/// that points outside of it is rejected as well. A missing root resolves
/// nothing (fail closed).
fn resolve_contained_attachment(root: Option<&Path>, raw: &str) -> Option<PathBuf> {
    let root = root?;
    let canonical = resolve_attachment_path(raw)?.canonicalize().ok()?;
    if !canonical.starts_with(root) {
        debug!(
            path = %canonical.display(),
            root = %root.display(),
            "Refusing attachment outside the Messages directory"
        );
        return None;
    }
    Some(canonical)
}

/// Raw message data from the database
#[derive(Debug)]
pub struct RawMessage {
    pub rowid: i64,
    pub guid: String,
    pub text: Option<String>,
    pub handle_id: i64,
    pub date: i64,
    pub cache_has_attachments: bool,
    pub chat_id: Option<i64>,
    /// `associated_message_type`: 0 for a normal message, 2000–2005 for an
    /// add-tapback (removes 3000–3005 are excluded by the query).
    pub associated_message_type: i64,
    /// `associated_message_guid`: the reacted-to message (tapback rows only).
    pub associated_message_guid: Option<String>,
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
    pub id: String,      // Phone number or email
    pub service: String, // "iMessage" or "SMS"
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
    /// Whether inbound attachments should be resolved to on-disk paths.
    include_attachments: bool,
    /// Maximum attachment size in bytes to resolve a local path for
    /// (0 = unlimited). Oversized attachments keep their metadata but omit
    /// the local path so the agent is not forced to load a huge file.
    max_attachment_size: u64,
    /// Canonicalized directory the attachments must live under — the
    /// directory that holds `chat.db` (`~/Library/Messages` in the default
    /// setup, whose `Attachments/` subdirectory holds the files).
    ///
    /// `None` means "no attachment may be resolved": the containment root is
    /// unknown, so the answer is no.
    attachments_root: Option<PathBuf>,
}

impl MessagesDb {
    /// Open the Messages database
    pub fn open(path: impl AsRef<Path>) -> SqliteResult<Self> {
        let path = path.as_ref();
        debug!("Opening Messages database: {}", path.display());

        // Open in read-only mode
        let conn = open_sqlite_readonly(path)?;

        // Get the latest message ID to start from
        let last_rowid: i64 = conn
            .query_row("SELECT MAX(ROWID) FROM message", [], |row| row.get(0))
            .unwrap_or(0);

        debug!("Starting from message ROWID: {}", last_rowid);

        // Attachments must live under the directory that holds `chat.db`
        // (`~/Library/Messages`, whose `Attachments/` subdirectory holds the
        // files). A missing or unresolvable directory leaves the root unset,
        // which disables attachment resolution entirely (fail closed).
        let attachments_root = path.parent().and_then(|dir| dir.canonicalize().ok());

        Ok(Self {
            conn,
            last_message_rowid: last_rowid,
            include_attachments: true,
            max_attachment_size: 0,
            attachments_root,
        })
    }

    /// Test-only constructor over an already-open connection (e.g. in-memory),
    /// so the polling/filtering logic can be exercised without a real chat.db.
    #[cfg(test)]
    fn from_conn(conn: Connection) -> Self {
        Self {
            conn,
            last_message_rowid: 0,
            include_attachments: true,
            max_attachment_size: 0,
            // No database path means no containment root — no attachment is
            // resolved.
            attachments_root: None,
        }
    }

    /// Configure attachment handling (from `IMessageConfig`).
    pub const fn set_attachment_policy(
        &mut self,
        include_attachments: bool,
        max_attachment_size: u64,
    ) {
        self.include_attachments = include_attachments;
        self.max_attachment_size = max_attachment_size;
    }

    /// Override the polling watermark so the next `poll_new_messages` resumes
    /// from messages with `ROWID > rowid`. Used to recover messages received
    /// while the daemon was offline (catch-up), restoring from a persisted
    /// cursor instead of skipping straight to the newest message.
    pub const fn resume_from_rowid(&mut self, rowid: i64) {
        self.last_message_rowid = rowid;
    }

    /// The highest message ROWID processed so far (the catch-up watermark).
    pub const fn last_rowid(&self) -> i64 {
        self.last_message_rowid
    }

    /// Poll for new messages since the last poll.
    ///
    /// The `item_type = 0` filter drops group-action system rows (participant
    /// added/removed, group renamed, etc.). The `associated_message_type`
    /// predicate keeps normal messages (type 0) and *add* tapbacks (2000–2005),
    /// while excluding *remove* tapbacks (3000–3005). Add tapbacks surface as
    /// reaction messages (`Reacted with: …` + [`MessageMeta::Reaction`]),
    /// matching the BlueBubbles path; removes and system rows are dropped so they
    /// don't arrive as junk text (e.g. `You named the conversation …`) and
    /// trigger spurious agent turns.
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
                cmj.chat_id,
                m.associated_message_type,
                m.associated_message_guid
            FROM message m
            LEFT JOIN chat_message_join cmj ON m.ROWID = cmj.message_id
            WHERE m.ROWID > ?1
              AND m.is_from_me = 0
              AND (m.associated_message_type = 0
                   OR m.associated_message_type BETWEEN 2000 AND 2005)
              AND m.item_type = 0
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
                cache_has_attachments: row.get::<_, i64>(6)? != 0,
                chat_id: row.get(7)?,
                associated_message_type: row.get(8)?,
                associated_message_guid: row.get(9)?,
            })
        })?;

        let mut messages = Vec::new();

        // Hoisted out of the message loop so the attachment closure below
        // borrows no part of `self`.
        let max_attachment_size = self.max_attachment_size;
        let attachments_root = self.attachments_root.clone();

        let mut new_watermark = self.last_message_rowid;

        for row_result in rows {
            let raw = match row_result {
                Ok(r) => r,
                Err(e) => {
                    warn!("Message decode failed after ROWID {new_watermark}: {e}");
                    break; // Deliver the valid prefix; retry this row next poll.
                }
            };
            // Keep the prior cursor available if routing cannot be trusted.
            let prior_watermark = new_watermark;
            if raw.rowid > new_watermark {
                new_watermark = raw.rowid;
            }

            // A non-zero associated_message_type is an add-tapback (the query
            // already excludes removes 3000–3005 and system rows). Map it to its
            // emoji so it surfaces as a reaction; a row the filter let through
            // with an unmapped type falls back to normal handling.
            let reaction_emoji = if raw.associated_message_type == 0 {
                None
            } else {
                reaction::tapback_emoji(raw.associated_message_type)
            };

            // Skip empty messages (unless they have attachments). Reactions carry
            // synthesized text, so they are never treated as empty.
            if reaction_emoji.is_none() && raw.text.is_none() && !raw.cache_has_attachments {
                continue;
            }

            // Get sender info. A failed lookup (e.g. handle_id = 0 rows in
            // chat.db) must not abort the whole batch: messages collected so
            // far would be dropped by the caller while the watermark has
            // already advanced past them, losing them permanently.
            let handle = match self.get_handle(raw.handle_id) {
                Ok(h) => h,
                Err(e) => {
                    warn!(
                        "Skipping message ROWID {} (handle {} lookup failed: {})",
                        raw.rowid, raw.handle_id, e
                    );
                    continue;
                }
            };

            // Distinguish "no chat_id" (a genuine DM) from "chat_id present but
            // lookup failed". The latter must not be downgraded to a DM route:
            // a group message misrouted to a single contact would leak the
            // reply to the wrong recipient. Roll the watermark back and retry
            // the batch later instead of consuming an untrusted route.
            let chat_info = if let Some(chat_id) = raw.chat_id {
                match self.get_chat_info(chat_id) {
                    Ok(info) => Some(info),
                    Err(e) => {
                        warn!(
                            "Refusing to route message ROWID {} (chat {} lookup failed: {})",
                            raw.rowid, chat_id, e
                        );
                        new_watermark = prior_watermark;
                        break;
                    }
                }
            } else {
                None
            };

            // Get attachments if present (and enabled). Degrade to no
            // attachments on error instead of dropping the whole batch. Reaction
            // rows never carry attachments, so skip the lookup for them.
            let attachments = if raw.cache_has_attachments
                && self.include_attachments
                && reaction_emoji.is_none()
            {
                match self.get_message_attachments(raw.rowid) {
                    Ok(a) => a,
                    Err(e) => {
                        warn!(
                            "Attachment lookup failed for message ROWID {}: {}",
                            raw.rowid, e
                        );
                        vec![]
                    }
                }
            } else {
                vec![]
            };

            // Determine conversation ID
            let conversation_id = if let Some(ref chat) = chat_info {
                format!("chat_guid:{}", chat.guid)
            } else {
                handle.id.clone()
            };

            // Determine if this is a group message
            let is_group = chat_info.as_ref().is_some_and(|c| c.is_group);

            // Render an add-tapback like the Telegram/Matrix reaction convention:
            // `Reacted with: <emoji>` + a Reaction meta, pointed at the reacted-to
            // message. Normal rows keep their own text and carry no reaction meta.
            let (text, reply_to, metadata) = match reaction_emoji {
                Some(emoji) => (
                    format!("Reacted with: {emoji}"),
                    raw.associated_message_guid
                        .as_deref()
                        .and_then(reaction_target_guid)
                        .map(MessageId::new),
                    vec![MessageMeta::Reaction {
                        emojis: vec![emoji.to_string()],
                    }],
                ),
                None => (raw.text.clone().unwrap_or_default(), None, vec![]),
            };

            let inbound = InboundMessage {
                id: MessageId::new(&raw.guid),
                channel_id: ChannelId::new("imessage"),
                conversation_id: ConversationId::new(&conversation_id),
                sender_id: UserId::new(&handle.id),
                sender_name: None, // Would need AddressBook access
                text,
                attachments: attachments
                    .into_iter()
                    .map(|a| {
                        // chat.db stores the absolute on-disk path (often with a
                        // literal `~`) in `attachment.filename`; `transfer_name`
                        // is the user-facing basename. Resolve the real path so
                        // the agent can actually read the file, honoring the
                        // configured size cap (0 = unlimited).
                        let within_size = max_attachment_size == 0
                            || (a.total_bytes as u64) <= max_attachment_size;
                        let resolved_path = if within_size {
                            a.filename.as_deref().and_then(|raw| {
                                resolve_contained_attachment(attachments_root.as_deref(), raw)
                            })
                        } else {
                            None
                        };
                        let display_name = a.transfer_name.clone().or_else(|| {
                            a.filename.as_ref().and_then(|f| {
                                Path::new(f)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                            })
                        });
                        Attachment {
                            id: a.guid,
                            mime_type: a
                                .mime_type
                                .unwrap_or_else(|| "application/octet-stream".to_string()),
                            filename: display_name,
                            size: Some(a.total_bytes as u64),
                            url: None,
                            path: resolved_path.map(|p| p.to_string_lossy().into_owned()),
                            data: None,
                            is_voice_note: false,
                            is_video_note: false,
                        }
                    })
                    .collect(),
                timestamp: apple_timestamp_to_datetime(raw.date),
                reply_to,
                is_group,
                raw: None,
                metadata,
            };

            trace!("Parsed message: {:?}", inbound);
            messages.push(inbound);
        }

        if !messages.is_empty() {
            debug!("Found {} new messages", messages.len());
        }

        self.last_message_rowid = new_watermark;
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
    pub const fn reset_marker(&mut self) {
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
    use chrono::Datelike;

    #[test]
    fn test_apple_timestamp_conversion() {
        // Test a known timestamp: 2024-01-01 00:00:00 UTC
        // Apple timestamp: (2024-01-01 - 2001-01-01) in nanoseconds
        let apple_ts: i64 = 725_846_400_000_000_000; // Approximate
        let dt = apple_timestamp_to_datetime(apple_ts);

        // Should be around 2024
        assert!(dt.year() >= 2023 && dt.year() <= 2025);
    }

    #[test]
    fn test_apple_epoch_offset() {
        // 2001-01-01 00:00:00 UTC = 978307200 Unix timestamp
        assert_eq!(APPLE_EPOCH_OFFSET, 978307200);
    }

    #[test]
    fn test_resolve_attachment_path_existing() {
        // An existing file resolves to its absolute path.
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("attachment.bin");
        std::fs::write(&file, b"x").unwrap();

        let resolved = resolve_attachment_path(&file.to_string_lossy());
        assert_eq!(resolved.as_deref(), Some(file.as_path()));
    }

    #[test]
    fn test_resolve_attachment_path_missing() {
        // A non-existent path resolves to None (don't hand the agent a dead path).
        assert!(resolve_attachment_path("/nonexistent/aleph/imsg/attachment.heic").is_none());
    }

    #[test]
    fn test_attachment_outside_the_messages_root_is_refused() {
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("messages");
        std::fs::create_dir_all(&root).unwrap();
        let inside = root.join("attachment.bin");
        std::fs::write(&inside, b"x").unwrap();
        let outside = dir.join("outside.bin");
        std::fs::write(&outside, b"x").unwrap();

        let canonical_root = root.canonicalize().unwrap();
        assert!(
            resolve_contained_attachment(Some(canonical_root.as_path()), &inside.to_string_lossy())
                .is_some(),
            "a file inside the Messages directory resolves"
        );
        assert!(
            resolve_contained_attachment(
                Some(canonical_root.as_path()),
                &outside.to_string_lossy()
            )
            .is_none(),
            "a file outside the Messages directory does not resolve"
        );
        assert!(
            resolve_contained_attachment(None, &inside.to_string_lossy()).is_none(),
            "without a containment root nothing resolves"
        );
    }

    /// Build a minimal in-memory chat.db subset with the columns
    /// `poll_new_messages` reads, so filtering can be verified end-to-end.
    fn seed_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT, service TEXT);
            CREATE TABLE chat_message_join (message_id INTEGER, chat_id INTEGER);
            CREATE TABLE message (
                ROWID INTEGER PRIMARY KEY,
                guid TEXT, text TEXT, handle_id INTEGER, date INTEGER,
                is_from_me INTEGER, cache_has_attachments INTEGER,
                associated_message_type INTEGER, item_type INTEGER,
                associated_message_guid TEXT
            );
            INSERT INTO handle (ROWID, id, service) VALUES (1, '+15551234567', 'iMessage');
            -- normal inbound message (kept)
            INSERT INTO message VALUES (10,'g-normal','hello',1,0,0,0,0,0,NULL);
            -- add-tapback love (kept: surfaces as a reaction on g-normal)
            INSERT INTO message VALUES (11,'g-love','Loved "hello"',1,0,0,0,2000,0,'p:0/g-normal');
            -- group-action system row (dropped: item_type != 0)
            INSERT INTO message VALUES (12,'g-sysevt','You named the conversation',1,0,0,0,0,1,NULL);
            -- outbound echo (dropped: is_from_me = 1)
            INSERT INTO message VALUES (13,'g-fromme','my own reply',1,0,1,0,0,0,NULL);
            -- remove-tapback (dropped: type 3000 outside 2000–2005)
            INSERT INTO message VALUES (14,'g-unlove','Removed a heart',1,0,0,0,3000,0,'p:0/g-normal');
            "#,
        )
        .unwrap();
        conn
    }

    #[test]
    fn unresolved_existing_chat_does_not_become_dm_or_advance_watermark() {
        let conn = seed_db();
        conn.execute("INSERT INTO chat_message_join VALUES (11, 99)", [])
            .unwrap();
        let mut db = MessagesDb::from_conn(conn);
        let msgs = db.poll_new_messages().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id.as_str(), "g-normal");
        assert_eq!(db.last_rowid(), 10);
        assert!(db.poll_new_messages().unwrap().is_empty());
        assert_eq!(db.last_rowid(), 10);
    }

    #[test]
    fn poll_surfaces_add_tapbacks_drops_removes_system_self() {
        let mut db = MessagesDb::from_conn(seed_db());
        let msgs = db.poll_new_messages().unwrap();
        // The normal message and the add-tapback survive; system / self / remove drop.
        assert_eq!(msgs.len(), 2, "expected normal + reaction, got {msgs:?}");

        assert_eq!(msgs[0].text, "hello");
        assert_eq!(msgs[0].id.as_str(), "g-normal");
        assert!(msgs[0].metadata.is_empty());

        // The add-tapback is rendered as a reaction pointed at the reacted-to
        // message (the `p:0/` part prefix stripped down to the bare GUID).
        assert_eq!(msgs[1].text, "Reacted with: ❤️");
        assert_eq!(msgs[1].id.as_str(), "g-love");
        assert_eq!(
            msgs[1].reply_to.as_ref().map(|r| r.as_str()),
            Some("g-normal")
        );
        assert!(matches!(
            msgs[1].metadata.as_slice(),
            [MessageMeta::Reaction { .. }]
        ));

        // The watermark tracks the last *matched* row (11): the trailing system,
        // self, and remove-tapback rows are excluded by the WHERE clause and so
        // never advance it. Safe because SQLite applies the WHERE filter before
        // LIMIT, so a later real message is still returned even when filtered rows
        // sit in between.
        assert_eq!(db.last_rowid(), 11);
    }

    #[test]
    fn reaction_target_guid_strips_part_prefix() {
        assert_eq!(
            reaction_target_guid("p:0/ABC-123"),
            Some("ABC-123".to_string())
        );
        assert_eq!(
            reaction_target_guid("bp:ABC-123"),
            Some("ABC-123".to_string())
        );
        assert_eq!(reaction_target_guid("ABC-123"), Some("ABC-123".to_string()));
        assert_eq!(reaction_target_guid(""), None);
    }

    #[test]
    fn test_resolve_attachment_path_tilde_expands() {
        // `~/` is expanded to the home dir; missing file still yields None but
        // the prefix must not survive verbatim.
        let resolved = resolve_attachment_path("~/this_should_not_exist_aleph_imsg");
        assert!(resolved.is_none());
        if let Some(home) = dirs::home_dir() {
            // Sanity: a path that does exist under home resolves.
            let resolved_home = resolve_attachment_path(&home.to_string_lossy());
            assert_eq!(resolved_home.as_deref(), Some(home.as_path()));
        }
    }
}
