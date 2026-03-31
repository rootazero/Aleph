//! SQLite-backed message store for team communication.
//!
//! Provides threaded messaging with TTL-based expiration, recipient tracking,
//! and unread counts. Uses the same `Arc<tokio::sync::Mutex<Connection>>`
//! pattern as the sibling team and artifact stores.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use super::types::{MessageType, NewMessage, Recipient, RecipientRole, TeamMessage};
use crate::error::AlephError;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn db_err(e: impl std::fmt::Display) -> AlephError {
    AlephError::ConfigError {
        message: format!("MessageStore: {e}"),
        suggestion: None,
    }
}

fn parse_rfc3339(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_rfc3339_opt(s: &Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref().map(parse_rfc3339)
}

/// Compute default TTL based on recipient roles and message type.
fn default_ttl(msg_type: &MessageType, recipients: &[Recipient]) -> Duration {
    if *msg_type == MessageType::SystemNotification {
        return Duration::minutes(15);
    }
    let has_to = recipients.iter().any(|r| r.role == RecipientRole::To);
    if has_to {
        Duration::hours(2)
    } else {
        Duration::minutes(30)
    }
}

// ---------------------------------------------------------------------------
// MessageStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for team messages.
#[async_trait]
pub trait MessageStore: Send + Sync {
    /// Send a message with default TTL based on recipient roles.
    async fn send_message(&self, input: NewMessage) -> crate::error::Result<TeamMessage>;

    /// Send a message with an explicit TTL.
    async fn send_message_with_ttl(
        &self,
        input: NewMessage,
        ttl: Duration,
    ) -> crate::error::Result<TeamMessage>;

    /// Read inbox for an agent (unread, non-expired messages), optionally filtered by type.
    async fn read_inbox(
        &self,
        agent_id: &str,
        team_id: &str,
        msg_type: Option<&MessageType>,
    ) -> crate::error::Result<Vec<TeamMessage>>;

    /// Read all unread messages for an agent in a team.
    async fn read_inbox_unread(
        &self,
        agent_id: &str,
        team_id: &str,
    ) -> crate::error::Result<Vec<TeamMessage>>;

    /// Mark a message as read by an agent.
    async fn mark_read(&self, message_id: &str, agent_id: &str) -> crate::error::Result<()>;

    /// Read all messages in a thread, ordered by creation time.
    async fn read_thread(&self, thread_id: &str) -> crate::error::Result<Vec<TeamMessage>>;

    /// Get unread counts as (to_count, cc_count) for an agent in a team.
    async fn get_unread_counts(
        &self,
        agent_id: &str,
        team_id: &str,
    ) -> crate::error::Result<(u32, u32)>;

    /// Count messages in a thread.
    async fn count_thread_messages(&self, thread_id: &str) -> crate::error::Result<u32>;

    /// Expire all pending messages for a team (used on disband).
    async fn expire_all_for_team(&self, team_id: &str) -> crate::error::Result<usize>;
}

// ---------------------------------------------------------------------------
// SqliteMessageStore
// ---------------------------------------------------------------------------

pub struct SqliteMessageStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteMessageStore {
    /// Create a new store wrapping the given connection.
    /// Call [`migrate`] before using the store.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Convenience constructor for tests — opens an in-memory database and migrates.
    #[cfg(test)]
    pub async fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = Self::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    /// Run schema migration — creates the message tables and indices.
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS team_messages (
                id         TEXT PRIMARY KEY,
                team_id    TEXT NOT NULL,
                from_agent TEXT NOT NULL,
                msg_type   TEXT NOT NULL,
                subject    TEXT NOT NULL,
                content    TEXT NOT NULL,
                reply_to   TEXT,
                thread_id  TEXT,
                created_at TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_team_messages_thread
                ON team_messages(thread_id);
            CREATE INDEX IF NOT EXISTS idx_team_messages_team
                ON team_messages(team_id, created_at);

            CREATE TABLE IF NOT EXISTS message_recipients (
                message_id TEXT NOT NULL,
                agent_id   TEXT NOT NULL,
                role       TEXT NOT NULL,
                read_at    TEXT,
                PRIMARY KEY (message_id, agent_id)
            );

            CREATE TABLE IF NOT EXISTS message_attachments (
                message_id  TEXT NOT NULL,
                artifact_id TEXT NOT NULL,
                PRIMARY KEY (message_id, artifact_id)
            );
            "#,
        )
        .map_err(db_err)?;

        Ok(())
    }

    /// Resolve the thread_id for a new message.
    /// If `reply_to` is set, inherit the replied message's thread_id.
    /// Otherwise the message starts a new thread (thread_id = its own id).
    fn resolve_thread_id(
        conn: &Connection,
        message_id: &str,
        reply_to: &Option<String>,
    ) -> Result<String, AlephError> {
        if let Some(ref parent_id) = reply_to {
            let thread_id: Option<String> = conn
                .prepare_cached("SELECT thread_id FROM team_messages WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![parent_id], |row| row.get(0))
                .optional()
                .map_err(db_err)?;

            // If parent exists and has a thread_id, inherit it.
            // Otherwise fall back to using parent_id as thread root.
            Ok(thread_id.unwrap_or_else(|| parent_id.clone()))
        } else {
            // New thread root — thread_id is the message's own id.
            Ok(message_id.to_string())
        }
    }

    /// Internal send implementation shared by both public send methods.
    async fn send_impl(
        &self,
        input: NewMessage,
        ttl: Duration,
    ) -> crate::error::Result<TeamMessage> {
        let conn = self.conn.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let expires_at = now + ttl;
        let expires_at_str = expires_at.to_rfc3339();

        let thread_id = Self::resolve_thread_id(&conn, &id, &input.reply_to)?;

        conn.execute(
            r#"
            INSERT INTO team_messages (id, team_id, from_agent, msg_type, subject, content, reply_to, thread_id, created_at, expires_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                id,
                input.team_id,
                input.from_agent,
                input.msg_type.as_str(),
                input.subject,
                input.content,
                input.reply_to,
                thread_id,
                now_str,
                expires_at_str,
            ],
        )
        .map_err(db_err)?;

        // Insert recipients
        for recipient in &input.recipients {
            conn.execute(
                "INSERT INTO message_recipients (message_id, agent_id, role) VALUES (?1, ?2, ?3)",
                params![id, recipient.agent_id, recipient.role.as_str()],
            )
            .map_err(db_err)?;
        }

        // Insert attachments
        for artifact_id in &input.attachments {
            conn.execute(
                "INSERT INTO message_attachments (message_id, artifact_id) VALUES (?1, ?2)",
                params![id, artifact_id],
            )
            .map_err(db_err)?;
        }

        Ok(TeamMessage {
            id,
            team_id: input.team_id,
            from_agent: input.from_agent,
            msg_type: input.msg_type,
            subject: input.subject,
            content: input.content,
            recipients: input.recipients,
            reply_to: input.reply_to,
            thread_id: Some(thread_id),
            attachments: input.attachments,
            created_at: now,
            expires_at: Some(expires_at),
        })
    }

    /// Read the flat fields from a team_messages row (no sub-queries).
    fn read_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMessage> {
        Ok(RawMessage {
            id: row.get(0)?,
            team_id: row.get(1)?,
            from_agent: row.get(2)?,
            msg_type_str: row.get(3)?,
            subject: row.get(4)?,
            content: row.get(5)?,
            reply_to: row.get(6)?,
            thread_id: row.get(7)?,
            created_at_str: row.get(8)?,
            expires_at_str: row.get(9)?,
        })
    }

    /// Fetch recipients and attachments for a raw message and produce a full TeamMessage.
    fn materialise(conn: &Connection, raw: RawMessage) -> Result<TeamMessage, AlephError> {
        let mut recip_stmt = conn
            .prepare_cached("SELECT agent_id, role FROM message_recipients WHERE message_id = ?1")
            .map_err(db_err)?;
        let recipients = recip_stmt
            .query_map(params![raw.id], |r| {
                let agent_id: String = r.get(0)?;
                let role_str: String = r.get(1)?;
                Ok(Recipient {
                    agent_id,
                    role: RecipientRole::from_stored(&role_str),
                })
            })
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        let mut attach_stmt = conn
            .prepare_cached("SELECT artifact_id FROM message_attachments WHERE message_id = ?1")
            .map_err(db_err)?;
        let attachments = attach_stmt
            .query_map(params![raw.id], |r| r.get::<_, String>(0))
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        Ok(TeamMessage {
            id: raw.id,
            team_id: raw.team_id,
            from_agent: raw.from_agent,
            msg_type: MessageType::from_stored(&raw.msg_type_str),
            subject: raw.subject,
            content: raw.content,
            recipients,
            reply_to: raw.reply_to,
            thread_id: raw.thread_id,
            attachments,
            created_at: parse_rfc3339(&raw.created_at_str),
            expires_at: parse_rfc3339_opt(&raw.expires_at_str),
        })
    }
}

/// Intermediate row data before relation fetching.
struct RawMessage {
    id: String,
    team_id: String,
    from_agent: String,
    msg_type_str: String,
    subject: String,
    content: String,
    reply_to: Option<String>,
    thread_id: Option<String>,
    created_at_str: String,
    expires_at_str: Option<String>,
}

// ---------------------------------------------------------------------------
// MessageStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl MessageStore for SqliteMessageStore {
    async fn send_message(&self, input: NewMessage) -> crate::error::Result<TeamMessage> {
        let ttl = default_ttl(&input.msg_type, &input.recipients);
        self.send_impl(input, ttl).await
    }

    async fn send_message_with_ttl(
        &self,
        input: NewMessage,
        ttl: Duration,
    ) -> crate::error::Result<TeamMessage> {
        self.send_impl(input, ttl).await
    }

    async fn read_inbox(
        &self,
        agent_id: &str,
        team_id: &str,
        msg_type: Option<&MessageType>,
    ) -> crate::error::Result<Vec<TeamMessage>> {
        let conn = self.conn.lock().await;
        let now_str = Utc::now().to_rfc3339();

        // Collect message IDs first, then materialise each one.
        // This avoids closure type issues with conditional query_map.
        let ids: Vec<String> = if let Some(mt) = msg_type {
            let mut stmt = conn
                .prepare_cached(
                    r#"
                    SELECT m.id
                    FROM team_messages m
                    JOIN message_recipients r ON r.message_id = m.id
                    WHERE r.agent_id = ?1
                      AND m.team_id = ?2
                      AND r.read_at IS NULL
                      AND (m.expires_at IS NULL OR m.expires_at > ?3)
                      AND m.msg_type = ?4
                    ORDER BY m.created_at ASC
                    "#,
                )
                .map_err(db_err)?;
            let type_str = mt.as_str();
            let result = stmt
                .query_map(params![agent_id, team_id, now_str, type_str], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(db_err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db_err)?;
            result
        } else {
            let mut stmt = conn
                .prepare_cached(
                    r#"
                    SELECT m.id
                    FROM team_messages m
                    JOIN message_recipients r ON r.message_id = m.id
                    WHERE r.agent_id = ?1
                      AND m.team_id = ?2
                      AND r.read_at IS NULL
                      AND (m.expires_at IS NULL OR m.expires_at > ?3)
                    ORDER BY m.created_at ASC
                    "#,
                )
                .map_err(db_err)?;
            let result = stmt
                .query_map(params![agent_id, team_id, now_str], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(db_err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db_err)?;
            result
        };

        // Materialise each message with its relations.
        // NOTE: This is an N+1 query pattern (2 sub-queries per message for recipients
        // and attachments). This is acceptable because team inboxes are small (typically
        // <50 unread messages) and all queries hit the same locked SQLite connection
        // with prepare_cached, so overhead is minimal. Batching with IN(...) clauses
        // would add complexity without meaningful benefit at this scale.
        let mut messages = Vec::with_capacity(ids.len());
        for id in &ids {
            let raw = conn
                .prepare_cached(
                    r#"
                    SELECT id, team_id, from_agent, msg_type, subject, content,
                           reply_to, thread_id, created_at, expires_at
                    FROM team_messages WHERE id = ?1
                    "#,
                )
                .map_err(db_err)?
                .query_row(params![id], Self::read_message_row)
                .map_err(db_err)?;
            messages.push(Self::materialise(&conn, raw)?);
        }

        Ok(messages)
    }

    async fn read_inbox_unread(
        &self,
        agent_id: &str,
        team_id: &str,
    ) -> crate::error::Result<Vec<TeamMessage>> {
        self.read_inbox(agent_id, team_id, None).await
    }

    async fn mark_read(&self, message_id: &str, agent_id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        let now_str = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE message_recipients SET read_at = ?1 WHERE message_id = ?2 AND agent_id = ?3",
            params![now_str, message_id, agent_id],
        )
        .map_err(db_err)?;

        Ok(())
    }

    async fn read_thread(&self, thread_id: &str) -> crate::error::Result<Vec<TeamMessage>> {
        let conn = self.conn.lock().await;

        // Phase 1: read raw rows
        let raws: Vec<RawMessage> = {
            let mut stmt = conn
                .prepare_cached(
                    r#"
                    SELECT id, team_id, from_agent, msg_type, subject, content,
                           reply_to, thread_id, created_at, expires_at
                    FROM team_messages
                    WHERE thread_id = ?1
                    ORDER BY created_at ASC
                    "#,
                )
                .map_err(db_err)?;
            let result = stmt
                .query_map(params![thread_id], Self::read_message_row)
                .map_err(db_err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db_err)?;
            result
        };

        // Phase 2: materialise with relations (N+1 pattern — see read_inbox comment)
        let mut messages = Vec::with_capacity(raws.len());
        for raw in raws {
            messages.push(Self::materialise(&conn, raw)?);
        }

        Ok(messages)
    }

    async fn get_unread_counts(
        &self,
        agent_id: &str,
        team_id: &str,
    ) -> crate::error::Result<(u32, u32)> {
        let conn = self.conn.lock().await;
        let now_str = Utc::now().to_rfc3339();

        let mut stmt = conn
            .prepare_cached(
                r#"
                SELECT r.role, COUNT(*) as cnt
                FROM message_recipients r
                JOIN team_messages m ON m.id = r.message_id
                WHERE r.agent_id = ?1
                  AND m.team_id = ?2
                  AND r.read_at IS NULL
                  AND (m.expires_at IS NULL OR m.expires_at > ?3)
                GROUP BY r.role
                "#,
            )
            .map_err(db_err)?;

        let mut to_count: u32 = 0;
        let mut cc_count: u32 = 0;

        let rows = stmt
            .query_map(params![agent_id, team_id, now_str], |row| {
                let role: String = row.get(0)?;
                let count: u32 = row.get(1)?;
                Ok((role, count))
            })
            .map_err(db_err)?;

        for row in rows {
            let (role, count) = row.map_err(db_err)?;
            match role.as_str() {
                "to" => to_count = count,
                "cc" => cc_count = count,
                _ => {}
            }
        }

        Ok((to_count, cc_count))
    }

    async fn count_thread_messages(&self, thread_id: &str) -> crate::error::Result<u32> {
        let conn = self.conn.lock().await;

        let count: u32 = conn
            .prepare_cached("SELECT COUNT(*) FROM team_messages WHERE thread_id = ?1")
            .map_err(db_err)?
            .query_row(params![thread_id], |row| row.get(0))
            .map_err(db_err)?;

        Ok(count)
    }

    async fn expire_all_for_team(&self, team_id: &str) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        let now_str = Utc::now().to_rfc3339();

        let affected = conn
            .execute(
                "UPDATE team_messages SET expires_at = ?1 WHERE team_id = ?2 AND (expires_at IS NULL OR expires_at > ?1)",
                params![now_str, team_id],
            )
            .map_err(db_err)?;

        Ok(affected)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::messages::types::*;

    fn make_msg(team_id: &str, from: &str, recipients: Vec<Recipient>) -> NewMessage {
        NewMessage {
            team_id: team_id.to_string(),
            from_agent: from.to_string(),
            msg_type: MessageType::Message,
            subject: "Test subject".to_string(),
            content: "Test content".to_string(),
            recipients,
            reply_to: None,
            attachments: vec![],
        }
    }

    fn to_recipient(agent_id: &str) -> Recipient {
        Recipient {
            agent_id: agent_id.to_string(),
            role: RecipientRole::To,
        }
    }

    fn cc_recipient(agent_id: &str) -> Recipient {
        Recipient {
            agent_id: agent_id.to_string(),
            role: RecipientRole::Cc,
        }
    }

    #[tokio::test]
    async fn test_send_and_read_message() {
        let store = SqliteMessageStore::new_in_memory().await;

        let msg = store
            .send_message(make_msg(
                "team-1",
                "agent-a",
                vec![to_recipient("agent-b"), cc_recipient("agent-c")],
            ))
            .await
            .unwrap();

        assert!(!msg.id.is_empty());
        assert_eq!(msg.team_id, "team-1");
        assert_eq!(msg.from_agent, "agent-a");
        assert_eq!(msg.recipients.len(), 2);
        assert!(msg.thread_id.is_some());
        // New message — thread_id equals its own id
        assert_eq!(msg.thread_id.as_deref(), Some(msg.id.as_str()));

        // agent-b should see it in inbox
        let inbox_b = store.read_inbox("agent-b", "team-1", None).await.unwrap();
        assert_eq!(inbox_b.len(), 1);
        assert_eq!(inbox_b[0].id, msg.id);

        // agent-c (cc) should also see it
        let inbox_c = store.read_inbox("agent-c", "team-1", None).await.unwrap();
        assert_eq!(inbox_c.len(), 1);

        // agent-a (sender, not a recipient) should not see it
        let inbox_a = store.read_inbox("agent-a", "team-1", None).await.unwrap();
        assert!(inbox_a.is_empty());
    }

    #[tokio::test]
    async fn test_thread_continuation() {
        let store = SqliteMessageStore::new_in_memory().await;

        // First message starts a new thread
        let msg1 = store
            .send_message(make_msg("team-1", "agent-a", vec![to_recipient("agent-b")]))
            .await
            .unwrap();

        let thread_id = msg1.thread_id.clone().unwrap();

        // Reply inherits thread_id
        let reply = store
            .send_message(NewMessage {
                team_id: "team-1".to_string(),
                from_agent: "agent-b".to_string(),
                msg_type: MessageType::Message,
                subject: "Re: Test subject".to_string(),
                content: "Reply content".to_string(),
                recipients: vec![to_recipient("agent-a")],
                reply_to: Some(msg1.id.clone()),
                attachments: vec![],
            })
            .await
            .unwrap();

        assert_eq!(reply.thread_id.as_deref(), Some(thread_id.as_str()));
        assert_ne!(reply.id, msg1.id);

        // Thread should contain both messages
        let thread = store.read_thread(&thread_id).await.unwrap();
        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].id, msg1.id);
        assert_eq!(thread[1].id, reply.id);

        // Thread count
        let count = store.count_thread_messages(&thread_id).await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_message_expiry() {
        let store = SqliteMessageStore::new_in_memory().await;

        // Send with zero TTL — should already be expired
        let msg = store
            .send_message_with_ttl(
                make_msg("team-1", "agent-a", vec![to_recipient("agent-b")]),
                Duration::zero(),
            )
            .await
            .unwrap();

        assert!(!msg.id.is_empty());

        // Expired messages should not appear in inbox
        let inbox = store.read_inbox("agent-b", "team-1", None).await.unwrap();
        assert!(inbox.is_empty());
    }

    #[tokio::test]
    async fn test_mark_read() {
        let store = SqliteMessageStore::new_in_memory().await;

        let msg = store
            .send_message(make_msg("team-1", "agent-a", vec![to_recipient("agent-b")]))
            .await
            .unwrap();

        // Before marking read
        let inbox = store.read_inbox("agent-b", "team-1", None).await.unwrap();
        assert_eq!(inbox.len(), 1);

        // Mark read
        store.mark_read(&msg.id, "agent-b").await.unwrap();

        // After marking read — inbox should be empty
        let inbox = store.read_inbox("agent-b", "team-1", None).await.unwrap();
        assert!(inbox.is_empty());
    }

    #[tokio::test]
    async fn test_unread_counts() {
        let store = SqliteMessageStore::new_in_memory().await;

        // Send two messages: one TO, one CC
        store
            .send_message(make_msg("team-1", "agent-a", vec![to_recipient("agent-b")]))
            .await
            .unwrap();

        store
            .send_message(NewMessage {
                team_id: "team-1".to_string(),
                from_agent: "agent-c".to_string(),
                msg_type: MessageType::Discovery,
                subject: "FYI".to_string(),
                content: "Just a heads up".to_string(),
                recipients: vec![cc_recipient("agent-b")],
                reply_to: None,
                attachments: vec![],
            })
            .await
            .unwrap();

        let (to_count, cc_count) = store.get_unread_counts("agent-b", "team-1").await.unwrap();
        assert_eq!(to_count, 1);
        assert_eq!(cc_count, 1);
    }

    #[tokio::test]
    async fn test_expire_all_for_team() {
        let store = SqliteMessageStore::new_in_memory().await;

        store
            .send_message(make_msg("team-1", "agent-a", vec![to_recipient("agent-b")]))
            .await
            .unwrap();

        store
            .send_message(make_msg("team-1", "agent-c", vec![to_recipient("agent-b")]))
            .await
            .unwrap();

        // Expire all
        let expired = store.expire_all_for_team("team-1").await.unwrap();
        assert_eq!(expired, 2);

        // Inbox should be empty now
        let inbox = store.read_inbox("agent-b", "team-1", None).await.unwrap();
        assert!(inbox.is_empty());
    }

    #[tokio::test]
    async fn test_inbox_filter_by_type() {
        let store = SqliteMessageStore::new_in_memory().await;

        store
            .send_message(make_msg("team-1", "agent-a", vec![to_recipient("agent-b")]))
            .await
            .unwrap();

        store
            .send_message(NewMessage {
                team_id: "team-1".to_string(),
                from_agent: "agent-c".to_string(),
                msg_type: MessageType::Discovery,
                subject: "Found something".to_string(),
                content: "Discovery content".to_string(),
                recipients: vec![to_recipient("agent-b")],
                reply_to: None,
                attachments: vec![],
            })
            .await
            .unwrap();

        // Filter by Discovery
        let discoveries = store
            .read_inbox("agent-b", "team-1", Some(&MessageType::Discovery))
            .await
            .unwrap();
        assert_eq!(discoveries.len(), 1);
        assert_eq!(discoveries[0].msg_type, MessageType::Discovery);

        // Filter by Message
        let messages = store
            .read_inbox("agent-b", "team-1", Some(&MessageType::Message))
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].msg_type, MessageType::Message);
    }

    #[tokio::test]
    async fn test_attachments_roundtrip() {
        let store = SqliteMessageStore::new_in_memory().await;

        let msg = store
            .send_message(NewMessage {
                team_id: "team-1".to_string(),
                from_agent: "agent-a".to_string(),
                msg_type: MessageType::ReviewRequest,
                subject: "Please review".to_string(),
                content: "See attached".to_string(),
                recipients: vec![to_recipient("agent-b")],
                reply_to: None,
                attachments: vec!["artifact-1".to_string(), "artifact-2".to_string()],
            })
            .await
            .unwrap();

        assert_eq!(msg.attachments.len(), 2);

        // Read from inbox and verify attachments are materialised
        let inbox = store.read_inbox("agent-b", "team-1", None).await.unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].attachments.len(), 2);
        assert!(inbox[0].attachments.contains(&"artifact-1".to_string()));
        assert!(inbox[0].attachments.contains(&"artifact-2".to_string()));
    }
}
