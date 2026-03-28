//! Types for collaborative sessions between agents.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current status of a collaborative session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Concluded,
    Deadlocked,
    Cancelled,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Concluded => "concluded",
            Self::Deadlocked => "deadlocked",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_stored(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "concluded" => Self::Concluded,
            "deadlocked" => Self::Deadlocked,
            "cancelled" => Self::Cancelled,
            _ => Self::Active,
        }
    }
}

/// What triggered the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTrigger {
    Explicit {
        requested_by: String,
    },
    AutoEscalation {
        thread_id: String,
        message_count: u32,
        rule: String,
    },
}

/// A single turn in the session transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTurn {
    pub agent_id: String,
    pub content: String,
    pub turn_number: u32,
    pub timestamp: DateTime<Utc>,
}

/// The outcome when a session concludes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOutcome {
    pub conclusion: String,
    pub agreed_by: Vec<String>,
    pub dissent: Option<String>,
}

/// A collaborative session between multiple agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborativeSession {
    pub id: String,
    pub team_id: String,
    pub participants: Vec<String>,
    pub topic: String,
    pub trigger: SessionTrigger,
    pub thread_id: Option<String>,
    pub max_rounds: u32,
    pub status: SessionStatus,
    pub transcript: Vec<SessionTurn>,
    pub outcome: Option<SessionOutcome>,
    pub created_at: DateTime<Utc>,
}

/// Input for creating a new session.
pub struct NewSession {
    pub team_id: String,
    pub participants: Vec<String>,
    pub topic: String,
    pub trigger: SessionTrigger,
    pub thread_id: Option<String>,
    pub max_rounds: u32,
}
