//! Task coordination data models and store trait
//!
//! Provides the foundational types for the swarm task coordination system:
//! - `CoordTask`: A unit of work assigned to an agent
//! - `CoordTaskStore`: Async persistence trait for tasks

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod acceptance;
pub mod dag;
pub mod retry;
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
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    #[must_use]
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
///
/// `WaitingReview` and `Skipped` are openteams-parity additions (Phase C):
/// a finished run that needs human/lead-agent approval before downstream
/// tasks unblock, and a manual "not-needed" marker that still counts as
/// satisfied for dependency resolution.
///
/// `Paused` is a R3 task-level control state. The dispatcher only claims
/// tasks with `status = 'pending'`, so a paused task is naturally skipped
/// without any conditional in the scheduler. Downstream dependents stay
/// blocked because Paused does NOT satisfy dependencies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordTaskStatus {
    #[default]
    Pending,
    Blocked,
    InProgress,
    /// The run finished but a reviewer must approve before downstream
    /// dependents unblock. Set by the dispatcher when `lead_review_required`
    /// is true on the task metadata or by an explicit
    /// `teams.workflow.set_pending_review` call.
    WaitingReview,
    Completed,
    Failed,
    Cancelled,
    /// Manually marked as not-required. Treated as "satisfied" for
    /// dependency resolution (downstream tasks unblock).
    Skipped,
    /// Manually paused. Dispatcher does not claim until resumed.
    /// Does NOT satisfy downstream dependency — dependents stay blocked.
    Paused,
    /// **Derived** (like `Blocked`, never stored): a pending task that has at
    /// least one dependency in a terminal non-satisfying state (`Failed` or
    /// `Cancelled`). Such a task can never become runnable on its own — it is
    /// distinct from `Blocked` (still waiting on deps that may yet complete).
    /// This is pure visibility for the leader/panel; the dispatcher takes NO
    /// automatic recovery action (that decision is the leader's, per R7/R10).
    Unsatisfiable,
}

impl CoordTaskStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Blocked => "blocked",
            Self::InProgress => "in_progress",
            Self::WaitingReview => "waiting_review",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
            Self::Paused => "paused",
            Self::Unsatisfiable => "unsatisfiable",
        }
    }

    /// Deserialize from a stored string value.
    ///
    /// `"blocked"` and `"unsatisfiable"` are intentionally excluded — both are
    /// derived dynamically at query time and never appear in persistent storage.
    #[must_use]
    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            // "blocked" is never stored — it is derived at query time
            "in_progress" => Some(Self::InProgress),
            "waiting_review" => Some(Self::WaitingReview),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "skipped" => Some(Self::Skipped),
            "paused" => Some(Self::Paused),
            _ => None,
        }
    }

    /// Whether this status satisfies a downstream dependency — i.e. a
    /// task whose `blocked_by` set contains a task with this status no
    /// longer waits on it. Completed and Skipped both satisfy; all
    /// other statuses (including Failed) leave the dependent blocked.
    #[must_use]
    pub const fn satisfies_dependency(&self) -> bool {
        matches!(self, Self::Completed | Self::Skipped)
    }

    /// Whether this status is a derived "not yet runnable because of
    /// dependencies" state — either `Blocked` (deps may still complete) or
    /// `Unsatisfiable` (a dep terminally failed). Consumers that group these
    /// two together (e.g. a kanban "Blocked" column) use this so the
    /// `Unsatisfiable` refinement never makes a task disappear from view.
    #[must_use]
    pub const fn is_blocked_like(&self) -> bool {
        matches!(self, Self::Blocked | Self::Unsatisfiable)
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
    pub locked_by: Option<String>,
    pub locked_at: Option<u64>,
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
// CoordTaskRun — per-attempt execution record
// ---------------------------------------------------------------------------

/// Terminal status of a single task execution attempt. A task can have many
/// runs (one per dispatcher claim); this captures what each individually
/// produced so the drawer can show retry history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRunStatus {
    /// Run is still in flight — `ended_at` will be null.
    Running,
    /// Run finished cleanly.
    Completed,
    /// Run errored out (worker process or adapter failure).
    Failed,
    /// Run exceeded its timeout and was aborted.
    Timeout,
}

impl TaskRunStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
        }
    }

    #[must_use]
    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "timeout" => Some(Self::Timeout),
            _ => None,
        }
    }
}

/// Reviewer category for a step-level approval decision (Phase C).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerKind {
    /// Approved/rejected by a human via the panel UI.
    User,
    /// Approved/rejected by the team's leader agent (programmatic review).
    LeadAgent,
    /// Auto-approved by policy (e.g. `lead_review_required` = false).
    Auto,
}

/// Verdict of a step review (Phase C).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Approved,
    Rejected,
}

impl ReviewerKind {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::LeadAgent => "lead_agent",
            Self::Auto => "auto",
        }
    }
    #[must_use]
    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "lead_agent" => Some(Self::LeadAgent),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

impl ReviewVerdict {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
    #[must_use]
    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

/// One execution attempt of a [`CoordTask`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordTaskRun {
    pub id: String,
    pub task_id: CoordTaskId,
    pub agent_id: AgentId,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub status: TaskRunStatus,
    pub summary: Option<String>,
    pub error: Option<String>,
    /// Phase C — step review verdict captured against this attempt. `None`
    /// until a reviewer acts via `teams.workflow.approve_step` /
    /// `reject_step`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_verdict: Option<ReviewVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_kind: Option<ReviewerKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_id: Option<String>,
}

// ---------------------------------------------------------------------------
// CoordTaskComment — per-task handoff note
// ---------------------------------------------------------------------------

/// A free-text comment attached to a [`CoordTask`]. Used by workers to leave
/// handoff context for downstream attempts (analogous to hermes-agent's
/// `task_comments` table), and by humans to annotate state via the panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordTaskComment {
    pub id: String,
    pub task_id: CoordTaskId,
    pub author: String,
    pub body: String,
    pub created_at: u64,
}

// ---------------------------------------------------------------------------
// TaskExitJournal — R3 exit journal (ClawTeam parity)
// ---------------------------------------------------------------------------

/// Structured "end-of-task" summary written by the executing agent.
///
/// `ClawTeam` parity: when an agent finishes work it is expected to write a
/// terse, machine-readable record of what it did, what it decided, what
/// artifacts it produced, and what next steps remain. The journal is
/// surfaced in the unified trace API so the panel's replay view can
/// render a one-glance decision summary instead of forcing reviewers to
/// scroll the raw run/event stream.
///
/// One journal per task (UPSERT semantics): an agent may rewrite its
/// journal between retries — only the latest version is canonical. Older
/// runs are preserved via `coord_task_runs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExitJournal {
    /// FK back to `coord_tasks.id`. Unique — one journal per task.
    pub task_id: CoordTaskId,
    /// Agent that authored this journal (typically the task owner).
    pub agent_id: String,
    /// Required free-text summary of what was accomplished.
    pub summary: String,
    /// Optional list of decisions made (free-text bullets). JSON array
    /// of strings, stored serialised.
    pub decisions: Vec<String>,
    /// Optional list of artifact references. Strings are flexible:
    /// artifact-IDs, file paths, URLs, anchor terms — whatever the
    /// reviewer needs to find the output.
    pub artifacts_ref: Vec<String>,
    /// Optional list of next-step recommendations.
    pub next_steps: Vec<String>,
    /// Self-rated confidence 0–100. None means agent did not estimate.
    pub confidence: Option<u8>,
    /// Wall-clock when this journal was last written.
    pub created_at: u64,
}

/// Input shape used by [`CoordTaskStore::upsert_task_journal`]. Mirrors
/// `TaskExitJournal` but omits `created_at` (server-stamped).
#[derive(Debug, Clone)]
pub struct NewTaskExitJournal {
    pub task_id: CoordTaskId,
    pub agent_id: String,
    pub summary: String,
    pub decisions: Vec<String>,
    pub artifacts_ref: Vec<String>,
    pub next_steps: Vec<String>,
    pub confidence: Option<u8>,
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

    // --- Task locking ---

    /// Atomically acquire a lock on a task. Idempotent if the same agent already holds it.
    async fn acquire_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()>;
    /// Release a lock held by `agent_id`. Idempotent if already unlocked.
    async fn release_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()>;
    /// Release all locks older than `max_age_secs`. Returns number of locks released.
    async fn release_stale_locks(&self, max_age_secs: u64) -> crate::error::Result<usize>;

    // --- Run history ---
    // Per-attempt records let the panel drawer surface retry history rather
    // than only the final state. Default impls return empty so stores wired
    // before this trait extension keep compiling.

    /// Record the start of a new execution attempt. Returns the assigned run id.
    async fn start_task_run(
        &self,
        _task_id: &str,
        _agent_id: &str,
    ) -> crate::error::Result<String> {
        Ok(String::new())
    }

    /// Finish a run started via [`start_task_run`]. No-op when `run_id` is empty.
    async fn finish_task_run(
        &self,
        _run_id: &str,
        _status: TaskRunStatus,
        _summary: Option<String>,
        _error: Option<String>,
    ) -> crate::error::Result<()> {
        Ok(())
    }

    /// List all runs for a task, oldest first.
    async fn list_task_runs(&self, _task_id: &str) -> crate::error::Result<Vec<CoordTaskRun>> {
        Ok(Vec::new())
    }

    /// Record a step-level review verdict against the most recent
    /// completed run of `task_id`. Phase C (workflow parity). Default
    /// impl is a no-op so back-ends that don't support reviews fail
    /// gracefully — callers should still check via `list_task_runs`.
    async fn record_run_review(
        &self,
        _task_id: &str,
        _verdict: ReviewVerdict,
        _reviewer_kind: ReviewerKind,
        _reviewer_id: Option<&str>,
    ) -> crate::error::Result<()> {
        Ok(())
    }

    // --- Comments ---
    // Free-text handoff notes attached to a task. Stored permanently (no
    // TTL) so they survive across retries and panel sessions.

    async fn add_task_comment(
        &self,
        _task_id: &str,
        _author: &str,
        _body: &str,
    ) -> crate::error::Result<CoordTaskComment> {
        Err(crate::error::AlephError::ConfigError {
            message: "CoordTaskStore: add_task_comment not implemented".into(),
            suggestion: None,
        })
    }

    async fn list_task_comments(
        &self,
        _task_id: &str,
    ) -> crate::error::Result<Vec<CoordTaskComment>> {
        Ok(Vec::new())
    }

    // --- Exit Journal (R3) ---
    // ClawTeam parity. The executing agent calls `upsert_task_journal`
    // when it finishes; reviewers read via `get_task_journal`. Default
    // impls are no-ops/empty so backends without journal support degrade
    // gracefully — the trace API still works, journal field is just None.

    async fn upsert_task_journal(
        &self,
        _input: NewTaskExitJournal,
    ) -> crate::error::Result<TaskExitJournal> {
        Err(crate::error::AlephError::ConfigError {
            message: "CoordTaskStore: upsert_task_journal not implemented".into(),
            suggestion: None,
        })
    }

    async fn get_task_journal(
        &self,
        _task_id: &str,
    ) -> crate::error::Result<Option<TaskExitJournal>> {
        Ok(None)
    }

    /// List journals for a team, newest first. Used by the panel's
    /// team-level replay overview ("which tasks have a journal?").
    async fn list_team_journals(
        &self,
        _team_id: &str,
    ) -> crate::error::Result<Vec<TaskExitJournal>> {
        Ok(Vec::new())
    }

    /// Hard-delete all tasks for a team and their child rows
    /// (runs/comments/journals/dependencies). Returns coord_tasks rows deleted.
    async fn delete_team_tasks(&self, team_id: &str) -> crate::error::Result<usize>;
}
