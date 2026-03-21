//! Task coordination data models and store trait
//!
//! Provides the foundational types for the swarm task coordination system:
//! - `CoordTask`: A unit of work assigned to an agent
//! - `Team`: A named group of agents that collaborate on tasks
//! - `CoordTaskStore`: Async persistence trait for tasks and teams

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod dag;
pub mod store;
pub mod template;

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

pub type CoordTaskId = String;
pub type TeamId = String;
pub type AgentId = String;

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "low" => Some(Self::Low),
            "normal" => Some(Self::Normal),
            "high" => Some(Self::High),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// CoordTaskStatus
// ---------------------------------------------------------------------------

/// Lifecycle status of a coordination task.
///
/// `Blocked` is **derived dynamically** at query time when a task has
/// unresolved dependencies — it is never stored in the database.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordTaskStatus {
    #[default]
    Pending,
    Blocked,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl CoordTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Blocked => "blocked",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "blocked" => Some(Self::Blocked),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

impl std::fmt::Display for CoordTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// TeamStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    #[default]
    Active,
    Completed,
    Disbanded,
}

impl TeamStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Disbanded => "disbanded",
        }
    }

    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "completed" => Some(Self::Completed),
            "disbanded" => Some(Self::Disbanded),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// CoordTask
// ---------------------------------------------------------------------------

/// A unit of coordinated work in the swarm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordTask {
    pub id: CoordTaskId,
    pub team_id: Option<TeamId>,
    pub subject: String,
    pub description: String,
    pub status: CoordTaskStatus,
    pub owner: Option<AgentId>,
    pub priority: Priority,
    pub result: Option<String>,
    pub metadata: serde_json::Value,
    /// Original dependency list (immutable after creation, for display/audit)
    pub dependencies: Vec<CoordTaskId>,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
}

// ---------------------------------------------------------------------------
// Team
// ---------------------------------------------------------------------------

/// A named group of agents that collaborate on coordinated tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader: AgentId,
    pub members: Vec<TeamMember>,
    pub status: TeamStatus,
    pub created_at: u64,
}

/// A member of a team
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub agent_id: AgentId,
    pub role: String,
    pub joined_at: u64,
}

// ---------------------------------------------------------------------------
// Input / update / filter types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NewCoordTask {
    pub team_id: Option<TeamId>,
    pub subject: String,
    pub description: String,
    pub owner: Option<AgentId>,
    pub priority: Priority,
    pub blocked_by: Vec<CoordTaskId>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct CoordTaskUpdate {
    pub status: Option<CoordTaskStatus>,
    pub owner: Option<AgentId>,
    pub result: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct CoordTaskFilter {
    pub team_id: Option<TeamId>,
    pub status: Option<CoordTaskStatus>,
    pub owner: Option<AgentId>,
}

#[derive(Debug, Clone)]
pub struct NewTeam {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader: AgentId,
}

#[derive(Debug, Clone, Default)]
pub struct TeamUpdate {
    pub status: Option<TeamStatus>,
}

#[derive(Debug, Clone, Default)]
pub struct TeamFilter {}

// ---------------------------------------------------------------------------
// CoordTaskStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for coordination tasks and teams.
#[async_trait]
pub trait CoordTaskStore: Send + Sync {
    // --- Task CRUD ---

    async fn create_task(&self, task: NewCoordTask) -> crate::error::Result<CoordTask>;
    async fn get_task(&self, id: &str) -> crate::error::Result<Option<CoordTask>>;
    async fn update_task(&self, id: &str, update: CoordTaskUpdate) -> crate::error::Result<CoordTask>;
    async fn list_tasks(&self, filter: CoordTaskFilter) -> crate::error::Result<Vec<CoordTask>>;

    // --- DAG queries (read-only — dependency edges are immutable after creation) ---

    async fn get_dependencies(&self, id: &str) -> crate::error::Result<Vec<CoordTaskId>>;
    async fn get_dependents(&self, id: &str) -> crate::error::Result<Vec<CoordTaskId>>;
    /// Returns tasks whose ALL dependencies are now Completed after completing `completed_id`
    async fn get_newly_unblocked(&self, completed_id: &str) -> crate::error::Result<Vec<CoordTask>>;

    // --- Team CRUD ---

    async fn create_team(&self, team: NewTeam) -> crate::error::Result<Team>;
    async fn get_team(&self, id: &str) -> crate::error::Result<Option<Team>>;
    async fn update_team(&self, id: &str, update: TeamUpdate) -> crate::error::Result<Team>;
    async fn list_teams(&self, filter: TeamFilter) -> crate::error::Result<Vec<Team>>;
    async fn add_member(&self, team_id: &str, member: TeamMember) -> crate::error::Result<()>;
    async fn remove_member(&self, team_id: &str, agent_id: &str) -> crate::error::Result<()>;

    /// Find all teams where agent_id is leader or member
    async fn get_agent_teams(&self, agent_id: &str) -> crate::error::Result<Vec<Team>>;
}
