# iMessage Gateway Integration - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Connect iMessage channel to the Gateway, enabling end-to-end message flow from iMessage to Agent execution and back.

**Architecture:** InboundMessageRouter consumes ChannelRegistry's inbound stream, checks permissions, resolves SessionKey, executes Agent via ExecutionEngine, and routes replies back through the Channel.

**Tech Stack:** Rust, tokio, async-trait, SQLite (rusqlite), serde, uuid

---

## Task 1: Create InboundContext Types

**Files:**
- Create: `core/src/gateway/inbound_context.rs`
- Modify: `core/src/gateway/mod.rs` (add module)

**Step 1: Write the failing test**

Create file `core/src/gateway/inbound_context.rs`:

```rust
//! Inbound Message Context
//!
//! Carries routing information through the entire message processing flow.

use serde::{Deserialize, Serialize};

use super::channel::{ChannelId, ConversationId, InboundMessage, MessageId};
use super::router::SessionKey;

/// Route information for sending replies back to the originating conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyRoute {
    /// Channel to send reply through
    pub channel_id: ChannelId,
    /// Conversation to send reply to
    pub conversation_id: ConversationId,
    /// Optional: reply to specific message
    pub reply_to: Option<MessageId>,
}

impl ReplyRoute {
    /// Create a new reply route
    pub fn new(channel_id: ChannelId, conversation_id: ConversationId) -> Self {
        Self {
            channel_id,
            conversation_id,
            reply_to: None,
        }
    }

    /// Create reply route with reply-to reference
    pub fn with_reply_to(mut self, message_id: MessageId) -> Self {
        self.reply_to = Some(message_id);
        self
    }
}

/// Full context for an inbound message, used throughout processing
#[derive(Debug, Clone)]
pub struct InboundContext {
    /// Original inbound message
    pub message: InboundMessage,

    /// Route for sending replies
    pub reply_route: ReplyRoute,

    /// Resolved session key for this message
    pub session_key: SessionKey,

    /// Whether sender is authorized (passed permission check)
    pub is_authorized: bool,

    /// Whether bot was mentioned (for group messages)
    pub is_mentioned: bool,

    /// Sender's normalized identifier
    pub sender_normalized: String,
}

impl InboundContext {
    /// Create a new inbound context
    pub fn new(
        message: InboundMessage,
        reply_route: ReplyRoute,
        session_key: SessionKey,
    ) -> Self {
        let sender_normalized = message.sender_id.as_str().to_string();
        Self {
            message,
            reply_route,
            session_key,
            is_authorized: false,
            is_mentioned: false,
            sender_normalized,
        }
    }

    /// Mark as authorized
    pub fn authorize(mut self) -> Self {
        self.is_authorized = true;
        self
    }

    /// Mark as mentioned
    pub fn with_mention(mut self, mentioned: bool) -> Self {
        self.is_mentioned = mentioned;
        self
    }

    /// Set normalized sender ID
    pub fn with_sender_normalized(mut self, normalized: String) -> Self {
        self.sender_normalized = normalized;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_test_message() -> InboundMessage {
        InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("imessage"),
            conversation_id: ConversationId::new("+15551234567"),
            sender_id: super::super::channel::UserId::new("+15551234567"),
            sender_name: None,
            text: "Hello".to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        }
    }

    #[test]
    fn test_reply_route_creation() {
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        assert_eq!(route.channel_id.as_str(), "imessage");
        assert_eq!(route.conversation_id.as_str(), "+15551234567");
        assert!(route.reply_to.is_none());
    }

    #[test]
    fn test_reply_route_with_reply_to() {
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        )
        .with_reply_to(MessageId::new("msg-123"));

        assert_eq!(route.reply_to.as_ref().unwrap().as_str(), "msg-123");
    }

    #[test]
    fn test_inbound_context_creation() {
        let msg = make_test_message();
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        let session_key = SessionKey::main("main");

        let ctx = InboundContext::new(msg, route, session_key);

        assert!(!ctx.is_authorized);
        assert!(!ctx.is_mentioned);
        assert_eq!(ctx.sender_normalized, "+15551234567");
    }

    #[test]
    fn test_inbound_context_authorize() {
        let msg = make_test_message();
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        let session_key = SessionKey::main("main");

        let ctx = InboundContext::new(msg, route, session_key).authorize();

        assert!(ctx.is_authorized);
    }
}
```

**Step 2: Add module to gateway/mod.rs**

Add after line 65 (after `hot_reload`):

```rust
#[cfg(feature = "gateway")]
pub mod inbound_context;
```

And add to exports (after line 112):

```rust
#[cfg(feature = "gateway")]
pub use inbound_context::{InboundContext, ReplyRoute};
```

**Step 3: Run tests to verify**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --features gateway inbound_context -- --nocapture
```

Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/gateway/inbound_context.rs core/src/gateway/mod.rs
git commit -m "feat(gateway): add InboundContext and ReplyRoute types

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Create PairingStore Trait and SQLite Implementation

**Files:**
- Create: `core/src/gateway/pairing_store.rs`
- Modify: `core/src/gateway/mod.rs` (add module)

**Step 1: Create pairing_store.rs with trait and implementation**

```rust
//! Pairing Store
//!
//! Manages pairing requests for unknown senders.
//! Stores pending pairing codes and approved senders.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// A pending pairing request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Channel type (e.g., "imessage", "telegram")
    pub channel: String,
    /// Sender identifier
    pub sender_id: String,
    /// Pairing code (6 alphanumeric characters)
    pub code: String,
    /// When the request was created
    pub created_at: DateTime<Utc>,
    /// Additional metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Error type for pairing operations
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Pairing request not found")]
    NotFound,

    #[error("Invalid pairing code")]
    InvalidCode,

    #[error("Pairing request expired")]
    Expired,
}

/// Trait for pairing request storage
#[async_trait]
pub trait PairingStore: Send + Sync {
    /// Create or get existing pairing request for a sender
    /// Returns (code, was_created)
    async fn upsert(
        &self,
        channel: &str,
        sender_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<(String, bool), PairingError>;

    /// Approve a pairing request by code, adding sender to allowlist
    async fn approve(&self, channel: &str, code: &str) -> Result<PairingRequest, PairingError>;

    /// Reject/delete a pairing request
    async fn reject(&self, channel: &str, code: &str) -> Result<(), PairingError>;

    /// List pending pairing requests
    async fn list_pending(&self, channel: Option<&str>) -> Result<Vec<PairingRequest>, PairingError>;

    /// Check if a sender is in the approved list
    async fn is_approved(&self, channel: &str, sender_id: &str) -> Result<bool, PairingError>;

    /// Get approved senders for a channel
    async fn list_approved(&self, channel: &str) -> Result<Vec<String>, PairingError>;

    /// Remove a sender from the approved list
    async fn revoke(&self, channel: &str, sender_id: &str) -> Result<(), PairingError>;
}

/// SQLite-based pairing store
pub struct SqlitePairingStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqlitePairingStore {
    /// Create a new SQLite pairing store
    pub fn new(db_path: impl AsRef<Path>) -> Result<Self, PairingError> {
        let conn = Connection::open(db_path)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema_sync()?;
        Ok(store)
    }

    /// Create an in-memory pairing store (for testing)
    pub fn in_memory() -> Result<Self, PairingError> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_schema_sync()?;
        Ok(store)
    }

    fn init_schema_sync(&self) -> Result<(), PairingError> {
        let conn = self.conn.blocking_lock();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS pairing_requests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                sender_id TEXT NOT NULL,
                code TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                metadata TEXT,
                UNIQUE(channel, sender_id)
            );

            CREATE TABLE IF NOT EXISTS approved_senders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                sender_id TEXT NOT NULL,
                approved_at TEXT NOT NULL,
                UNIQUE(channel, sender_id)
            );

            CREATE INDEX IF NOT EXISTS idx_pairing_channel ON pairing_requests(channel);
            CREATE INDEX IF NOT EXISTS idx_pairing_code ON pairing_requests(code);
            CREATE INDEX IF NOT EXISTS idx_approved_channel ON approved_senders(channel);
            "#,
        )?;
        Ok(())
    }

    /// Generate a random 6-character alphanumeric code
    fn generate_code() -> String {
        use rand::Rng;
        const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        let mut rng = rand::thread_rng();
        (0..6)
            .map(|_| {
                let idx = rng.gen_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect()
    }
}

#[async_trait]
impl PairingStore for SqlitePairingStore {
    async fn upsert(
        &self,
        channel: &str,
        sender_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<(String, bool), PairingError> {
        let conn = self.conn.lock().await;

        // Check for existing request
        let existing: Option<String> = conn
            .query_row(
                "SELECT code FROM pairing_requests WHERE channel = ?1 AND sender_id = ?2",
                params![channel, sender_id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(code) = existing {
            debug!("Found existing pairing request for {}:{}", channel, sender_id);
            return Ok((code, false));
        }

        // Create new request
        let code = Self::generate_code();
        let now = Utc::now().to_rfc3339();
        let metadata_json = serde_json::to_string(&metadata).unwrap_or_default();

        conn.execute(
            "INSERT INTO pairing_requests (channel, sender_id, code, created_at, metadata) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![channel, sender_id, code, now, metadata_json],
        )?;

        info!("Created pairing request for {}:{} with code {}", channel, sender_id, code);
        Ok((code, true))
    }

    async fn approve(&self, channel: &str, code: &str) -> Result<PairingRequest, PairingError> {
        let conn = self.conn.lock().await;

        // Find the request
        let request: Option<(String, String, String, String)> = conn
            .query_row(
                "SELECT sender_id, code, created_at, metadata FROM pairing_requests WHERE channel = ?1 AND code = ?2",
                params![channel, code],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;

        let (sender_id, code, created_at, metadata_json) =
            request.ok_or(PairingError::NotFound)?;

        // Add to approved senders
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO approved_senders (channel, sender_id, approved_at) VALUES (?1, ?2, ?3)",
            params![channel, sender_id, now],
        )?;

        // Delete the pairing request
        conn.execute(
            "DELETE FROM pairing_requests WHERE channel = ?1 AND code = ?2",
            params![channel, code],
        )?;

        let metadata: HashMap<String, String> =
            serde_json::from_str(&metadata_json).unwrap_or_default();
        let created_at = DateTime::parse_from_rfc3339(&created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        info!("Approved pairing for {}:{}", channel, sender_id);

        Ok(PairingRequest {
            channel: channel.to_string(),
            sender_id,
            code,
            created_at,
            metadata,
        })
    }

    async fn reject(&self, channel: &str, code: &str) -> Result<(), PairingError> {
        let conn = self.conn.lock().await;
        let deleted = conn.execute(
            "DELETE FROM pairing_requests WHERE channel = ?1 AND code = ?2",
            params![channel, code],
        )?;

        if deleted == 0 {
            return Err(PairingError::NotFound);
        }

        info!("Rejected pairing request with code {}", code);
        Ok(())
    }

    async fn list_pending(&self, channel: Option<&str>) -> Result<Vec<PairingRequest>, PairingError> {
        let conn = self.conn.lock().await;

        let sql = match channel {
            Some(_) => "SELECT channel, sender_id, code, created_at, metadata FROM pairing_requests WHERE channel = ?1 ORDER BY created_at DESC",
            None => "SELECT channel, sender_id, code, created_at, metadata FROM pairing_requests ORDER BY created_at DESC",
        };

        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(ch) = channel {
            stmt.query_map(params![ch], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
        } else {
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
        };

        let mut requests = Vec::new();
        for row in rows {
            let (channel, sender_id, code, created_at, metadata_json) = row?;
            let metadata: HashMap<String, String> =
                serde_json::from_str(&metadata_json).unwrap_or_default();
            let created_at = DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            requests.push(PairingRequest {
                channel,
                sender_id,
                code,
                created_at,
                metadata,
            });
        }

        Ok(requests)
    }

    async fn is_approved(&self, channel: &str, sender_id: &str) -> Result<bool, PairingError> {
        let conn = self.conn.lock().await;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM approved_senders WHERE channel = ?1 AND sender_id = ?2",
                params![channel, sender_id],
                |row| row.get(0),
            )
            .optional()?;

        Ok(exists.is_some())
    }

    async fn list_approved(&self, channel: &str) -> Result<Vec<String>, PairingError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT sender_id FROM approved_senders WHERE channel = ?1 ORDER BY approved_at DESC",
        )?;
        let rows = stmt.query_map(params![channel], |row| row.get(0))?;

        let mut senders = Vec::new();
        for row in rows {
            senders.push(row?);
        }
        Ok(senders)
    }

    async fn revoke(&self, channel: &str, sender_id: &str) -> Result<(), PairingError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM approved_senders WHERE channel = ?1 AND sender_id = ?2",
            params![channel, sender_id],
        )?;
        info!("Revoked approval for {}:{}", channel, sender_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_upsert_creates_new_request() {
        let store = SqlitePairingStore::in_memory().unwrap();
        let (code, created) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        assert!(created);
        assert_eq!(code.len(), 6);
    }

    #[tokio::test]
    async fn test_upsert_returns_existing() {
        let store = SqlitePairingStore::in_memory().unwrap();

        let (code1, created1) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();
        let (code2, created2) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        assert!(created1);
        assert!(!created2);
        assert_eq!(code1, code2);
    }

    #[tokio::test]
    async fn test_approve_adds_to_approved() {
        let store = SqlitePairingStore::in_memory().unwrap();

        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        let request = store.approve("imessage", &code).await.unwrap();
        assert_eq!(request.sender_id, "+15551234567");

        // Should be approved now
        assert!(store.is_approved("imessage", "+15551234567").await.unwrap());

        // Pairing request should be deleted
        let pending = store.list_pending(Some("imessage")).await.unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_reject_deletes_request() {
        let store = SqlitePairingStore::in_memory().unwrap();

        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        store.reject("imessage", &code).await.unwrap();

        let pending = store.list_pending(Some("imessage")).await.unwrap();
        assert!(pending.is_empty());

        // Should NOT be approved
        assert!(!store.is_approved("imessage", "+15551234567").await.unwrap());
    }

    #[tokio::test]
    async fn test_list_pending() {
        let store = SqlitePairingStore::in_memory().unwrap();

        store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();
        store
            .upsert("imessage", "+15559876543", HashMap::new())
            .await
            .unwrap();
        store
            .upsert("telegram", "user123", HashMap::new())
            .await
            .unwrap();

        let all = store.list_pending(None).await.unwrap();
        assert_eq!(all.len(), 3);

        let imessage_only = store.list_pending(Some("imessage")).await.unwrap();
        assert_eq!(imessage_only.len(), 2);
    }

    #[tokio::test]
    async fn test_revoke() {
        let store = SqlitePairingStore::in_memory().unwrap();

        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();
        store.approve("imessage", &code).await.unwrap();

        assert!(store.is_approved("imessage", "+15551234567").await.unwrap());

        store.revoke("imessage", "+15551234567").await.unwrap();

        assert!(!store.is_approved("imessage", "+15551234567").await.unwrap());
    }
}
```

**Step 2: Add module to gateway/mod.rs**

Add after `inbound_context` module:

```rust
#[cfg(feature = "gateway")]
pub mod pairing_store;
```

Add to exports:

```rust
#[cfg(feature = "gateway")]
pub use pairing_store::{PairingStore, PairingRequest, PairingError, SqlitePairingStore};
```

**Step 3: Run tests**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --features gateway pairing_store -- --nocapture
```

Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/gateway/pairing_store.rs core/src/gateway/mod.rs
git commit -m "feat(gateway): add PairingStore trait and SQLite implementation

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Create ReplyEmitter

**Files:**
- Create: `core/src/gateway/reply_emitter.rs`
- Modify: `core/src/gateway/mod.rs` (add module)

**Step 1: Create reply_emitter.rs**

```rust
//! Reply Emitter
//!
//! Routes Agent output back to the originating Channel/Conversation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, error};

use super::channel::OutboundMessage;
use super::channel_registry::ChannelRegistry;
use super::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use super::inbound_context::ReplyRoute;

/// Configuration for reply emitter behavior
#[derive(Debug, Clone)]
pub struct ReplyEmitterConfig {
    /// Buffer size before flushing (characters)
    pub buffer_threshold: usize,
    /// Whether to stream responses or wait for completion
    pub stream_enabled: bool,
}

impl Default for ReplyEmitterConfig {
    fn default() -> Self {
        Self {
            buffer_threshold: 500,
            stream_enabled: false, // iMessage doesn't support streaming well
        }
    }
}

/// Emitter that routes Agent responses back to the originating Channel
pub struct ReplyEmitter {
    channel_registry: Arc<ChannelRegistry>,
    route: ReplyRoute,
    config: ReplyEmitterConfig,
    buffer: Arc<Mutex<String>>,
    seq_counter: AtomicU64,
    run_id: String,
}

impl ReplyEmitter {
    /// Create a new reply emitter
    pub fn new(
        channel_registry: Arc<ChannelRegistry>,
        route: ReplyRoute,
        run_id: String,
    ) -> Self {
        Self {
            channel_registry,
            route,
            config: ReplyEmitterConfig::default(),
            buffer: Arc::new(Mutex::new(String::new())),
            seq_counter: AtomicU64::new(0),
            run_id,
        }
    }

    /// Create with custom configuration
    pub fn with_config(
        channel_registry: Arc<ChannelRegistry>,
        route: ReplyRoute,
        run_id: String,
        config: ReplyEmitterConfig,
    ) -> Self {
        Self {
            channel_registry,
            route,
            config,
            buffer: Arc::new(Mutex::new(String::new())),
            seq_counter: AtomicU64::new(0),
            run_id,
        }
    }

    /// Send buffered content to the channel
    async fn flush(&self) -> Result<(), EventEmitError> {
        let text = {
            let mut buffer = self.buffer.lock().await;
            if buffer.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut *buffer)
        };

        debug!(
            "Flushing {} chars to {}:{}",
            text.len(),
            self.route.channel_id.as_str(),
            self.route.conversation_id.as_str()
        );

        let message = OutboundMessage::text(
            self.route.conversation_id.as_str(),
            text,
        );

        if let Err(e) = self
            .channel_registry
            .send(&self.route.channel_id, message)
            .await
        {
            error!("Failed to send reply: {}", e);
            return Err(EventEmitError::EventBus(e.to_string()));
        }

        Ok(())
    }

    /// Buffer text and flush if threshold reached
    async fn buffer_text(&self, text: &str) -> Result<(), EventEmitError> {
        let should_flush = {
            let mut buffer = self.buffer.lock().await;
            buffer.push_str(text);
            buffer.len() >= self.config.buffer_threshold
        };

        if should_flush && self.config.stream_enabled {
            self.flush().await?;
        }

        Ok(())
    }

    /// Get the run ID
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Get the reply route
    pub fn route(&self) -> &ReplyRoute {
        &self.route
    }
}

#[async_trait]
impl EventEmitter for ReplyEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        match event {
            StreamEvent::ResponseChunk { content, is_final, .. } => {
                self.buffer_text(&content).await?;
                if is_final {
                    self.flush().await?;
                }
            }
            StreamEvent::RunComplete { summary, .. } => {
                // Flush any remaining buffer
                self.flush().await?;

                // If there's a final response in summary and buffer was empty
                if let Some(response) = summary.final_response {
                    let buffer = self.buffer.lock().await;
                    if buffer.is_empty() && !response.is_empty() {
                        drop(buffer);
                        let message = OutboundMessage::text(
                            self.route.conversation_id.as_str(),
                            response,
                        );
                        if let Err(e) = self
                            .channel_registry
                            .send(&self.route.channel_id, message)
                            .await
                        {
                            error!("Failed to send final response: {}", e);
                        }
                    }
                }
            }
            StreamEvent::RunError { error, .. } => {
                // Send error message to user
                let error_msg = format!("Sorry, an error occurred: {}", error);
                let message = OutboundMessage::text(
                    self.route.conversation_id.as_str(),
                    error_msg,
                );
                if let Err(e) = self
                    .channel_registry
                    .send(&self.route.channel_id, message)
                    .await
                {
                    error!("Failed to send error message: {}", e);
                }
            }
            // Other events are not relevant for channel reply routing
            _ => {}
        }
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId};
    use crate::gateway::event_emitter::RunSummary;

    // Note: Full integration tests require a mock ChannelRegistry
    // These are unit tests for the buffer logic

    #[test]
    fn test_config_defaults() {
        let config = ReplyEmitterConfig::default();
        assert_eq!(config.buffer_threshold, 500);
        assert!(!config.stream_enabled);
    }

    #[test]
    fn test_reply_route() {
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );

        let emitter = ReplyEmitter::new(
            Arc::new(ChannelRegistry::new()),
            route.clone(),
            "run-123".to_string(),
        );

        assert_eq!(emitter.run_id(), "run-123");
        assert_eq!(emitter.route().channel_id.as_str(), "imessage");
    }
}
```

**Step 2: Add module to gateway/mod.rs**

Add after `pairing_store`:

```rust
#[cfg(feature = "gateway")]
pub mod reply_emitter;
```

Add to exports:

```rust
#[cfg(feature = "gateway")]
pub use reply_emitter::{ReplyEmitter, ReplyEmitterConfig};
```

**Step 3: Run tests**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --features gateway reply_emitter -- --nocapture
```

Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/gateway/reply_emitter.rs core/src/gateway/mod.rs
git commit -m "feat(gateway): add ReplyEmitter for routing Agent output to channels

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Create RoutingConfig

**Files:**
- Create: `core/src/gateway/routing_config.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Create routing_config.rs**

```rust
//! Routing Configuration
//!
//! Configuration for message routing, session resolution, and permission policies.

use serde::{Deserialize, Serialize};

/// DM (Direct Message) scope - how to isolate DM sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DmScope {
    /// All DMs share the main session
    Main,
    /// Each peer gets their own session (cross-channel)
    #[default]
    PerPeer,
    /// Each peer per channel gets their own session
    PerChannelPeer,
}

/// Configuration for inbound message routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Default agent ID for routing
    #[serde(default = "default_agent_id")]
    pub default_agent: String,

    /// How to scope DM sessions
    #[serde(default)]
    pub dm_scope: DmScope,

    /// Whether to auto-start channels on gateway startup
    #[serde(default = "default_true")]
    pub auto_start_channels: bool,

    /// Pairing code expiry in seconds (0 = never)
    #[serde(default = "default_pairing_expiry")]
    pub pairing_code_expiry_secs: u64,
}

fn default_agent_id() -> String {
    "main".to_string()
}

fn default_true() -> bool {
    true
}

fn default_pairing_expiry() -> u64 {
    86400 // 24 hours
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            default_agent: default_agent_id(),
            dm_scope: DmScope::default(),
            auto_start_channels: true,
            pairing_code_expiry_secs: default_pairing_expiry(),
        }
    }
}

impl RoutingConfig {
    /// Create a new routing config with default agent
    pub fn new(default_agent: impl Into<String>) -> Self {
        Self {
            default_agent: default_agent.into(),
            ..Default::default()
        }
    }

    /// Set DM scope
    pub fn with_dm_scope(mut self, scope: DmScope) -> Self {
        self.dm_scope = scope;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RoutingConfig::default();
        assert_eq!(config.default_agent, "main");
        assert_eq!(config.dm_scope, DmScope::PerPeer);
        assert!(config.auto_start_channels);
    }

    #[test]
    fn test_dm_scope_serialization() {
        let json = serde_json::to_string(&DmScope::PerChannelPeer).unwrap();
        assert_eq!(json, "\"per-channel-peer\"");

        let parsed: DmScope = serde_json::from_str("\"main\"").unwrap();
        assert_eq!(parsed, DmScope::Main);
    }

    #[test]
    fn test_config_builder() {
        let config = RoutingConfig::new("custom-agent")
            .with_dm_scope(DmScope::Main);

        assert_eq!(config.default_agent, "custom-agent");
        assert_eq!(config.dm_scope, DmScope::Main);
    }
}
```

**Step 2: Add module to gateway/mod.rs**

Add after `reply_emitter`:

```rust
#[cfg(feature = "gateway")]
pub mod routing_config;
```

Add to exports:

```rust
#[cfg(feature = "gateway")]
pub use routing_config::{RoutingConfig, DmScope};
```

**Step 3: Run tests**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --features gateway routing_config -- --nocapture
```

**Step 4: Commit**

```bash
git add core/src/gateway/routing_config.rs core/src/gateway/mod.rs
git commit -m "feat(gateway): add RoutingConfig for message routing policies

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Create InboundMessageRouter

**Files:**
- Create: `core/src/gateway/inbound_router.rs`
- Modify: `core/src/gateway/mod.rs`

**Step 1: Create inbound_router.rs**

```rust
//! Inbound Message Router
//!
//! Consumes the ChannelRegistry's inbound message stream and routes
//! messages to the appropriate Agent/Session.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use super::channel::{ChannelId, ConversationId, InboundMessage, OutboundMessage};
use super::channel_registry::ChannelRegistry;
use super::channels::imessage::{IMessageConfig, normalize_phone};
use super::inbound_context::{InboundContext, ReplyRoute};
use super::pairing_store::{PairingError, PairingStore};
use super::reply_emitter::ReplyEmitter;
use super::router::SessionKey;
use super::routing_config::{DmScope, RoutingConfig};

/// Error type for routing operations
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Execution error: {0}")]
    Execution(String),

    #[error("Pairing error: {0}")]
    Pairing(#[from] PairingError),
}

/// Inbound message router
pub struct InboundMessageRouter {
    channel_registry: Arc<ChannelRegistry>,
    pairing_store: Arc<dyn PairingStore>,
    config: RoutingConfig,
    /// Channel-specific configs (keyed by channel_id)
    channel_configs: HashMap<String, ChannelConfig>,
}

/// Unified channel config for permission checking
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// DM policy
    pub dm_policy: DmPolicy,
    /// Group policy
    pub group_policy: GroupPolicy,
    /// Allowlist for DMs
    pub allow_from: Vec<String>,
    /// Allowlist for groups
    pub group_allow_from: Vec<String>,
    /// Whether to require mention in groups
    pub require_mention: bool,
    /// Bot name for mention detection
    pub bot_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmPolicy {
    Open,
    Allowlist,
    Pairing,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupPolicy {
    Open,
    Allowlist,
    Disabled,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Pairing,
            group_policy: GroupPolicy::Open,
            allow_from: Vec::new(),
            group_allow_from: Vec::new(),
            require_mention: true,
            bot_name: None,
        }
    }
}

impl From<&IMessageConfig> for ChannelConfig {
    fn from(cfg: &IMessageConfig) -> Self {
        use super::channels::imessage::config::{DmPolicy as ImDmPolicy, GroupPolicy as ImGroupPolicy};

        Self {
            dm_policy: match cfg.dm_policy {
                ImDmPolicy::Open => DmPolicy::Open,
                ImDmPolicy::Allowlist => DmPolicy::Allowlist,
                ImDmPolicy::Pairing => DmPolicy::Pairing,
                ImDmPolicy::Disabled => DmPolicy::Disabled,
            },
            group_policy: match cfg.group_policy {
                ImGroupPolicy::Open => GroupPolicy::Open,
                ImGroupPolicy::Allowlist => GroupPolicy::Allowlist,
                ImGroupPolicy::Disabled => GroupPolicy::Disabled,
            },
            allow_from: cfg.allow_from.clone(),
            group_allow_from: cfg.group_allow_from.clone(),
            require_mention: cfg.require_mention,
            bot_name: cfg.bot_name.clone(),
        }
    }
}

impl InboundMessageRouter {
    /// Create a new inbound message router
    pub fn new(
        channel_registry: Arc<ChannelRegistry>,
        pairing_store: Arc<dyn PairingStore>,
        config: RoutingConfig,
    ) -> Self {
        Self {
            channel_registry,
            pairing_store,
            config,
            channel_configs: HashMap::new(),
        }
    }

    /// Register channel-specific configuration
    pub fn register_channel_config(&mut self, channel_id: &str, config: ChannelConfig) {
        self.channel_configs.insert(channel_id.to_string(), config);
    }

    /// Start consuming inbound messages
    ///
    /// This takes ownership of the inbound receiver from ChannelRegistry.
    /// Returns a handle that can be used to stop the router.
    pub async fn start(self: Arc<Self>) -> Option<tokio::task::JoinHandle<()>> {
        let rx = self.channel_registry.take_inbound_receiver().await?;

        let handle = tokio::spawn(async move {
            self.run_loop(rx).await;
        });

        Some(handle)
    }

    /// Main message processing loop
    async fn run_loop(self: Arc<Self>, mut rx: mpsc::Receiver<InboundMessage>) {
        info!("InboundMessageRouter started");

        while let Some(msg) = rx.recv().await {
            let router = self.clone();
            tokio::spawn(async move {
                if let Err(e) = router.handle_message(msg).await {
                    error!("Failed to handle inbound message: {}", e);
                }
            });
        }

        info!("InboundMessageRouter stopped");
    }

    /// Handle a single inbound message
    pub async fn handle_message(&self, msg: InboundMessage) -> Result<(), RoutingError> {
        let channel_id = msg.channel_id.as_str();
        debug!(
            "Handling message from {}:{} - {}",
            channel_id,
            msg.sender_id.as_str(),
            &msg.text[..msg.text.len().min(50)]
        );

        // Build context
        let ctx = self.build_context(&msg);

        // Check permissions
        let ctx = match self.check_permission(ctx).await {
            Ok(ctx) => ctx,
            Err(e) => {
                debug!("Permission check failed: {}", e);
                return Ok(()); // Not an error, just filtered
            }
        };

        // For now, log that we would execute
        // TODO: Integrate with ExecutionEngine in next task
        info!(
            "Would execute agent for session {} with input: {}",
            ctx.session_key.to_key_string(),
            &ctx.message.text[..ctx.message.text.len().min(100)]
        );

        Ok(())
    }

    /// Build InboundContext from message
    fn build_context(&self, msg: &InboundMessage) -> InboundContext {
        let reply_route = ReplyRoute::new(
            msg.channel_id.clone(),
            msg.conversation_id.clone(),
        );

        let session_key = self.resolve_session_key(msg);

        let sender_normalized = if msg.channel_id.as_str() == "imessage" {
            normalize_phone(msg.sender_id.as_str())
        } else {
            msg.sender_id.as_str().to_string()
        };

        InboundContext::new(msg.clone(), reply_route, session_key)
            .with_sender_normalized(sender_normalized)
    }

    /// Resolve SessionKey for a message
    fn resolve_session_key(&self, msg: &InboundMessage) -> SessionKey {
        let agent_id = &self.config.default_agent;
        let channel = msg.channel_id.as_str();

        if msg.is_group {
            // Group message → isolate by conversation_id
            SessionKey::peer(
                agent_id,
                format!("{}:group:{}", channel, msg.conversation_id.as_str()),
            )
        } else {
            // DM → based on dm_scope
            match self.config.dm_scope {
                DmScope::Main => SessionKey::main(agent_id),
                DmScope::PerPeer => SessionKey::peer(
                    agent_id,
                    format!("dm:{}", msg.sender_id.as_str()),
                ),
                DmScope::PerChannelPeer => SessionKey::peer(
                    agent_id,
                    format!("{}:dm:{}", channel, msg.sender_id.as_str()),
                ),
            }
        }
    }

    /// Check if message is permitted
    async fn check_permission(&self, mut ctx: InboundContext) -> Result<InboundContext, RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let channel_config = self
            .channel_configs
            .get(channel_id)
            .cloned()
            .unwrap_or_default();

        if ctx.message.is_group {
            // Group message permission check
            match channel_config.group_policy {
                GroupPolicy::Disabled => {
                    return Err(RoutingError::PermissionDenied(
                        "Group messages disabled".to_string(),
                    ));
                }
                GroupPolicy::Allowlist => {
                    let chat_id = ctx.message.conversation_id.as_str();
                    if !channel_config.group_allow_from.iter().any(|a| a == chat_id) {
                        return Err(RoutingError::PermissionDenied(
                            "Group not in allowlist".to_string(),
                        ));
                    }
                }
                GroupPolicy::Open => {
                    // Check mention requirement
                    if channel_config.require_mention {
                        let mentioned = self.check_mention(&ctx.message.text, &channel_config);
                        if !mentioned {
                            return Err(RoutingError::PermissionDenied(
                                "Mention required in group".to_string(),
                            ));
                        }
                        ctx = ctx.with_mention(true);
                    }
                }
            }
        } else {
            // DM permission check
            match channel_config.dm_policy {
                DmPolicy::Disabled => {
                    return Err(RoutingError::PermissionDenied(
                        "DMs disabled".to_string(),
                    ));
                }
                DmPolicy::Open => {
                    // Always allow
                }
                DmPolicy::Allowlist => {
                    if !self.is_in_allowlist(&ctx.sender_normalized, &channel_config.allow_from) {
                        return Err(RoutingError::PermissionDenied(
                            "Sender not in allowlist".to_string(),
                        ));
                    }
                }
                DmPolicy::Pairing => {
                    // Check allowlist first
                    if self.is_in_allowlist(&ctx.sender_normalized, &channel_config.allow_from) {
                        // Already approved via config
                    } else if self.pairing_store.is_approved(channel_id, &ctx.sender_normalized).await? {
                        // Approved via pairing
                    } else {
                        // Need pairing
                        self.send_pairing_request(&ctx).await?;
                        return Err(RoutingError::PermissionDenied(
                            "Pairing required".to_string(),
                        ));
                    }
                }
            }
        }

        ctx = ctx.authorize();
        Ok(ctx)
    }

    /// Check if sender is in allowlist
    fn is_in_allowlist(&self, sender: &str, allowlist: &[String]) -> bool {
        if allowlist.is_empty() {
            return false;
        }
        if allowlist.iter().any(|a| a == "*") {
            return true;
        }

        // Normalize both for comparison
        let sender_normalized = normalize_phone(sender);
        allowlist.iter().any(|a| {
            let allowed_normalized = normalize_phone(a);
            sender == a
                || sender.to_lowercase() == a.to_lowercase()
                || (!sender_normalized.is_empty()
                    && !allowed_normalized.is_empty()
                    && sender_normalized == allowed_normalized)
        })
    }

    /// Check if bot was mentioned in message
    fn check_mention(&self, text: &str, config: &ChannelConfig) -> bool {
        let text_lower = text.to_lowercase();

        // Check bot name
        if let Some(bot_name) = &config.bot_name {
            if text_lower.contains(&bot_name.to_lowercase()) {
                return true;
            }
        }

        // Check common patterns
        let patterns = ["@aleph", "@bot", "aleph"];
        patterns.iter().any(|p| text_lower.contains(p))
    }

    /// Send pairing request to unknown sender
    async fn send_pairing_request(&self, ctx: &InboundContext) -> Result<(), RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let sender_id = &ctx.sender_normalized;

        let mut metadata = HashMap::new();
        metadata.insert("sender_display".to_string(), ctx.message.sender_id.as_str().to_string());

        let (code, created) = self
            .pairing_store
            .upsert(channel_id, sender_id, metadata)
            .await?;

        if created {
            // Send pairing message
            let message = format!(
                "Hi! I'm Aleph, a personal AI assistant.\n\n\
                To chat with me, please have my owner approve your access.\n\n\
                Your ID: {}\n\
                Pairing code: {}\n\n\
                Once approved, just send me a message!",
                sender_id, code
            );

            let outbound = OutboundMessage::text(
                ctx.reply_route.conversation_id.as_str(),
                message,
            );

            if let Err(e) = self
                .channel_registry
                .send(&ctx.reply_route.channel_id, outbound)
                .await
            {
                warn!("Failed to send pairing message: {}", e);
            } else {
                info!("Sent pairing request to {} with code {}", sender_id, code);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::gateway::channel::{MessageId, UserId};
    use crate::gateway::pairing_store::SqlitePairingStore;

    fn make_test_message(is_group: bool) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("imessage"),
            conversation_id: ConversationId::new(if is_group { "chat_id:42" } else { "+15551234567" }),
            sender_id: UserId::new("+15551234567"),
            sender_name: None,
            text: "Hello".to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group,
            raw: None,
        }
    }

    #[test]
    fn test_resolve_session_key_dm_per_peer() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default().with_dm_scope(DmScope::PerPeer);

        let router = InboundMessageRouter::new(registry, store, config);

        let msg = make_test_message(false);
        let key = router.resolve_session_key(&msg);

        assert_eq!(key.to_key_string(), "peer:main:dm:+15551234567");
    }

    #[test]
    fn test_resolve_session_key_dm_main() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default().with_dm_scope(DmScope::Main);

        let router = InboundMessageRouter::new(registry, store, config);

        let msg = make_test_message(false);
        let key = router.resolve_session_key(&msg);

        assert_eq!(key.to_key_string(), "main:main:main");
    }

    #[test]
    fn test_resolve_session_key_group() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        let router = InboundMessageRouter::new(registry, store, config);

        let msg = make_test_message(true);
        let key = router.resolve_session_key(&msg);

        assert_eq!(key.to_key_string(), "peer:main:imessage:group:chat_id:42");
    }

    #[test]
    fn test_is_in_allowlist() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        let router = InboundMessageRouter::new(registry, store, config);

        let allowlist = vec!["+15551234567".to_string(), "user@example.com".to_string()];

        assert!(router.is_in_allowlist("+15551234567", &allowlist));
        assert!(router.is_in_allowlist("5551234567", &allowlist)); // Normalized
        assert!(router.is_in_allowlist("user@example.com", &allowlist));
        assert!(!router.is_in_allowlist("+19999999999", &allowlist));
    }

    #[test]
    fn test_is_in_allowlist_wildcard() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        let router = InboundMessageRouter::new(registry, store, config);

        let allowlist = vec!["*".to_string()];
        assert!(router.is_in_allowlist("+19999999999", &allowlist));
    }

    #[test]
    fn test_check_mention() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        let router = InboundMessageRouter::new(registry, store, config);

        let channel_config = ChannelConfig {
            bot_name: Some("MyBot".to_string()),
            ..Default::default()
        };

        assert!(router.check_mention("Hey @aleph, help me", &channel_config));
        assert!(router.check_mention("MyBot can you help?", &channel_config));
        assert!(router.check_mention("Hello AETHER", &channel_config));
        assert!(!router.check_mention("Hello world", &channel_config));
    }
}
```

**Step 2: Add module to gateway/mod.rs**

Add after `routing_config`:

```rust
#[cfg(feature = "gateway")]
pub mod inbound_router;
```

Add to exports:

```rust
#[cfg(feature = "gateway")]
pub use inbound_router::{InboundMessageRouter, RoutingError, ChannelConfig, DmPolicy, GroupPolicy};
```

**Step 3: Run tests**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --features gateway inbound_router -- --nocapture
```

**Step 4: Commit**

```bash
git add core/src/gateway/inbound_router.rs core/src/gateway/mod.rs
git commit -m "feat(gateway): add InboundMessageRouter for message routing

Implements:
- SessionKey resolution based on DmScope config
- Permission checking (allowlist, pairing, mention)
- Pairing request flow for unknown senders

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Add Pairing RPC Handlers

**Files:**
- Create: `core/src/gateway/handlers/pairing.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Create handlers/pairing.rs**

```rust
//! Pairing Handlers
//!
//! RPC handlers for pairing operations: list, approve, reject.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::debug;

use crate::gateway::pairing_store::{PairingRequest, PairingStore};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Pairing request response format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequestResponse {
    pub channel: String,
    pub sender_id: String,
    pub code: String,
    pub created_at: String,
}

impl From<PairingRequest> for PairingRequestResponse {
    fn from(req: PairingRequest) -> Self {
        Self {
            channel: req.channel,
            sender_id: req.sender_id,
            code: req.code,
            created_at: req.created_at.to_rfc3339(),
        }
    }
}

/// Handle pairing.list RPC request
///
/// Lists pending pairing requests, optionally filtered by channel.
pub async fn handle_list(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let channel = request
        .params
        .as_ref()
        .and_then(|p| p.get("channel"))
        .and_then(|v| v.as_str());

    debug!("Handling pairing.list for channel: {:?}", channel);

    match store.list_pending(channel).await {
        Ok(requests) => {
            let responses: Vec<PairingRequestResponse> =
                requests.into_iter().map(|r| r.into()).collect();

            JsonRpcResponse::success(
                request.id,
                json!({
                    "requests": responses,
                    "count": responses.len(),
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list pairing requests: {}", e),
        ),
    }
}

/// Handle pairing.approve RPC request
///
/// Approves a pairing request by code, adding the sender to the approved list.
pub async fn handle_approve(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let channel = match params.get("channel").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'channel' field");
        }
    };

    let code = match params.get("code").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'code' field");
        }
    };

    debug!("Handling pairing.approve for {}:{}", channel, code);

    match store.approve(channel, code).await {
        Ok(req) => {
            let response: PairingRequestResponse = req.into();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "approved": true,
                    "request": response,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to approve pairing: {}", e),
        ),
    }
}

/// Handle pairing.reject RPC request
///
/// Rejects a pairing request by code.
pub async fn handle_reject(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let channel = match params.get("channel").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'channel' field");
        }
    };

    let code = match params.get("code").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'code' field");
        }
    };

    debug!("Handling pairing.reject for {}:{}", channel, code);

    match store.reject(channel, code).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "rejected": true,
                "channel": channel,
                "code": code,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to reject pairing: {}", e),
        ),
    }
}

/// Handle pairing.approved RPC request
///
/// Lists approved senders for a channel.
pub async fn handle_approved_list(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let channel = match request
        .params
        .as_ref()
        .and_then(|p| p.get("channel"))
        .and_then(|v| v.as_str())
    {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'channel' field");
        }
    };

    debug!("Handling pairing.approved for channel: {}", channel);

    match store.list_approved(channel).await {
        Ok(senders) => JsonRpcResponse::success(
            request.id,
            json!({
                "channel": channel,
                "approved": senders,
                "count": senders.len(),
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list approved senders: {}", e),
        ),
    }
}

/// Handle pairing.revoke RPC request
///
/// Revokes approval for a sender.
pub async fn handle_revoke(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let channel = match params.get("channel").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'channel' field");
        }
    };

    let sender_id = match params.get("sender_id").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'sender_id' field");
        }
    };

    debug!("Handling pairing.revoke for {}:{}", channel, sender_id);

    match store.revoke(channel, sender_id).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "revoked": true,
                "channel": channel,
                "sender_id": sender_id,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to revoke approval: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::pairing_store::SqlitePairingStore;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_handle_list_empty() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let request = JsonRpcRequest::new("pairing.list", None, Some(json!(1)));

        let response = handle_list(request, store).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["count"], 0);
    }

    #[tokio::test]
    async fn test_handle_approve() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());

        // Create a pairing request first
        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        let request = JsonRpcRequest::new(
            "pairing.approve",
            Some(json!({
                "channel": "imessage",
                "code": code,
            })),
            Some(json!(1)),
        );

        let response = handle_approve(request, store.clone()).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["approved"], true);

        // Verify approved
        assert!(store.is_approved("imessage", "+15551234567").await.unwrap());
    }

    #[tokio::test]
    async fn test_handle_reject() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());

        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        let request = JsonRpcRequest::new(
            "pairing.reject",
            Some(json!({
                "channel": "imessage",
                "code": code,
            })),
            Some(json!(1)),
        );

        let response = handle_reject(request, store.clone()).await;
        assert!(response.is_success());

        // Verify NOT approved
        assert!(!store.is_approved("imessage", "+15551234567").await.unwrap());
    }
}
```

**Step 2: Add module to handlers/mod.rs**

Add after existing modules:

```rust
pub mod pairing;
```

**Step 3: Run tests**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --features gateway handlers::pairing -- --nocapture
```

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/pairing.rs core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): add pairing RPC handlers

Adds pairing.list, pairing.approve, pairing.reject,
pairing.approved, pairing.revoke handlers.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Integration Test - End-to-End Message Flow

**Files:**
- Create: `core/tests/integration_imessage_routing.rs`

**Step 1: Create integration test**

```rust
//! Integration test for iMessage Gateway routing
//!
//! Tests the complete message flow from InboundMessage to routing.

#![cfg(feature = "gateway")]

use std::collections::HashMap;
use std::sync::Arc;

use alephcore::gateway::{
    ChannelId, ChannelRegistry, ConversationId, InboundMessage, MessageId, UserId,
    InboundMessageRouter, RoutingConfig, DmScope,
    SqlitePairingStore, PairingStore,
    ChannelConfig, DmPolicy, GroupPolicy,
};
use chrono::Utc;

fn make_dm_message(sender: &str, text: &str) -> InboundMessage {
    InboundMessage {
        id: MessageId::new(format!("msg-{}", uuid::Uuid::new_v4())),
        channel_id: ChannelId::new("imessage"),
        conversation_id: ConversationId::new(sender),
        sender_id: UserId::new(sender),
        sender_name: None,
        text: text.to_string(),
        attachments: vec![],
        timestamp: Utc::now(),
        reply_to: None,
        is_group: false,
        raw: None,
    }
}

fn make_group_message(chat_id: &str, sender: &str, text: &str) -> InboundMessage {
    InboundMessage {
        id: MessageId::new(format!("msg-{}", uuid::Uuid::new_v4())),
        channel_id: ChannelId::new("imessage"),
        conversation_id: ConversationId::new(chat_id),
        sender_id: UserId::new(sender),
        sender_name: None,
        text: text.to_string(),
        attachments: vec![],
        timestamp: Utc::now(),
        reply_to: None,
        is_group: true,
        raw: None,
    }
}

#[tokio::test]
async fn test_dm_with_open_policy() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    // Set open policy
    router.register_channel_config(
        "imessage",
        ChannelConfig {
            dm_policy: DmPolicy::Open,
            ..Default::default()
        },
    );

    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    // Should succeed with open policy
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dm_with_allowlist_policy_allowed() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        ChannelConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["+15551234567".to_string()],
            ..Default::default()
        },
    );

    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dm_with_allowlist_policy_denied() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        ChannelConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["+15559999999".to_string()], // Different number
            ..Default::default()
        },
    );

    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    // Should succeed but message filtered (not an error)
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dm_with_pairing_policy() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store.clone(), config);

    router.register_channel_config(
        "imessage",
        ChannelConfig {
            dm_policy: DmPolicy::Pairing,
            allow_from: vec![],
            ..Default::default()
        },
    );

    // First message should trigger pairing request
    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;
    assert!(result.is_ok());

    // Should have created a pairing request
    let pending = store.list_pending(Some("imessage")).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].sender_id.contains("15551234567"));
}

#[tokio::test]
async fn test_dm_with_pairing_approved() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    // Pre-approve the sender
    let (code, _) = store
        .upsert("imessage", "+15551234567", HashMap::new())
        .await
        .unwrap();
    store.approve("imessage", &code).await.unwrap();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        ChannelConfig {
            dm_policy: DmPolicy::Pairing,
            allow_from: vec![],
            ..Default::default()
        },
    );

    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_group_with_mention_required() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        ChannelConfig {
            group_policy: GroupPolicy::Open,
            require_mention: true,
            bot_name: Some("Aleph".to_string()),
            ..Default::default()
        },
    );

    // Without mention - should be filtered
    let msg = make_group_message("chat_id:42", "+15551234567", "Hello everyone!");
    let result = router.handle_message(msg).await;
    assert!(result.is_ok()); // Filtered but no error

    // With mention - should pass
    let msg = make_group_message("chat_id:42", "+15551234567", "Hey @aleph, help me!");
    let result = router.handle_message(msg).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_group_disabled() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        ChannelConfig {
            group_policy: GroupPolicy::Disabled,
            ..Default::default()
        },
    );

    let msg = make_group_message("chat_id:42", "+15551234567", "@aleph help!");
    let result = router.handle_message(msg).await;

    // Should be filtered (group disabled)
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_session_key_per_peer() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::new("main").with_dm_scope(DmScope::PerPeer);

    let router = InboundMessageRouter::new(registry, store, config);

    let msg1 = make_dm_message("+15551111111", "Hello");
    let msg2 = make_dm_message("+15552222222", "Hi");

    let ctx1 = router.build_context(&msg1);
    let ctx2 = router.build_context(&msg2);

    // Different senders should get different session keys
    assert_ne!(ctx1.session_key.to_key_string(), ctx2.session_key.to_key_string());
    assert!(ctx1.session_key.to_key_string().contains("15551111111"));
    assert!(ctx2.session_key.to_key_string().contains("15552222222"));
}

#[tokio::test]
async fn test_session_key_main_scope() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::new("main").with_dm_scope(DmScope::Main);

    let router = InboundMessageRouter::new(registry, store, config);

    let msg1 = make_dm_message("+15551111111", "Hello");
    let msg2 = make_dm_message("+15552222222", "Hi");

    let ctx1 = router.build_context(&msg1);
    let ctx2 = router.build_context(&msg2);

    // With Main scope, all DMs share the same session
    assert_eq!(ctx1.session_key.to_key_string(), ctx2.session_key.to_key_string());
    assert_eq!(ctx1.session_key.to_key_string(), "main:main:main");
}
```

**Step 2: Run integration tests**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --features gateway integration_imessage_routing -- --nocapture
```

**Step 3: Commit**

```bash
git add core/tests/integration_imessage_routing.rs
git commit -m "test(gateway): add integration tests for iMessage routing

Tests DM/Group policies, pairing flow, and session key resolution.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary

This plan implements Phase 1 (Core Infrastructure) and Phase 2 (Permission System) of the iMessage Gateway integration:

| Task | Component | Description |
|------|-----------|-------------|
| 1 | InboundContext | Message context with reply routing info |
| 2 | PairingStore | Pairing request storage (SQLite) |
| 3 | ReplyEmitter | Route Agent output back to Channel |
| 4 | RoutingConfig | DmScope and routing policies |
| 5 | InboundMessageRouter | Main router with permission checking |
| 6 | Pairing Handlers | RPC handlers for pairing management |
| 7 | Integration Tests | End-to-end routing tests |

**Next Steps (Phase 3 - Agent Integration):**
- Wire InboundMessageRouter to ExecutionEngine
- Handle streaming responses via ReplyEmitter
- Test complete message→agent→reply flow
