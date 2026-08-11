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
pub mod timeout;

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
    /// dependents unblock. Set by the dispatcher on member-run completion
    /// when `lead_review_required` is true on the task metadata.
    WaitingReview,
    Completed,
    Failed,
    Cancelled,
    /// Manually marked as not-required. Treated as "satisfied" for
    /// dependency resolution (downstream tasks unblock).
    Skipped,
    /// Manually paused. Dispatcher does not claim until resumed.
    /// Does NOT satisfy downstream dependency — dependents stay blocked.
    /// A task paused from `WaitingReview` carries a [`PAUSED_FROM_KEY`]
    /// stamp so resume restores the review gate instead of re-executing.
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

    /// Parse a user/RPC-facing status FILTER string: the full 10-variant
    /// vocabulary, including the derived `blocked`/`unsatisfiable` (which
    /// `from_stored` deliberately rejects — a filter may match against derived
    /// statuses, a write may not). Single source for every status-filter
    /// surface (`teams.list_tasks`, `teams.workflow.export_canvas`, the
    /// `team_workflow_canvas` tool); callers reject `None` explicitly instead
    /// of silently dropping the filter.
    #[must_use]
    pub fn from_filter_str(s: &str) -> Option<Self> {
        match s {
            "blocked" => Some(Self::Blocked),
            "unsatisfiable" => Some(Self::Unsatisfiable),
            other => Self::from_stored(other),
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

    /// Whether this is a **stored terminal** state: the task's outcome is
    /// decided and no dispatcher action can or should follow. Used as the
    /// sticky guard on write-back paths — a task an operator/tool moved to a
    /// terminal state mid-flight must never be resurrected or overwritten by
    /// the run that was still executing it.
    ///
    /// `WaitingReview` is deliberately NOT terminal (parked for a verdict);
    /// `Unsatisfiable` is derived, never stored, so it cannot appear on the
    /// write-back paths this guards (see [`Self::is_settled`] for the
    /// observer-side notion that includes it).
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Skipped
        )
    }

    /// Whether this status is **settled** from an observer's point of view:
    /// terminal, or structurally dead (`Unsatisfiable` — a dependency
    /// terminally failed, so the task can never run unless a lead retry of
    /// the failed parent re-derives it back to `Blocked`). "All tasks
    /// settled" is the honest completion predicate for a team / workflow run;
    /// using [`Self::is_terminal`] there would wait forever on the
    /// unsatisfiable corpses a failed step leaves behind.
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        self.is_terminal() || matches!(self, Self::Unsatisfiable)
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
    /// Full replacement value for the task's metadata. The store writes this
    /// verbatim (`metadata = ?`), so internal callers that own the whole
    /// object (dispatcher retry stamps, claim pipelines) pass a value they
    /// built from a fresh read. **Boundary callers** (the `task_update` tool,
    /// the `teams.update_task` RPC) receive arbitrary partial patches from an
    /// LLM/panel and must merge via [`merge_metadata_patch`] first — writing a
    /// patch here directly wipes the dispatcher control keys (`managed_by`,
    /// `max_retries`, `timeout_secs`, review/retry markers) and silently
    /// drops the task out of scheduling.
    pub metadata: Option<serde_json::Value>,
}

/// Shallow JSON merge-patch (RFC 7386 semantics, one level deep) for task
/// metadata: `patch`'s top-level keys are laid over a clone of `existing`;
/// a `null` patch value deletes the key; keys absent from the patch are
/// preserved. A non-object `patch` (or non-object `existing`) replaces
/// wholesale — the caller explicitly sent a scalar and gets literal
/// behaviour. Pure; the single source both metadata-patching boundaries
/// (`task_update` tool, `teams.update_task` RPC) share so their semantics
/// cannot drift.
#[must_use]
pub fn merge_metadata_patch(
    existing: &serde_json::Value,
    patch: serde_json::Value,
) -> serde_json::Value {
    let serde_json::Value::Object(patch_map) = patch else {
        return patch; // scalar/array patch: explicit wholesale replace
    };
    let mut merged = match existing {
        serde_json::Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    for (key, value) in patch_map {
        if value.is_null() {
            merged.remove(&key);
        } else {
            merged.insert(key, value);
        }
    }
    serde_json::Value::Object(merged)
}

/// Metadata key stamped when a pause parks a task from a status that a plain
/// `Resume → Pending` reset would corrupt. `Blocked`/`Unsatisfiable` are
/// derived at read time from stored `pending` (see `derive_status`), so
/// Pending restores them losslessly and they carry NO stamp. `WaitingReview`
/// is the exception: it is a real stored status on a task that already RAN —
/// flattening it to Pending makes the dispatcher re-execute finished work.
/// Pause surfaces stamp the origin status here for WaitingReview only; resume
/// restores it and clears the stamp in the same write. Read ONLY while the
/// task is `Paused` (a verdict landing mid-pause moves the task on and leaves
/// a stale-but-inert stamp behind).
pub const PAUSED_FROM_KEY: &str = "paused_from";

/// [`PAUSED_FROM_KEY`] value meaning "the user paused this step while its run
/// was still in flight".
///
/// A pause cannot stop a running member run, so `workflow(action='pause')`
/// records the *intent* here and leaves the status `InProgress` — writing
/// `Paused` over a live run would make the finalize fence discard the work.
/// The stamp is what stops `reclaim_orphaned` from silently re-dispatching the
/// step after a restart (it parks it `Paused` instead); resume clears it.
///
/// A single source because writer (`builtin_tools::workflow_tool`) and reader
/// (`teams::dispatcher::schedule::reclaim`) live in different subsystems and a
/// typo on either side fails **silently** — the pause is just quietly undone,
/// which is the exact bug the stamp exists to fix.
pub const PAUSED_FROM_IN_PROGRESS: &str = "in_progress";

/// Read the pause-origin stamp, if present. Tolerant: missing / non-string
/// reads as `None` (legacy rows paused before the stamp existed restore to
/// the Pending default).
#[must_use]
pub fn paused_from(metadata: &serde_json::Value) -> Option<&str> {
    metadata.get(PAUSED_FROM_KEY).and_then(|v| v.as_str())
}

#[derive(Debug, Clone, Default)]
pub struct CoordTaskFilter {
    pub team_id: Option<String>,
    pub status: Option<CoordTaskStatus>,
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
    /// Run never finished and its worker is gone (daemon restart or lost
    /// worker) — closed after the fact by the run-row janitor. Deliberately
    /// excluded from the retry budget (crash orphans are not the task's
    /// fault; see `fail_or_retry`).
    Abandoned,
}

impl TaskRunStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::Abandoned => "abandoned",
        }
    }

    #[must_use]
    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "timeout" => Some(Self::Timeout),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

/// The `error` text stamped on a run row closed by the **run-row janitor**
/// (`CoordTaskStore::abandon_orphaned_runs`) because its worker vanished.
///
/// [`TaskRunStatus::Abandoned`] alone cannot answer "did this task crash?":
/// the dispatcher also files a deliberately-deferred attempt (the target agent
/// was busy, so nothing was tried) under the same status, precisely so it does
/// NOT consume the retry budget. Both readings are wanted, so the two
/// populations are told
/// apart by *who closed the row*, and this constant is the single source both
/// the writer (`store::runs::abandon_orphaned_runs`) and the reader
/// (`retry::recovery_abandons_since`) share — a second copy of the sentence
/// would drift and the drift is silent (a miscounted crash-recovery budget
/// neither errors nor logs).
pub const RUN_ABANDONED_BY_JANITOR_ERROR: &str =
    "interrupted: run never finished (process restart or lost worker)";

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
    // than only the final state.
    //
    // B5-03 (no-op default bodies deleted): with a single implementor
    // (`SqliteCoordTaskStore`), `cargo check` is the enforcement mechanism.
    // The previous defaults handed the dispatcher exactly the values it
    // refuses to guess — `list_task_runs -> Ok(vec![])` reads as
    // `failed_attempts = 0`, the budget predicate then returns `Retry`,
    // `jittered_backoff_secs(0, …) = 0`, `retry_not_before = now`, and
    // every future backend that inherits the defaults gets an unbounded,
    // zero-backoff retry hot loop of real member runs against the
    // provider. `record_run_review`'s default was the same shape for the
    // audit trail — an approval returns `Ok(())` and is recorded nowhere.

    /// Record the start of a new execution attempt. Returns the assigned run id.
    async fn start_task_run(&self, task_id: &str, agent_id: &str) -> crate::error::Result<String>;

    /// Finish a run started via [`start_task_run`]. No-op when `run_id` is empty.
    async fn finish_task_run(
        &self,
        run_id: &str,
        status: TaskRunStatus,
        summary: Option<String>,
        error: Option<String>,
    ) -> crate::error::Result<()>;

    /// List all runs for a task, oldest first.
    async fn list_task_runs(&self, task_id: &str) -> crate::error::Result<Vec<CoordTaskRun>>;

    /// Close `running` run rows whose task is NOT in `live_task_ids` as
    /// [`TaskRunStatus::Abandoned`]. Keyed on the runs table (not task
    /// status) so cancel-then-crash orphans — terminal tasks with a stuck
    /// `running` row that no status-keyed janitor can see — also close.
    ///
    /// Correctness contract: the dispatcher is the sole producer of run
    /// rows, and every live row's task id is in the dispatcher's in-process
    /// running set for the row's whole lifetime (insert before spawn,
    /// removal after finish). A future producer outside the dispatcher must
    /// register its live task ids here or its rows will be swept.
    ///
    /// Returns the number of rows closed.
    async fn abandon_orphaned_runs(&self, live_task_ids: &[String]) -> crate::error::Result<usize>;

    /// Record a step-level review verdict against the most recent
    /// completed run of `task_id`. Phase C (workflow parity).
    async fn record_run_review(
        &self,
        task_id: &str,
        verdict: ReviewVerdict,
        reviewer_kind: ReviewerKind,
        reviewer_id: Option<&str>,
    ) -> crate::error::Result<()>;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_metadata_patch_preserves_unmentioned_keys() {
        // The P0 this fn exists for: a partial patch must never wipe the
        // dispatcher control keys.
        let existing = serde_json::json!({
            "managed_by": "dispatcher",
            "max_retries": 2,
            "timeout_secs": 600,
        });
        let merged = merge_metadata_patch(&existing, serde_json::json!({ "note": "hi" }));
        assert_eq!(merged["managed_by"], "dispatcher");
        assert_eq!(merged["max_retries"], 2);
        assert_eq!(merged["timeout_secs"], 600);
        assert_eq!(merged["note"], "hi");
        // Immutable: the original is untouched.
        assert!(existing.get("note").is_none());
    }

    #[test]
    fn merge_metadata_patch_null_deletes_and_overwrite_wins() {
        let existing = serde_json::json!({ "a": 1, "b": 2 });
        let merged = merge_metadata_patch(
            &existing,
            serde_json::json!({ "a": serde_json::Value::Null, "b": 9 }),
        );
        assert!(merged.get("a").is_none(), "null deletes the key");
        assert_eq!(merged["b"], 9, "explicit value overwrites");
    }

    #[test]
    fn merge_metadata_patch_non_object_replaces_wholesale() {
        let existing = serde_json::json!({ "a": 1 });
        assert_eq!(
            merge_metadata_patch(&existing, serde_json::json!("scalar")),
            serde_json::json!("scalar")
        );
        // Non-object existing promotes to an object holding the patch keys.
        let merged = merge_metadata_patch(&serde_json::Value::Null, serde_json::json!({"k": 1}));
        assert_eq!(merged["k"], 1);
    }

    #[test]
    fn terminal_and_settled_predicates_partition_statuses() {
        use CoordTaskStatus as S;
        for s in [S::Completed, S::Failed, S::Cancelled, S::Skipped] {
            assert!(s.is_terminal(), "{s} terminal");
            assert!(s.is_settled(), "{s} settled");
        }
        assert!(!S::Unsatisfiable.is_terminal());
        assert!(S::Unsatisfiable.is_settled(), "structurally dead = settled");
        for s in [
            S::Pending,
            S::Blocked,
            S::InProgress,
            S::WaitingReview,
            S::Paused,
        ] {
            assert!(!s.is_terminal(), "{s} not terminal");
            assert!(!s.is_settled(), "{s} not settled");
        }
    }

    /// SSOT drift-guard for the dependency-resolution rule.
    ///
    /// "A `blocked_by` dep is satisfied iff its status ∈ {completed, skipped}"
    /// is expressed as [`CoordTaskStatus::satisfies_dependency`] AND hand-copied
    /// as raw SQL `status (NOT) IN ('completed', 'skipped')` in three store sites
    /// (`store/crud.rs` unresolved_parents, `store/deps.rs` get_newly_unblocked,
    /// `store/row_decode.rs` has_unresolved_deps). Its terminal-dead twin
    /// "{failed, cancelled} → Unsatisfiable" is copied as `IN ('failed',
    /// 'cancelled')` in two sites (`store/crud.rs` dead_parents,
    /// `store/row_decode.rs` has_dead_deps).
    ///
    /// The inner **wildcard-free match** makes this compile-fail the moment a new
    /// `CoordTaskStatus` variant is added, forcing a conscious classification here
    /// — and the string-set assertions pin the exact `as_str()` literals the SQL
    /// depends on, so the enum can never silently drift from those SQL copies.
    /// If this test changes, update every SQL site named above in lockstep.
    #[test]
    fn dependency_resolution_rule_is_pinned_across_all_statuses() {
        use CoordTaskStatus::*;
        let all = [
            Pending,
            Blocked,
            InProgress,
            WaitingReview,
            Completed,
            Failed,
            Cancelled,
            Skipped,
            Paused,
            Unsatisfiable,
        ];

        for s in all {
            // Wildcard-free: a new variant must be classified explicitly.
            match s {
                Completed | Skipped => assert!(
                    s.satisfies_dependency(),
                    "{s} must satisfy deps (SQL IN 'completed','skipped')"
                ),
                Pending | Blocked | InProgress | WaitingReview | Failed | Cancelled | Paused
                | Unsatisfiable => assert!(
                    !s.satisfies_dependency(),
                    "{s} must NOT satisfy deps — would drift from the SQL literal"
                ),
            }
        }

        // Exact satisfying set, as the SQL `IN ('completed', 'skipped')` sees it.
        let satisfying: Vec<&str> = all
            .iter()
            .filter(|s| s.satisfies_dependency())
            .map(|s| s.as_str())
            .collect();
        assert_eq!(satisfying, ["completed", "skipped"]);

        // Exact terminal-dead set, as the SQL `IN ('failed', 'cancelled')` sees it.
        let dead: Vec<&str> = all
            .iter()
            .filter(|s| matches!(s, Failed | Cancelled))
            .map(|s| s.as_str())
            .collect();
        assert_eq!(dead, ["failed", "cancelled"]);
    }

    /// Drift-guard for the RPC/tool status-FILTER vocabulary: every variant's
    /// `as_str()` must round-trip through `from_filter_str` (the hand-rolled
    /// RPC matches used to silently drop `waiting_review`/`paused`/`skipped`
    /// and return the ENTIRE board as if filtered). Wildcard-free `all` list
    /// above compile-fails on a new variant, forcing it into the vocabulary.
    #[test]
    fn status_filter_vocabulary_covers_every_variant() {
        use CoordTaskStatus::*;
        let all = [
            Pending,
            Blocked,
            InProgress,
            WaitingReview,
            Completed,
            Failed,
            Cancelled,
            Skipped,
            Paused,
            Unsatisfiable,
        ];
        for s in all {
            assert_eq!(
                CoordTaskStatus::from_filter_str(s.as_str()),
                Some(s),
                "filter vocabulary must accept {s}"
            );
        }
        assert_eq!(CoordTaskStatus::from_filter_str("bogus"), None);
    }
}
