//! Protocol types for multi-agent group chat.
//!
//! These types form the channel-agnostic contract between Core and Channel layers.
//! They define speakers, personas, requests, messages, and coordination plans
//! used across all group chat interactions regardless of the underlying transport.

use std::fmt;
use std::str::FromStr;

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
    #[must_use]
    pub const fn name(&self) -> &str {
        match self {
            Self::Coordinator => "Coordinator",
            Self::Persona { name, .. } => name.as_str(),
            Self::System => "System",
        }
    }
}

// =============================================================================
// Persona
// =============================================================================

/// Maximum number of characters allowed in a persona's `system_prompt`.
const MAX_SYSTEM_PROMPT_LEN: usize = 2000;

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

impl Persona {
    /// Validate persona fields.
    ///
    /// Returns `Err` if the id, name, or `system_prompt` is empty, or if the
    /// `system_prompt` exceeds the maximum length.
    pub fn validate(&self) -> Result<(), GroupChatError> {
        if self.id.is_empty() {
            return Err(GroupChatError::InvalidPersona(
                "persona id must not be empty".into(),
            ));
        }
        if self.name.is_empty() {
            return Err(GroupChatError::InvalidPersona(
                "persona name must not be empty".into(),
            ));
        }
        if self.system_prompt.is_empty() {
            return Err(GroupChatError::InvalidPersona(
                "persona system_prompt must not be empty".into(),
            ));
        }
        if self.system_prompt.chars().count() > MAX_SYSTEM_PROMPT_LEN {
            return Err(GroupChatError::InvalidPersona(format!(
                "persona system_prompt exceeds maximum length of {MAX_SYSTEM_PROMPT_LEN} characters"
            )));
        }
        Ok(())
    }
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
    /// The session has ended.
    Ended,
}

impl GroupChatStatus {
    /// Returns the status as a string slice.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Ended => "ended",
        }
    }
}

impl fmt::Display for GroupChatStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for GroupChatStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            s if s.eq_ignore_ascii_case("active") => Ok(Self::Active),
            s if s.eq_ignore_ascii_case("ended") => Ok(Self::Ended),
            _ => Err(format!("unknown group chat status: '{s}')),
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
    ///
    /// Defaults to `false` when omitted by the coordinator LLM. Tolerating this
    /// omission prevents an otherwise-valid plan from being discarded in favor of
    /// the all-personas fallback just because the model dropped an optional field.
    #[serde(default)]
    pub need_summary: bool,
}

/// A planned response from a specific persona.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RespondentPlan {
    /// The ID of the persona that should respond.
    pub persona_id: String,
    /// The order in which this persona should respond (lower = earlier).
    ///
    /// Defaults to `0` when omitted; respondents are sorted by `order` with a
    /// stable sort, so omitted orders preserve the coordinator's declaration order.
    #[serde(default)]
    pub order: u32,
    /// Guidance for the persona on what to focus on.
    ///
    /// Defaults to empty when omitted by the coordinator LLM (same as the
    /// fallback plan), so a missing guidance string never discards the plan.
    #[serde(default)]
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

    /// A persona definition is invalid.
    #[error("invalid persona: {0}")]
    InvalidPersona(String),

    /// The session is not active (e.g., ended or paused).
    #[error("session is not active: {0}")]
    SessionInactive(String),
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
    fn test_group_chat_status_display_and_fromstr() {
        // Test as_str() and Display
        assert_eq!(GroupChatStatus::Active.as_str(), "active");
        assert_eq!(GroupChatStatus::Ended.as_str(), "ended");
        assert_eq!(format!("{}", GroupChatStatus::Active), "active");

        // Test FromStr roundtrip
        assert_eq!(
            "active".parse::<GroupChatStatus>().unwrap(),
            GroupChatStatus::Active
        );
        assert_eq!(
            "ended".parse::<GroupChatStatus>().unwrap(),
            GroupChatStatus::Ended
        );

        // Test invalid input
        assert!("unknown".parse::<GroupChatStatus>().is_err());
        assert!("".parse::<GroupChatStatus>().is_err());
    }

    #[test]
    fn test_persona_validate() {
        let valid = Persona {
            id: "arch".into(),
            name: "Architect".into(),
            system_prompt: "You are an architect".into(),
            provider: None,
            model: None,
            thinking_level: None,
        };
        assert!(valid.validate().is_ok());

        // Empty id
        let mut invalid = valid.clone();
        invalid.id = String::new();
        assert!(invalid.validate().is_err());

        // Empty name
        let mut invalid = valid.clone();
        invalid.name = String::new();
        assert!(invalid.validate().is_err());

        // Empty system_prompt
        let mut invalid = valid.clone();
        invalid.system_prompt = String::new();
        assert!(invalid.validate().is_err());

        // Prompt too long
        let mut invalid = valid.clone();
        invalid.system_prompt = "x".repeat(2001);
        assert!(invalid.validate().is_err());
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
        assert!(
            matches!(cont, GroupChatRequest::Continue { session_id, .. } if session_id == "session-001")
        );
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
