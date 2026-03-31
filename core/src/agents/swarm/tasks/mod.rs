//! Task coordination data models and store trait
//!
//! Provides the foundational types for the swarm task coordination system:
//! - `CoordTask`: A unit of work assigned to an agent
//! - `CoordTaskStore`: Async persistence trait for tasks

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod dag;
pub mod store;
pub mod template;

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

pub type CoordTaskId = String;
pub type AgentId = String;

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
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

    /// Deserialize from a stored string value.
    ///
    /// `"blocked"` is intentionally excluded — `Blocked` is derived dynamically
    /// at query time and should never appear in persistent storage.
    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            // "blocked" is never stored — it is derived at query time
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
// CoordTask
// ---------------------------------------------------------------------------

/// A unit of coordinated work in the swarm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordTask {
    pub id: CoordTaskId,
    pub team_id: Option<String>,
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
// Input / update / filter types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NewCoordTask {
    pub team_id: Option<String>,
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
    pub team_id: Option<String>,
    pub status: Option<CoordTaskStatus>,
    pub owner: Option<AgentId>,
}

// ---------------------------------------------------------------------------
// CoordTaskStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for coordination tasks.
#[async_trait]
pub trait CoordTaskStore: Send + Sync {
    // --- Task CRUD ---

    async fn create_task(&self, task: NewCoordTask) -> crate::error::Result<CoordTask>;
    async fn get_task(&self, id: &str) -> crate::error::Result<Option<CoordTask>>;
    async fn update_task(
        &self,
        id: &str,
        update: CoordTaskUpdate,
    ) -> crate::error::Result<CoordTask>;
    async fn list_tasks(&self, filter: CoordTaskFilter) -> crate::error::Result<Vec<CoordTask>>;

    // --- DAG queries (read-only — dependency edges are immutable after creation) ---

    async fn get_dependencies(&self, id: &str) -> crate::error::Result<Vec<CoordTaskId>>;
    async fn get_dependents(&self, id: &str) -> crate::error::Result<Vec<CoordTaskId>>;
    /// Returns tasks whose ALL dependencies are now Completed after completing `completed_id`
    async fn get_newly_unblocked(&self, completed_id: &str)
        -> crate::error::Result<Vec<CoordTask>>;
}
