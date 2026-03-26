//! Team management data types.
//!
//! Provides the foundational types for the team management system:
//! - `Team`: A named group of agents with a designated leader
//! - `TeamMember`: An agent's membership record within a team
//! - `TeamTask`: A task assigned to a specific agent within a team
//! - Store input/output types for CRUD operations

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

pub type TeamId = String;

// ---------------------------------------------------------------------------
// TeamStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    #[default]
    Active,
    Disbanded,
}

impl TeamStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disbanded => "disbanded",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "disbanded" => Some(Self::Disbanded),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TeamTaskStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamTaskStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
}

impl TeamTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Team
// ---------------------------------------------------------------------------

/// A named group of agents led by a designated leader agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: TeamStatus,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// TeamMember
// ---------------------------------------------------------------------------

/// An agent's membership record within a team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub team_id: TeamId,
    pub agent_id: String,
    pub role: String,
    pub joined_at: i64,
}

// ---------------------------------------------------------------------------
// TeamTask
// ---------------------------------------------------------------------------

/// A task assigned to a specific agent within a team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamTask {
    pub id: String,
    pub team_id: TeamId,
    pub agent_id: String,
    pub subject: String,
    pub status: TeamTaskStatus,
    pub result: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTeam {
    pub name: String,
    pub description: String,
    pub leader_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTeamMember {
    pub team_id: TeamId,
    pub agent_id: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTeamTask {
    pub team_id: TeamId,
    pub agent_id: String,
    pub subject: String,
}

// ---------------------------------------------------------------------------
// TeamSummary
// ---------------------------------------------------------------------------

/// A lightweight summary of a team, including aggregate counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSummary {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: TeamStatus,
    pub member_count: u64,
    pub task_count: u64,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
}
