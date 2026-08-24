//! Message types for the team communication system.
//!
//! Defines the message envelope, recipient addressing with roles,
//! and message type variants used for team collaboration.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// RecipientRole
// ---------------------------------------------------------------------------

/// Determines how a message relates to an agent — direct addressee or informational copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecipientRole {
    To,
    Cc,
}

impl RecipientRole {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::To => "to",
            Self::Cc => "cc",
        }
    }

    pub fn from_stored(s: &str) -> Self {
        match s {
            "to" => Self::To,
            "cc" => Self::Cc,
            other => {
                tracing::warn!("Unknown RecipientRole stored value: {other}, defaulting to Cc");
                Self::Cc
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Recipient
// ---------------------------------------------------------------------------

/// An addressed recipient of a team message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipient {
    pub agent_id: String,
    pub role: RecipientRole,
}

// ---------------------------------------------------------------------------
// MessageType
// ---------------------------------------------------------------------------

/// Categorizes messages for filtering and TTL computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Message,
    SystemNotification,
    Idle,
    PlanApprovalRequest,
    PlanApproved,
    PlanRejected,
    ShutdownRequest,
    ShutdownApproved,
    ShutdownRejected,
    Custom(String),
}

impl MessageType {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Message => "message",
            Self::SystemNotification => "system_notification",
            Self::Idle => "idle",
            Self::PlanApprovalRequest => "plan_approval_request",
            Self::PlanApproved => "plan_approved",
            Self::PlanRejected => "plan_rejected",
            Self::ShutdownRequest => "shutdown_request",
            Self::ShutdownApproved => "shutdown_approved",
            Self::ShutdownRejected => "shutdown_rejected",
            Self::Custom(s) => s.as_str(),
        }
    }

    #[must_use]
    pub fn from_stored(s: &str) -> Self {
        match s {
            "message" => Self::Message,
            "system_notification" => Self::SystemNotification,
            "idle" => Self::Idle,
            "plan_approval_request" => Self::PlanApprovalRequest,
            "plan_approved" => Self::PlanApproved,
            "plan_rejected" => Self::PlanRejected,
            "shutdown_request" => Self::ShutdownRequest,
            "shutdown_approved" => Self::ShutdownApproved,
            "shutdown_rejected" => Self::ShutdownRejected,
            other => Self::Custom(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// TeamMessage
// ---------------------------------------------------------------------------

/// A fully materialised message with all metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMessage {
    pub id: String,
    pub team_id: String,
    pub from_agent: String,
    pub msg_type: MessageType,
    pub subject: String,
    pub content: String,
    pub recipients: Vec<Recipient>,
    pub reply_to: Option<String>,
    pub thread_id: Option<String>,
    pub attachments: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    /// The human user who authored this message, when the sender was a human
    /// (vs. an agent or the system). `None` for agent/system rows, and for
    /// human rows sent before this column existed.
    pub author_user_id: Option<String>,
}

// ---------------------------------------------------------------------------
// NewMessage
// ---------------------------------------------------------------------------

/// Input for creating a new team message (no id, timestamps, or `thread_id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMessage {
    pub team_id: String,
    pub from_agent: String,
    pub msg_type: MessageType,
    pub subject: String,
    pub content: String,
    pub recipients: Vec<Recipient>,
    pub reply_to: Option<String>,
    pub attachments: Vec<String>,
    /// The human user who authored this message, when the sender was a human
    /// (vs. an agent or the system). `None` for agent/system rows.
    pub author_user_id: Option<String>,
}
