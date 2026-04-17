//! Telegram inbound message context — enriched metadata for the processing pipeline.
//!
//! `TelegramInboundContext` is constructed once per inbound message and carries
//! Telegram-specific context that would otherwise be lost after message conversion.
//! It acts as the foundation for session state, group policy, and ACP bindings.
//!
//! # Design Rationale
//!
//! The raw `InboundMessage` is channel-agnostic — it intentionally strips
//! Telegram-specific details to keep the core pipeline generic. But handlers
//! and downstream logic often need richer Telegram context (chat type, thread
//! info, sender role in group, etc.). `TelegramInboundContext` fills that gap
//! without polluting the generic channel types.
//!
//! # Lifetime
//!
//! Created early in the handler pipeline (right after `AccessDecision::Allowed`
//! is confirmed), lives for the duration of message processing, and is dropped
//! when the handler returns. It is **not** persisted — session state is stored
//! separately in SQLite.

use crate::gateway::channel::{ConversationId, InboundMessage, MessageId, UserId};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Telegram chat classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    /// Direct message with a single user.
    Private,
    /// Standard group chat (legacy, not a supergroup).
    Group,
    /// Supergroup with optional forum topics.
    Supergroup,
    /// Telegram channel (one-way broadcast).
    Channel,
}

impl ChatType {
    /// Classify from a teloxide `Chat` object using the same method-based
    /// approach as the rest of the codebase.
    pub fn from_chat(ch: &teloxide::types::Chat) -> Self {
        if ch.is_private() {
            ChatType::Private
        } else if ch.is_channel() {
            ChatType::Channel
        } else if ch.is_supergroup() {
            ChatType::Supergroup
        } else {
            ChatType::Group
        }
    }

    /// Returns `true` for supergroup or group (any chat that supports group policies).
    pub fn is_group(self) -> bool {
        matches!(self, ChatType::Group | ChatType::Supergroup)
    }
}

/// Access level within a specific chat context.
///
/// Unlike `AccessDecision` (which answers "can this user interact at all?"),
/// `AccessLevel` answers "what is this user's privilege within this chat?".
///
/// Computed from:
/// - `is_group`: whether the chat is a group/supergroup
/// - Telegram member status if available (creator, administrator, member)
/// - Runtime-paired user list
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLevel {
    /// Owner — created the chat or has full control (not common in bot context).
    Owner,
    /// Administrator — can modify chat settings, pin messages, etc.
    Admin,
    /// Regular member with standard permissions.
    Member,
    /// User who is in the chat but not recognized — default for strangers.
    Stranger,
}

impl AccessLevel {
    /// Derive from Telegram member status if available.
    pub fn from_telegram_status(
        is_creator: bool,
        is_administrator: bool,
        is_in_chat: bool,
    ) -> Self {
        if is_creator {
            AccessLevel::Owner
        } else if is_administrator {
            AccessLevel::Admin
        } else if is_in_chat {
            AccessLevel::Member
        } else {
            AccessLevel::Stranger
        }
    }
}

/// Conversation key for session isolation and routing.
///
/// Format: `"{chat_id}"` or `"{chat_id}:topic:{thread_id}"`
///
/// Using a dedicated struct (rather than raw strings) prevents bugs from
/// mixing up chat_id and thread_id in string formatting.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConversationKey {
    /// The Telegram chat/group ID.
    pub chat_id: i64,
    /// Forum topic thread ID, if the chat is a forum and the message
    /// was sent in a topic thread (not the general topic).
    pub thread_id: Option<i64>,
}

impl ConversationKey {
    /// Parse a conversation key from a string returned by `ConversationId::as_str()`.
    ///
    /// Format: `"{chat_id}"` or `"{chat_id}:topic:{thread_id}"`
    pub fn parse_from_conv_id(conv_id: &str) -> Self {
        if let Some((chat_str, topic_str)) = conv_id.split_once(":topic:") {
            let chat_id = chat_str.parse().unwrap_or(0);
            let thread_id = topic_str.parse().ok();
            Self { chat_id, thread_id }
        } else {
            let chat_id = conv_id.parse().unwrap_or(0);
            Self {
                chat_id,
                thread_id: None,
            }
        }
    }

    /// Format into a `ConversationId` string.
    pub fn to_conversation_id_string(&self) -> String {
        match self.thread_id {
            Some(tid) => format!("{}:topic:{}", self.chat_id, tid),
            None => self.chat_id.to_string(),
        }
    }

    /// Create a new key from chat_id only (no thread).
    pub fn new(chat_id: i64) -> Self {
        Self {
            chat_id,
            thread_id: None,
        }
    }

    /// Create a new key from chat_id and thread_id.
    pub fn with_thread(chat_id: i64, thread_id: i64) -> Self {
        Self {
            chat_id,
            thread_id: Some(thread_id),
        }
    }
}

/// Session state for a conversation.
///
/// Stored in SQLite, loaded on first message in a session.
/// A session is identified by `(chat_id, thread_id)` — a thread-scoped
/// conversation gets its own session separate from the general topic.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// Unique session identifier (UUID).
    pub session_id: uuid::Uuid,
    /// Conversation this session belongs to.
    pub conversation_key: ConversationKey,
    /// Timestamp of last activity in this session.
    pub last_activity: DateTime<Utc>,
    /// Free-form metadata set by the agent during the session.
    /// Examples: `agent_id`, `conversation_mode`, `custom_state`.
    pub metadata: HashMap<String, String>,
}

impl SessionState {
    /// Create a new session with a fresh UUID and current timestamp.
    pub fn new(conversation_key: ConversationKey) -> Self {
        Self {
            session_id: uuid::Uuid::new_v4(),
            conversation_key,
            last_activity: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Update `last_activity` to now.
    pub fn touch(&mut self) {
        self.last_activity = Utc::now();
    }

    /// Get a metadata value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(|s| s.as_str())
    }

    /// Set a metadata value.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }

    /// Remove a metadata key.
    pub fn remove(&mut self, key: &str) {
        self.metadata.remove(key);
    }

    /// Returns `true` if the session has been inactive for longer than `timeout`.
    pub fn is_expired(&self, timeout: std::time::Duration) -> bool {
        self.last_activity + timeout < Utc::now()
    }
}

/// Media item extracted from a Telegram message.
///
/// Unlike `Attachment` (which is channel-agnostic), this type includes
/// Telegram-specific fields like `file_unique_id` for deduplication.
#[derive(Debug, Clone)]
pub struct MediaItem {
    /// Telegram `file_id` — stable identifier for this file on Telegram's servers.
    pub file_id: String,
    /// Telegram `file_unique_id` — deduplication key across bot restarts.
    pub file_unique_id: String,
    /// MIME type.
    pub mime_type: String,
    /// Filename if present.
    pub filename: Option<String>,
    /// File size in bytes.
    pub size: u64,
    /// Resolved download URL (set during attachment extraction).
    pub url: Option<String>,
}

/// Telegram inbound message context — enriched message metadata.
///
/// Constructed in `handlers.rs` after the `InboundMessage` is created.
/// Provides a stable, well-typed interface for session management,
/// group policy, and ACP bindings without exposing teloxide types
/// to the rest of the pipeline.
#[derive(Debug, Clone)]
pub struct TelegramInboundContext {
    /// The underlying channel-agnostic message.
    pub message: InboundMessage,
    /// Telegram-specific chat classification.
    pub chat_type: ChatType,
    /// Resolved sender access level within this chat.
    pub access_level: AccessLevel,
    /// Conversation key for session isolation.
    pub conversation_key: ConversationKey,
    /// Session state (loaded from SQLite on first use, not always present).
    pub session: Option<SessionState>,
    /// Extracted media with Telegram-specific fields.
    pub media: Vec<MediaItem>,
    /// The Telegram `chat.id` as a raw i64.
    pub chat_id: i64,
    /// The Telegram `thread_id` (forum topic), if present.
    pub thread_id: Option<i64>,
    /// The sender's Telegram user ID.
    pub sender_id: i64,
}

impl TelegramInboundContext {
    /// Construct a new context from a converted `InboundMessage` and teloxide `Message`.
    ///
    /// This is called in `handlers.rs` after `convert_message()` succeeds and
    /// the access check has passed.
    pub fn from_inbound(inbound: InboundMessage, tg_msg: &teloxide::types::Message) -> Self {
        let chat_type = ChatType::from_chat(&tg_msg.chat);
        let chat_id = tg_msg.chat.id.0;
        let thread_id = tg_msg.thread_id.map(|t| t.0 .0 as i64);
        let sender_id = tg_msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);

        let conversation_key =
            ConversationKey::parse_from_conv_id(inbound.conversation_id.as_str());

        // AccessLevel is computed later by the session/group policy logic
        // once we know the user's role in the chat. Default to Stranger.
        let access_level = AccessLevel::Stranger;

        Self {
            message: inbound,
            chat_type,
            access_level,
            conversation_key,
            session: None,
            media: Vec::new(),
            chat_id,
            thread_id,
            sender_id,
        }
    }

    /// Update the access level (called after group policy evaluation).
    pub fn with_access_level(mut self, level: AccessLevel) -> Self {
        self.access_level = level;
        self
    }

    /// Attach session state to this context.
    pub fn with_session(mut self, session: SessionState) -> Self {
        self.session = Some(session);
        self
    }

    /// Attach media items to this context.
    pub fn with_media(mut self, media: Vec<MediaItem>) -> Self {
        self.media = media;
        self
    }

    /// Returns `true` if this message is from a group/supergroup.
    pub fn is_group(&self) -> bool {
        self.chat_type.is_group()
    }

    /// Returns the sender's `UserId` as a string.
    pub fn sender_user_id(&self) -> UserId {
        self.message.sender_id.clone()
    }

    /// Returns the `ConversationId` for this message.
    pub fn conversation_id(&self) -> &ConversationId {
        &self.message.conversation_id
    }

    /// Returns the `MessageId` for this message.
    pub fn message_id(&self) -> MessageId {
        self.message.id.clone()
    }

    /// Returns the text content of the message.
    pub fn text(&self) -> &str {
        &self.message.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_key_parse_plain() {
        let key = ConversationKey::parse_from_conv_id("-100123456789");
        assert_eq!(key.chat_id, -100123456789);
        assert_eq!(key.thread_id, None);
    }

    #[test]
    fn test_conversation_key_parse_with_topic() {
        let key = ConversationKey::parse_from_conv_id("-100123456789:topic:42");
        assert_eq!(key.chat_id, -100123456789);
        assert_eq!(key.thread_id, Some(42));
    }

    #[test]
    fn test_conversation_key_to_string() {
        let key = ConversationKey::with_thread(-100123456789, 42);
        assert_eq!(key.to_conversation_id_string(), "-100123456789:topic:42");

        let key2 = ConversationKey::new(-100123456789);
        assert_eq!(key2.to_conversation_id_string(), "-100123456789");
    }

    #[test]
    fn test_session_state_expired() {
        let key = ConversationKey::new(123);
        let mut session = SessionState::new(key);
        assert!(!session.is_expired(std::time::Duration::from_secs(3600)));

        // Backdate last_activity
        session.last_activity = Utc::now() - std::time::Duration::from_secs(7200);
        assert!(session.is_expired(std::time::Duration::from_secs(3600)));
    }

    #[test]
    fn test_session_metadata() {
        let key = ConversationKey::new(123);
        let mut session = SessionState::new(key);
        session.set("agent_id", "claude");
        assert_eq!(session.get("agent_id"), Some("claude"));
        session.remove("agent_id");
        assert_eq!(session.get("agent_id"), None);
    }
}
