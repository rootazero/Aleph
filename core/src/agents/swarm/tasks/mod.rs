//! Task coordination data models and store trait
//!
//! Provides the foundational types for the swarm task coordination system:
//! - `CoordTask`: A unit of work assigned to one or more agents
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

/// Unique identifier for a coordination task
pub type CoordTaskId = String;

/// Unique identifier for a team
pub type TeamId = String;

/// Unique identifier for an agent
pub type AgentId = String;

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

/// Task execution priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Normal
    }
}

// ---------------------------------------------------------------------------
// CoordTaskStatus
// ---------------------------------------------------------------------------

/// Lifecycle status of a coordination task.
///
/// Note: `Blocked` is **not stored** — it is derived dynamically at query
/// time when a task has unresolved (incomplete) dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordTaskStatus {
    /// Waiting to be picked up by an agent
    Pending,
    /// Currently being executed by an agent
    InProgress,
    /// Successfully completed
    Completed,
    /// Execution failed
    Failed,
    /// Explicitly cancelled
    Cancelled,
    /// Derived: has unresolved dependencies (never persisted)
    Blocked,
}

impl Default for CoordTaskStatus {
    fn default() -> Self {
        Self::Pending
    }
}

impl std::fmt::Display for CoordTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

// ---------------------------------------------------------------------------
// TeamStatus
// ---------------------------------------------------------------------------

/// Lifecycle status of a team
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamStatus {
    /// Team is active and accepting tasks
    Active,
    /// Team is paused
    Paused,
    /// Team is disbanded
    Disbanded,
}

impl Default for TeamStatus {
    fn default() -> Self {
        Self::Active
    }
}

// ---------------------------------------------------------------------------
// TeamMember
// ---------------------------------------------------------------------------

/// A member of a team with an optional role description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    /// Agent identifier
    pub agent_id: AgentId,
    /// Optional human-readable role (e.g. "backend", "reviewer")
    pub role: Option<String>,
}

// ---------------------------------------------------------------------------
// CoordTask
// ---------------------------------------------------------------------------

/// A unit of coordinated work in the swarm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordTask {
    /// Unique task identifier
    pub id: CoordTaskId,
    /// Optional team this task belongs to
    pub team_id: Option<TeamId>,
    /// Agent assigned to execute this task
    pub assigned_agent: Option<AgentId>,
    /// Human-readable title
    pub title: String,
    /// Detailed task description (what to do and why)
    pub description: String,
    /// Execution priority
    pub priority: Priority,
    /// Current lifecycle status
    pub status: CoordTaskStatus,
    /// Original dependency list for display/audit (immutable after creation)
    pub dependencies: Vec<CoordTaskId>,
    /// Output summary written by the executing agent upon completion
    pub result: Option<String>,
    /// LLM-extensible free-form metadata
    pub metadata: serde_json::Value,
    /// Unix timestamp when this task was created
    pub created_at: u64,
    /// Unix timestamp of the last status update
    pub updated_at: u64,
}

// ---------------------------------------------------------------------------
// Team
// ---------------------------------------------------------------------------

/// A named group of agents that collaborate on coordinated tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    /// Unique team identifier
    pub id: TeamId,
    /// Human-readable team name
    pub name: String,
    /// Optional description of the team's purpose
    pub description: Option<String>,
    /// Current team status
    pub status: TeamStatus,
    /// Members of this team
    pub members: Vec<TeamMember>,
    /// LLM-extensible free-form metadata
    pub metadata: serde_json::Value,
    /// Unix timestamp when this team was created
    pub created_at: u64,
    /// Unix timestamp of the last update
    pub updated_at: u64,
}

// ---------------------------------------------------------------------------
// Input / update / filter types
// ---------------------------------------------------------------------------

/// Input for creating a new coordination task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCoordTask {
    pub team_id: Option<TeamId>,
    pub assigned_agent: Option<AgentId>,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub dependencies: Vec<CoordTaskId>,
    #[serde(default = "serde_json::Value::default")]
    pub metadata: serde_json::Value,
}

/// Partial update for an existing coordination task
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoordTaskUpdate {
    pub assigned_agent: Option<AgentId>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<Priority>,
    pub status: Option<CoordTaskStatus>,
    pub result: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Filter criteria for querying coordination tasks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoordTaskFilter {
    pub team_id: Option<TeamId>,
    pub assigned_agent: Option<AgentId>,
    pub status: Option<CoordTaskStatus>,
    pub priority: Option<Priority>,
}

/// Input for creating a new team
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTeam {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub members: Vec<TeamMember>,
    #[serde(default = "serde_json::Value::default")]
    pub metadata: serde_json::Value,
}

/// Partial update for an existing team
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<TeamStatus>,
    pub members: Option<Vec<TeamMember>>,
    pub metadata: Option<serde_json::Value>,
}

/// Filter criteria for querying teams
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamFilter {
    pub status: Option<TeamStatus>,
    /// Filter teams that contain a specific agent as a member
    pub member_agent_id: Option<AgentId>,
}

// ---------------------------------------------------------------------------
// CoordTaskStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for coordination tasks and teams.
///
/// Implementors handle the storage layer (SQLite, in-memory, etc.).
/// The trait is object-safe and Send + Sync to support Arc<dyn CoordTaskStore>.
#[async_trait]
pub trait CoordTaskStore: Send + Sync {
    // --- Task operations ---

    /// Create a new task and return the persisted record
    async fn create_task(&self, task: NewCoordTask) -> crate::error::Result<CoordTask>;

    /// Retrieve a task by its identifier
    async fn get_task(&self, id: &str) -> crate::error::Result<Option<CoordTask>>;

    /// Update fields on an existing task; returns the updated record
    async fn update_task(
        &self,
        id: &str,
        update: CoordTaskUpdate,
    ) -> crate::error::Result<CoordTask>;

    /// Delete a task by identifier; returns true if a record was removed
    async fn delete_task(&self, id: &str) -> crate::error::Result<bool>;

    /// List tasks matching the given filter (empty filter returns all)
    async fn list_tasks(
        &self,
        filter: CoordTaskFilter,
    ) -> crate::error::Result<Vec<CoordTask>>;

    /// Return tasks whose stored status is Pending but whose dependencies
    /// are all Completed — i.e., tasks that are now ready to execute
    async fn get_ready_tasks(
        &self,
        team_id: Option<&str>,
    ) -> crate::error::Result<Vec<CoordTask>>;

    // --- Team operations ---

    /// Create a new team and return the persisted record
    async fn create_team(&self, team: NewTeam) -> crate::error::Result<Team>;

    /// Retrieve a team by its identifier
    async fn get_team(&self, id: &str) -> crate::error::Result<Option<Team>>;

    /// Update fields on an existing team; returns the updated record
    async fn update_team(&self, id: &str, update: TeamUpdate) -> crate::error::Result<Team>;

    /// Delete a team by identifier; returns true if a record was removed
    async fn delete_team(&self, id: &str) -> crate::error::Result<bool>;

    /// List teams matching the given filter (empty filter returns all)
    async fn list_teams(&self, filter: TeamFilter) -> crate::error::Result<Vec<Team>>;

    /// Find all teams that a given agent belongs to.
    ///
    /// Used by the Context Injector to supply an agent with its team context.
    async fn get_agent_teams(&self, agent_id: &str) -> crate::error::Result<Vec<Team>>;
}
