//! Team management data types.
//!
//! Provides the foundational types for the team management system:
//! - `Team`: A named group of agents with a designated leader
//! - `TeamMember`: An agent's membership record within a team
//! - Store input/output types for CRUD operations
//!
//! Task tracking is handled by the unified `CoordTask` system
//! (see `agents::swarm::tasks`).

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
}

impl std::str::FromStr for TeamStatus {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "disbanded" => Ok(Self::Disbanded),
            _ => Err(format!("unknown TeamStatus: {s}")),
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
