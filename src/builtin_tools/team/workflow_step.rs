//! `WorkflowStepReviewTool` — step-level workflow control (Phase C).
//!
//! Lets a lead agent (or via panel an end user) approve, reject, or retry
//! a single coordination task. Approval is the openteams "control point"
//! between dependent tasks: a downstream task only unblocks once its
//! upstreams are Completed (or Skipped). The dispatcher does NOT auto-
//! transition `InProgress` → Completed when `lead_review_required` is set
//! on a task — it leaves the run in `WaitingReview` for this tool to
//! resolve.
//!
//! Single tool with an `action` discriminator mirroring `team_snapshot`
//! and `team_acp_member`. Skipped is included as a fourth action so an
//! operator can mark a step as "not required" while still allowing
//! dependents to proceed.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::acceptance::read_acceptance_criteria;
use crate::agents::swarm::tasks::{
    CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, ReviewVerdict, ReviewerKind,
};
use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum WorkflowStepReviewArgs {
    /// Approve the latest run of `task_id`. Transitions the task to
    /// Completed; downstream dependents unblock.
    Approve {
        task_id: String,
        #[serde(default)]
        comment: Option<String>,
        /// A measurement YOU performed backing this approval (not the member's
        /// self-report). Required when the step declared `require_grounding`.
        /// Same closed vocabulary as loop_graph anchors and `task_review`.
        #[serde(default)]
        grounding: Option<crate::builtin_tools::team::task_review::GroundingEvidence>,
    },
    /// Reject the latest run. Transitions the task to Failed; downstream
    /// dependents stay blocked. The comment (if any) is stored as the
    /// task result so the next reviewer can see why.
    Reject {
        task_id: String,
        #[serde(default)]
        comment: Option<String>,
    },
    /// Re-queue a previously failed/rejected step. Status resets to
    /// Pending; prior run history is preserved.
    Retry { task_id: String },
    /// Mark the step as not required. Counts as satisfied for downstream
    /// dependency resolution.
    Skip {
        task_id: String,
        #[serde(default)]
        comment: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStepReviewOutput {
    pub task_id: String,
    pub status: String,
    /// The task's definition of done, echoed back so the reviewer can confirm
    /// the verdict was measured against the declared bar. Empty (and omitted
    /// from the wire) for tasks that declared no acceptance criteria.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
    /// On `retry` of an already-settled step: the settled steps downstream of
    /// it, whose results were produced from the OLD output of this one. Nothing
    /// re-derives them, so re-running this step alone leaves the corrected
    /// result on an edge nobody reads again. Decide per step whether each
    /// needs its own `retry`. Empty (and off the wire) otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub now_stale: Vec<String>,
}

/// What a verdict may be pronounced on.
///
/// A verdict judges *a run that happened*: `WaitingReview` is the parked-for-
/// review state the dispatcher creates, and `InProgress` is admitted because a
/// leader may accept a still-streaming run (the long-standing documented
/// contract on both this tool and the `teams.step.approve` RPC).
///
/// Everything else is rejected rather than blind-written. Approving a `Pending`
/// or `Blocked` step — one that never ran — used to succeed: the task jumped to
/// `Completed` with no result, dependents unblocked, `build_handoff_context`
/// rendered an empty dependency section, and the settle sweep reported the run
/// `✅ finished`. One mistyped id produced a green run with an empty
/// deliverable. Its sibling face `team_task_control` validates every arm; this
/// one validated none.
#[must_use]
pub fn verdict_admissible(status: CoordTaskStatus) -> bool {
    matches!(
        status,
        CoordTaskStatus::WaitingReview | CoordTaskStatus::InProgress
    )
}

/// Subjects of already-settled steps downstream of `task_id`, breadth-first.
///
/// Re-running a settled step makes every settled step below it stale: their
/// prompts were built from the OLD output (`build_handoff_context` reads the
/// upstream's `result` once, at launch), and nothing re-derives them —
/// `get_newly_unblocked` only fires on `Pending` children. The corrected result
/// therefore lands on an edge nobody reads again, and the run reports `✅`.
///
/// The tool REPORTS this rather than acting on it: whether a downstream step
/// must be redone is a judgement about whether the change matters (R7). This is
/// also the first consumer `CoordTaskStore::get_dependents` has ever had.
async fn stale_dependents(store: &Arc<dyn CoordTaskStore>, task_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut frontier = vec![task_id.to_string()];
    // Bounded: each id is expanded at most once, and the DAG is acyclic.
    while let Some(current) = frontier.pop() {
        let Ok(children) = store.get_dependents(&current).await else {
            break;
        };
        for child in children {
            if !seen.insert(child.clone()) {
                continue;
            }
            if let Ok(Some(t)) = store.get_task(&child).await {
                if t.status.is_settled() {
                    out.push(t.subject.clone());
                }
                frontier.push(child);
            }
        }
    }
    out
}

/// The already-in-that-state answer for a verdict, so a double-click is a
/// no-op rather than an error (mirrors `team_task_control`'s idempotent arms).
fn settled_echo(status: CoordTaskStatus, verdict: &WorkflowStepReviewArgs) -> Option<&'static str> {
    match (verdict, status) {
        (WorkflowStepReviewArgs::Approve { .. }, CoordTaskStatus::Completed) => Some("completed"),
        (WorkflowStepReviewArgs::Reject { .. }, CoordTaskStatus::Failed) => Some("failed"),
        _ => None,
    }
}

/// What the pre-flight guard decides for `action='skip'`, given the step's
/// current status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipGuard {
    /// Already skipped — echo the state (a double-click is a no-op).
    AlreadySkipped,
    /// Settled: skipping would relabel a step that finished as "never run".
    RefuseSettled,
    /// Nothing in the way.
    Proceed,
}

/// Classify a skip against the step's status.
///
/// The refusal derives from [`CoordTaskStatus::satisfies_dependency`] rather
/// than a hand-listed `Completed | …`: skipping a step that already satisfies
/// its dependents tells every downstream reader the opposite of the truth
/// ("deliberately not run"), and that is true of any status in that set — the
/// set is owned by the predicate, and a future member of it must inherit the
/// refusal instead of falling through the `_` arm into a silent relabel
/// (criterion 3). `Cancelled` does not satisfy dependents but is an explicit
/// abort, so it is refused on its own terms.
const fn skip_guard(current: CoordTaskStatus) -> SkipGuard {
    if matches!(current, CoordTaskStatus::Skipped) {
        return SkipGuard::AlreadySkipped;
    }
    if current.satisfies_dependency() || matches!(current, CoordTaskStatus::Cancelled) {
        return SkipGuard::RefuseSettled;
    }
    SkipGuard::Proceed
}

#[derive(Clone)]
pub struct WorkflowStepReviewTool {
    coord_store: Arc<dyn CoordTaskStore>,
    current_agent_id: String,
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
}

impl WorkflowStepReviewTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    pub fn new(coord_store: Arc<dyn CoordTaskStore>, current_agent_id: String) -> Self {
        Self {
            coord_store,
            current_agent_id,
            team_store: None,
        }
    }

    /// Wire the ownership gate — see [`crate::teams::task_team_reachable`].
    #[must_use]
    pub fn with_team_store(mut self, store: Option<Arc<dyn crate::teams::TeamStore>>) -> Self {
        self.team_store = store;
        self
    }
}

#[async_trait]
impl AlephTool for WorkflowStepReviewTool {
    const NAME: &'static str = "workflow_step_review";
    const DESCRIPTION: &'static str =
        "Approve / reject / retry / skip a single workflow step. The leader \
         agent invokes this to resolve tasks the dispatcher parked in \
         waiting_review (steps declared with `review: true`); downstream \
         dependents unblock only when an upstream step is Completed or \
         Skipped. When the task declares acceptance_criteria, judge approval \
         against every item — the response echoes the checklist for \
         confirmation. The member's result is a self-report, not a verified \
         fact: before approving claims with external side-effects (files \
         written, requests sent, things published), verify the handle it \
         returned (path, URL, id, status code) yourself; a step declared with \
         `require_grounding` REFUSES an approval that carries no `grounding` \
         measurement (kind: exit_code | numeric | line_count). Retrying a step that \
         already settled returns `now_stale`: the settled steps downstream of \
         it were produced from its OLD output and nothing re-derives them, so \
         decide per listed step whether it needs its own retry.";

    type Args = WorkflowStepReviewArgs;
    type Output = WorkflowStepReviewOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Read the task once, up front: every verdict path echoes back its
        // definition of done, and the retry path needs its current metadata
        // for the budget-anchor stamp. A missing task yields empty criteria
        // (omitted from the wire), keeping the legacy response shape.
        let task_snapshot = {
            let task_id = match &args {
                WorkflowStepReviewArgs::Approve { task_id, .. }
                | WorkflowStepReviewArgs::Reject { task_id, .. }
                | WorkflowStepReviewArgs::Retry { task_id }
                | WorkflowStepReviewArgs::Skip { task_id, .. } => task_id,
            };
            // `Err` must NOT collapse into `None` here: the transition gate
            // below keys on the snapshot, so an unreadable store (the coord
            // store is one `Mutex<Connection>` shared with the dispatcher loop
            // — `SQLITE_BUSY` is an ordinary event) would skip the gate
            // entirely and let a verdict land on a step that never ran. Its
            // RPC twin `verdict_gate` refuses on `Err`; this face must too.
            match self.coord_store.get_task(task_id).await {
                Ok(t) => t,
                Err(e) => {
                    return Err(AlephError::other(format!(
                        "could not read task '{task_id}': {e}"
                    )))
                }
            }
        };
        // Ownership gate, before any verdict is echoed or written.
        //
        // Absent and foreign refuse with ONE error, which also tightens the
        // absent case: this used to fall through to the write path with an
        // empty snapshot, recording a verdict against a task id that does not
        // exist. Mapping "foreign" onto that old tolerance would have meant
        // writing a verdict onto ANOTHER USER's task. Matches the contract its
        // RPC twin `teams.workflow.approve_step` now enforces.
        {
            let task_id = match &args {
                WorkflowStepReviewArgs::Approve { task_id, .. }
                | WorkflowStepReviewArgs::Reject { task_id, .. }
                | WorkflowStepReviewArgs::Retry { task_id }
                | WorkflowStepReviewArgs::Skip { task_id, .. } => task_id,
            };
            let reachable = match task_snapshot.as_ref() {
                None => false,
                Some(t) => {
                    crate::teams::task_team_reachable(
                        self.team_store.as_ref(),
                        t.team_id.as_deref(),
                    )
                    .await
                }
            };
            if !reachable {
                return Err(AlephError::invalid_input(format!(
                    "task '{task_id}' not found"
                )));
            }
        }
        let criteria = task_snapshot
            .as_ref()
            .map(|t| read_acceptance_criteria(&t.metadata))
            .unwrap_or_default();

        // Validate the transition before writing anything (the discipline
        // `team_task_control` already applies to all five of its arms).
        // `Retry` and `Skip` are recovery hatches with their own admissible
        // sets; the two verdicts share `verdict_admissible`.
        if let Some(current) = task_snapshot.as_ref().map(|t| t.status) {
            match &args {
                WorkflowStepReviewArgs::Approve { task_id, .. }
                | WorkflowStepReviewArgs::Reject { task_id, .. } => {
                    if let Some(echo) = settled_echo(current, &args) {
                        return Ok(WorkflowStepReviewOutput {
                            task_id: task_id.clone(),
                            status: echo.into(),
                            acceptance_criteria: criteria,
                            now_stale: Vec::new(),
                        });
                    }
                    if !verdict_admissible(current) {
                        return Err(AlephError::invalid_input(format!(
                            "cannot review a step in status '{current}' — a verdict judges a run \
                             that happened, and this one has not produced one (use \
                             action='retry' to queue it, or action='skip' to waive it)"
                        )));
                    }
                }
                WorkflowStepReviewArgs::Retry { .. } => {
                    if current == CoordTaskStatus::InProgress {
                        return Err(AlephError::invalid_input(
                            "cannot retry an in-progress step — cancel it first".to_string(),
                        ));
                    }
                }
                WorkflowStepReviewArgs::Skip { task_id, .. } => match skip_guard(current) {
                    SkipGuard::AlreadySkipped => {
                        return Ok(WorkflowStepReviewOutput {
                            task_id: task_id.clone(),
                            status: "skipped".into(),
                            acceptance_criteria: criteria,
                            now_stale: Vec::new(),
                        });
                    }
                    SkipGuard::RefuseSettled => {
                        return Err(AlephError::invalid_input(format!(
                            "cannot skip a step in status '{current}' — it already settled \
                             (use action='retry' for a fresh attempt)"
                        )));
                    }
                    SkipGuard::Proceed => {}
                },
            }
        }

        match args {
            WorkflowStepReviewArgs::Approve {
                task_id,
                comment,
                grounding,
            } => {
                debug!(task_id = %task_id, "workflow_step_review: approve");
                // Anchor gate: a step declared with `require_grounding` may not
                // be approved on the reviewer's say-so. Same closed vocabulary
                // and same bounce shape as `task_review` — one anchor language
                // system-wide. Rejection is never gated (it is conservative).
                if let Some(g) = &grounding {
                    if !crate::builtin_tools::team::task_review::grounding_kind_valid(&g.kind) {
                        return Err(AlephError::tool(format!(
                            "workflow_step_review: grounding.kind '{}' invalid — must be one \
                             of exit_code | numeric | line_count",
                            g.kind
                        )));
                    }
                }
                let meta = task_snapshot
                    .as_ref()
                    .map(|t| t.metadata.clone())
                    .unwrap_or(serde_json::Value::Null);
                if crate::builtin_tools::team::task_review::needs_grounding_bounce(
                    crate::builtin_tools::team::task_review::ReviewDecision::Approve,
                    &meta,
                    grounding.is_some(),
                ) {
                    return Ok(WorkflowStepReviewOutput {
                        task_id,
                        status: "grounding_required".into(),
                        acceptance_criteria: criteria,
                        now_stale: Vec::new(),
                    });
                }
                self.coord_store
                    .record_run_review(
                        &task_id,
                        ReviewVerdict::Approved,
                        ReviewerKind::LeadAgent,
                        Some(&self.actor()),
                    )
                    .await?;
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Completed),
                            ..Default::default()
                        },
                    )
                    .await?;
                if let Some(c) = comment.as_deref().filter(|c| !c.trim().is_empty()) {
                    let _ = self
                        .coord_store
                        .add_task_comment(&task_id, &self.actor(), c)
                        .await;
                }
                if let Some(g) = &grounding {
                    // Same durable, migration-free evidence trail `task_review`
                    // writes, so an auditor can later check the approval
                    // touched reality.
                    let line = format!(
                        "[grounding] kind={} source={} value={}{}",
                        g.kind,
                        g.source,
                        g.value,
                        g.note
                            .as_deref()
                            .map(|n| format!(" note={n}"))
                            .unwrap_or_default()
                    );
                    let _ = self
                        .coord_store
                        .add_task_comment(&task_id, &self.actor(), &line)
                        .await;
                }
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "completed".into(),
                    acceptance_criteria: criteria,
                    now_stale: Vec::new(),
                })
            }
            WorkflowStepReviewArgs::Reject { task_id, comment } => {
                debug!(task_id = %task_id, "workflow_step_review: reject");
                self.coord_store
                    .record_run_review(
                        &task_id,
                        ReviewVerdict::Rejected,
                        ReviewerKind::LeadAgent,
                        Some(&self.actor()),
                    )
                    .await?;
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Failed),
                            result: comment.clone(),
                            ..Default::default()
                        },
                    )
                    .await?;
                if let Some(c) = comment.as_deref().filter(|c| !c.trim().is_empty()) {
                    let _ = self
                        .coord_store
                        .add_task_comment(&task_id, &self.actor(), c)
                        .await;
                }
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "failed".into(),
                    acceptance_criteria: criteria,
                    now_stale: Vec::new(),
                })
            }
            WorkflowStepReviewArgs::Retry { task_id } => {
                debug!(task_id = %task_id, "workflow_step_review: retry");
                // Snapshot the leftover lock holder BEFORE the reset so it can
                // be released below with its actual holder — releasing with ""
                // never clears a genuinely held lock (the store checks holder
                // equality) and would leave the retried task
                // Pending-but-unschedulable until release_stale_locks fires.
                let locked_by = task_snapshot.as_ref().and_then(|t| t.locked_by.clone());
                // Re-running an already-settled step invalidates everything
                // settled below it — read the fan-out BEFORE the reset, while
                // the graph still describes the state being replaced.
                let now_stale = if task_snapshot
                    .as_ref()
                    .is_some_and(|t| t.status.is_settled())
                {
                    stale_dependents(&self.coord_store, &task_id).await
                } else {
                    Vec::new()
                };
                // A reviewer-driven retry is a deliberate re-queue: stamp the
                // budget anchor so the automatic retry ladder re-arms instead
                // of dying on the first new failure (mirrors
                // `team_task_control.retry`).
                let metadata = task_snapshot.map(|t| {
                    crate::agents::swarm::tasks::retry::with_retry_budget_reset_at(
                        t.metadata,
                        chrono::Utc::now().timestamp().max(0) as u64,
                    )
                });
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Pending),
                            result: Some(String::new()),
                            metadata,
                            ..Default::default()
                        },
                    )
                    .await?;
                if let Some(holder) = locked_by.as_deref() {
                    if let Err(e) = self.coord_store.release_lock(&task_id, holder).await {
                        tracing::warn!(task_id = %task_id, holder = %holder, error = %e, "workflow_step_review: retry could not release leftover lock");
                    }
                }
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "pending".into(),
                    acceptance_criteria: criteria,
                    now_stale,
                })
            }
            WorkflowStepReviewArgs::Skip { task_id, comment } => {
                debug!(task_id = %task_id, "workflow_step_review: skip");
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Skipped),
                            ..Default::default()
                        },
                    )
                    .await?;
                if let Some(c) = comment.as_deref().filter(|c| !c.trim().is_empty()) {
                    let _ = self
                        .coord_store
                        .add_task_comment(&task_id, &self.actor(), c)
                        .await;
                }
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "skipped".into(),
                    acceptance_criteria: criteria,
                    now_stale: Vec::new(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard that stops a verdict landing on a step that never ran. Its
    /// sibling face `team_task_control` validates every arm; this one validated
    /// none, so approving a `Pending` step completed it with no result,
    /// unblocked its dependents with an empty fan-in section, and let the
    /// settle sweep report the run `✅ finished`.
    #[test]
    fn only_a_run_that_happened_can_be_reviewed() {
        assert!(verdict_admissible(CoordTaskStatus::WaitingReview));
        assert!(verdict_admissible(CoordTaskStatus::InProgress));
        for never_ran in [
            CoordTaskStatus::Pending,
            CoordTaskStatus::Blocked,
            CoordTaskStatus::Paused,
            CoordTaskStatus::Unsatisfiable,
        ] {
            assert!(
                !verdict_admissible(never_ran),
                "{never_ran:?} produced no run to judge"
            );
        }
        for already_settled in [
            CoordTaskStatus::Completed,
            CoordTaskStatus::Failed,
            CoordTaskStatus::Cancelled,
            CoordTaskStatus::Skipped,
        ] {
            assert!(
                !verdict_admissible(already_settled),
                "{already_settled:?} is settled — a re-verdict is retry/skip territory"
            );
        }
    }

    /// Drift guard over the WHOLE status vocabulary: a skip must be refused
    /// for every status that already satisfies dependents (relabelling one as
    /// "deliberately not run" tells downstream the opposite of the truth) plus
    /// the explicit abort, echoed for an already-skipped step, and admitted
    /// otherwise.
    ///
    /// What makes it red: a new `CoordTaskStatus` variant (the wildcard-free
    /// match compile-fails first), or a `skip_guard` that stops deriving its
    /// refusal from `satisfies_dependency` and starts hand-listing again.
    #[test]
    fn skip_is_refused_for_every_status_that_already_satisfies_dependents() {
        use CoordTaskStatus::*;
        for status in [
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
        ] {
            // Wildcard-free: a new variant must be classified consciously.
            let expected = match status {
                Skipped => SkipGuard::AlreadySkipped,
                Completed | Cancelled => SkipGuard::RefuseSettled,
                Pending | Blocked | InProgress | WaitingReview | Failed | Paused
                | Unsatisfiable => SkipGuard::Proceed,
            };
            assert_eq!(skip_guard(status), expected, "skip guard for {status}");
            if status.satisfies_dependency() {
                assert_ne!(
                    skip_guard(status),
                    SkipGuard::Proceed,
                    "{status} satisfies dependents — a skip would relabel work that landed"
                );
            }
        }
    }

    /// A repeated verdict is a no-op, not an error (the idempotent shape
    /// `team_task_control` uses) — but only for the matching verdict.
    #[test]
    fn repeating_a_verdict_echoes_instead_of_erroring() {
        let approve = WorkflowStepReviewArgs::Approve {
            task_id: "t".into(),
            comment: None,
            grounding: None,
        };
        let reject = WorkflowStepReviewArgs::Reject {
            task_id: "t".into(),
            comment: None,
        };
        assert_eq!(
            settled_echo(CoordTaskStatus::Completed, &approve),
            Some("completed")
        );
        assert_eq!(
            settled_echo(CoordTaskStatus::Failed, &reject),
            Some("failed")
        );
        assert_eq!(settled_echo(CoordTaskStatus::Failed, &approve), None);
        assert_eq!(settled_echo(CoordTaskStatus::Completed, &reject), None);
    }
}
