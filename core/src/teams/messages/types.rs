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
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::To => "to",
            Self::Cc => "cc",
        }
    }

    pub fn from_stored(s: &str) -> Self {
        match s {
            "to" => Self::To,
            "cc" | _ => Self::Cc,
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
    Discovery,
    Challenge,
    ReviewRequest,
    ReviewResult,
    SystemNotification,
    Idle,
    PlanApprovalRequest,
    PlanApproved,
    PlanRejected,
}

impl MessageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Discovery => "discovery",
            Self::Challenge => "challenge",
            Self::ReviewRequest => "review_request",
            Self::ReviewResult => "review_result",
            Self::SystemNotification => "system_notification",
            Self::Idle => "idle",
            Self::PlanApprovalRequest => "plan_approval_request",
            Self::PlanApproved => "plan_approved",
            Self::PlanRejected => "plan_rejected",
        }
    }

    pub fn from_stored(s: &str) -> Self {
        match s {
            "message" => Self::Message,
            "discovery" => Self::Discovery,
            "challenge" => Self::Challenge,
            "review_request" => Self::ReviewRequest,
            "review_result" => Self::ReviewResult,
            "system_notification" => Self::SystemNotification,
            "idle" => Self::Idle,
            "plan_approval_request" => Self::PlanApprovalRequest,
            "plan_approved" => Self::PlanApproved,
            "plan_rejected" => Self::PlanRejected,
            _ => Self::Message,
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
}

// ---------------------------------------------------------------------------
// NewMessage
// ---------------------------------------------------------------------------

/// Input for creating a new team message (no id, timestamps, or thread_id).
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
}
