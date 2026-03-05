//! Protocol types for multi-agent group chat.
//!
//! These types form the channel-agnostic contract between Core and Channel layers.
//! They define speakers, personas, requests, messages, and coordination plans
//! used across all group chat interactions regardless of the underlying transport.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// =============================================================================
// Speaker
// =============================================================================

/// Identifies who is speaking in a group chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Speaker {
    /// The coordinator orchestrating the discussion.
    Coordinator,
    /// A persona participating in the discussion.
    Persona {
        /// Unique identifier for the persona.
        id: String,
        /// Display name of the persona.
        name: String,
    },
    /// System-generated messages (e.g., status updates, errors).
    System,
}

impl Speaker {
    /// Returns a human-readable name for the speaker.
    pub fn name(&self) -> &str {
        match self {
            Speaker::Coordinator => "Coordinator",
            Speaker::Persona { name, .. } => name.as_str(),
            Speaker::System => "System",
        }
    }
}

// =============================================================================
// Persona
// =============================================================================

/// Defines a persona that can participate in group chat discussions.
///
/// Each persona has a unique identity, a system prompt that shapes its behavior,
/// and optional overrides for the AI provider, model, and thinking level.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Persona {
    /// Unique identifier for this persona.
    pub id: String,
    /// Display name shown in conversation.
    pub name: String,
    /// System prompt that defines the persona's character, expertise, and behavior.
    pub system_prompt: String,
    /// Optional AI provider override (e.g., "anthropic", "openai").
    pub provider: Option<String>,
    /// Optional model override (e.g., "claude-sonnet-4-20250514").
    pub model: Option<String>,
    /// Optional thinking level override (e.g., "low", "medium", "high").
    pub thinking_level: Option<String>,
}

// =============================================================================
// PersonaSource
// =============================================================================

/// Where a persona definition comes from.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PersonaSource {
    /// A preset persona loaded by name from the persona registry.
    Preset(String),
    /// An inline persona definition provided directly in the request.
    Inline(Persona),
}

// =============================================================================
// GroupChatRequest
// =============================================================================

/// Requests that can be sent to the group chat orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum GroupChatRequest {
    /// Start a new group chat session.
    Start {
        /// Personas participating in the discussion.
        personas: Vec<PersonaSource>,
        /// The topic or theme of the discussion.
        topic: String,
        /// The initial message to kick off the discussion.
        initial_message: String,
    },
    /// Continue an existing group chat session with a new message.
    Continue {
        /// The session to continue.
        session_id: String,
        /// The message to add to the discussion.
        message: String,
    },
    /// Mention specific personas in a message, directing them to respond.
    Mention {
        /// The session to send the mention in.
        session_id: String,
        /// The message content.
        message: String,
        /// Persona IDs that are specifically targeted.
        targets: Vec<String>,
    },
    /// End a group chat session.
    End {
        /// The session to end.
        session_id: String,
    },
}

// =============================================================================
// GroupChatMessage
// =============================================================================

/// A message within a group chat session.
///
/// Messages are ordered by round and sequence number. The `is_final` flag
/// indicates whether this is the last message in the current round.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GroupChatMessage {
    /// The session this message belongs to.
    pub session_id: String,
    /// Who sent this message.
    pub speaker: Speaker,
    /// The message content.
    pub content: String,
    /// The discussion round (starts at 1).
    pub round: u32,
    /// Sequence number within the round (starts at 0).
    pub sequence: u32,
    /// Whether this is the final message of the current round.
    pub is_final: bool,
}

// =============================================================================
// GroupChatStatus
// =============================================================================

/// Status of a group chat session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupChatStatus {
    /// The session is active and accepting messages.
    Active,
    /// The session is paused (can be resumed).
    Paused,
    /// The session has ended.
    Ended,
}

impl GroupChatStatus {
    /// Returns the status as a string slice.
    pub fn as_str(&self) -> &str {
        match self {
            GroupChatStatus::Active => "active",
            GroupChatStatus::Paused => "paused",
            GroupChatStatus::Ended => "ended",
        }
    }

    /// Parses a status from a string.
    ///
    /// Returns `None` if the string doesn't match any known status.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(GroupChatStatus::Active),
            "paused" => Some(GroupChatStatus::Paused),
            "ended" => Some(GroupChatStatus::Ended),
            _ => None,
        }
    }
}

// =============================================================================
// ContentFormat / RenderedContent
// =============================================================================

/// The format of rendered content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentFormat {
    /// Markdown formatted text.
    Markdown,
    /// HTML formatted text.
    Html,
    /// Plain text with no formatting.
    Plain,
}

/// Rendered content with format metadata.
///
/// Provides convenience constructors for common formats.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RenderedContent {
    /// The rendered text content.
    pub text: String,
    /// The format of the text content.
    pub format: ContentFormat,
    /// Optional metadata associated with the content.
    pub metadata: Option<Value>,
}

impl RenderedContent {
    /// Creates a new Markdown-formatted content.
    pub fn markdown(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: ContentFormat::Markdown,
            metadata: None,
        }
    }

    /// Creates a new plain text content.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: ContentFormat::Plain,
            metadata: None,
        }
    }

    /// Creates a new HTML-formatted content.
    pub fn html(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: ContentFormat::Html,
            metadata: None,
        }
    }
}

// =============================================================================
// CoordinatorPlan / RespondentPlan
// =============================================================================

/// A plan produced by the coordinator for a discussion round.
///
/// The coordinator analyzes the conversation and decides which personas
/// should respond and in what order.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CoordinatorPlan {
    /// The personas that should respond in this round, in order.
    pub respondents: Vec<RespondentPlan>,
    /// Whether a summary should be generated after all respondents have spoken.
    pub need_summary: bool,
}

/// A planned response from a specific persona.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RespondentPlan {
    /// The ID of the persona that should respond.
    pub persona_id: String,
    /// The order in which this persona should respond (lower = earlier).
    pub order: u32,
    /// Guidance for the persona on what to focus on.
    pub guidance: String,
}

// =============================================================================
// GroupChatError
// =============================================================================

/// Errors that can occur during group chat operations.
#[derive(Debug, thiserror::Error)]
pub enum GroupChatError {
    /// The specified persona was not found.
    #[error("persona not found: {0}")]
    PersonaNotFound(String),

    /// Too many personas in a single session.
    #[error("too many personas: {count} exceeds maximum of {max}")]
    TooManyPersonas {
        /// The number of personas requested.
        count: usize,
        /// The maximum allowed.
        max: usize,
    },

    /// The maximum number of discussion rounds has been reached.
    #[error("maximum rounds reached: {0}")]
    MaxRoundsReached(u32),

    /// The specified session was not found.
    #[error("session not found: {0}")]
    SessionNotFound(String),

    /// Failed to parse the coordinator's response into a plan.
    #[error("failed to parse coordinator plan: {0}")]
    CoordinatorPlanParseError(String),

    /// A persona invocation failed.
    #[error("persona invocation failed for '{persona_id}': {reason}")]
    PersonaInvocationFailed {
        /// The ID of the persona that failed.
        persona_id: String,
        /// The reason for the failure.
        reason: String,
    },

    /// The requested AI provider is unavailable.
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speaker_display() {
        assert_eq!(Speaker::Coordinator.name(), "Coordinator");
        assert_eq!(Speaker::System.name(), "System");

        let persona_speaker = Speaker::Persona {
            id: "expert-1".to_string(),
            name: "Dr. Smith".to_string(),
        };
        assert_eq!(persona_speaker.name(), "Dr. Smith");
    }

    #[test]
    fn test_group_chat_message_is_final() {
        let msg = GroupChatMessage {
            session_id: "session-001".to_string(),
            speaker: Speaker::Persona {
                id: "persona-1".to_string(),
                name: "Alice".to_string(),
            },
            content: "I think we should consider...".to_string(),
            round: 2,
            sequence: 3,
            is_final: true,
        };

        assert_eq!(msg.session_id, "session-001");
        assert_eq!(msg.round, 2);
        assert_eq!(msg.sequence, 3);
        assert!(msg.is_final);
        assert_eq!(msg.speaker.name(), "Alice");
        assert_eq!(msg.content, "I think we should consider...");
    }

    #[test]
    fn test_group_chat_status_display() {
        // Test as_str()
        assert_eq!(GroupChatStatus::Active.as_str(), "active");
        assert_eq!(GroupChatStatus::Paused.as_str(), "paused");
        assert_eq!(GroupChatStatus::Ended.as_str(), "ended");

        // Test from_str() roundtrip
        assert_eq!(GroupChatStatus::from_str("active"), Some(GroupChatStatus::Active));
        assert_eq!(GroupChatStatus::from_str("paused"), Some(GroupChatStatus::Paused));
        assert_eq!(GroupChatStatus::from_str("ended"), Some(GroupChatStatus::Ended));

        // Test invalid input
        assert_eq!(GroupChatStatus::from_str("unknown"), None);
        assert_eq!(GroupChatStatus::from_str(""), None);
    }

    #[test]
    fn test_group_chat_request_variants() {
        let start = GroupChatRequest::Start {
            personas: vec![PersonaSource::Preset("expert".to_string())],
            topic: "Rust async patterns".to_string(),
            initial_message: "Let's discuss...".to_string(),
        };
        assert!(matches!(start, GroupChatRequest::Start { .. }));

        let cont = GroupChatRequest::Continue {
            session_id: "session-001".to_string(),
            message: "What about error handling?".to_string(),
        };
        assert!(matches!(cont, GroupChatRequest::Continue { session_id, .. } if session_id == "session-001"));
    }

    #[test]
    fn test_rendered_content_creation() {
        let md = RenderedContent::markdown("# Hello");
        assert_eq!(md.text, "# Hello");
        assert_eq!(md.format, ContentFormat::Markdown);
        assert!(md.metadata.is_none());

        let plain = RenderedContent::plain("Hello world");
        assert_eq!(plain.text, "Hello world");
        assert_eq!(plain.format, ContentFormat::Plain);
        assert!(plain.metadata.is_none());

        let html = RenderedContent::html("<h1>Hello</h1>");
        assert_eq!(html.text, "<h1>Hello</h1>");
        assert_eq!(html.format, ContentFormat::Html);
        assert!(html.metadata.is_none());
    }
}
