//! `TaskReviewTool` — the team leader's explicit acceptance/verification tool
//! (strategy round 2, group-chat path). After reading a member's submitted
//! deliverable (`task_read_artifact`), the leader accepts or rejects the owning
//! coord_task: approve → Completed (downstream dependents unblock); reject →
//! InProgress (the owner redoes it; the leader's feedback rides along). Mirrors
//! `WorkflowStepReviewTool` but uses a flat verdict arg and carries a soft
//! leader-only guard (a non-leader caller is a no-op — prompt-gating is the
//! primary gate; this is defense-in-depth, R7/R9).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::agents::swarm::tasks::acceptance::require_grounding;
use crate::agents::swarm::tasks::{
    CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, ReviewVerdict, ReviewerKind,
};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

/// Accept / reject discriminator (snake_case on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    Reject,
}

/// Reviewer-side anchor evidence backing an approval — a real measurement the
/// reviewer performed (not the submitter's self-report). `kind` uses the same
/// closed truth vocabulary as loop_graph anchor nodes (one anchor language
/// system-wide).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GroundingEvidence {
    /// exit_code | numeric | line_count
    pub kind: String,
    /// The real command run / independent data source consulted
    pub source: String,
    /// The measured value (exit code, number, line count)
    pub value: String,
    /// Optional context note
    #[serde(default)]
    pub note: Option<String>,
}

/// Closed grounding vocabulary — aligned with the loop_graph anchor truth set.
fn grounding_kind_valid(kind: &str) -> bool {
    matches!(kind, "exit_code" | "numeric" | "line_count")
}

/// Whether this review call must be bounced for missing grounding evidence.
/// Only approvals are gated: a rejection is naturally conservative. Pure /
/// host-testable.
#[must_use]
fn needs_grounding_bounce(
    decision: ReviewDecision,
    metadata: &serde_json::Value,
    has_grounding: bool,
) -> bool {
    matches!(decision, ReviewDecision::Approve) && require_grounding(metadata) && !has_grounding
}

/// Arguments for `task_review`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskReviewArgs {
    /// The coord_task to accept or reject — the id you handed the member when
    /// assigning, which the member echoes back on submit.
    pub task_id: String,
    /// `approve` → the task is completed and downstream dependents unblock;
    /// `reject` → the task returns to in_progress for the owner to redo.
    pub decision: ReviewDecision,
    /// Optional feedback, stored on the task (shown to the owner on a reject).
    #[serde(default)]
    pub feedback: Option<String>,
    /// Grounding evidence backing an approval (a measurement you ran yourself,
    /// or one collected via subagent(agent_type='loop-auditor')). Required to
    /// approve when the task carries `require_grounding: true`.
    #[serde(default)]
    pub grounding: Option<GroundingEvidence>,
}

/// Output from `task_review`.
#[derive(Debug, Clone, Serialize)]
pub struct TaskReviewOutput {
    pub task_id: String,
    pub status: String,
    /// On approve: the subjects of tasks whose dependencies are now all
    /// satisfied, so the leader knows what to assign next. Empty on reject /
    /// when nothing newly unblocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub newly_unblocked: Vec<String>,
    /// Human-facing note (e.g. why a non-leader call was ignored).
    pub message: String,
}

/// Map a verdict to the coord_task status it sets. `Approve` → Completed
/// (satisfies downstream deps); `Reject` → InProgress (re-queue for the owner —
/// strategy round 2 chose redo-in-place over the sibling `workflow_step_review`'s
/// terminal Failed). Pure / host-testable.
#[must_use]
fn target_status(decision: ReviewDecision) -> CoordTaskStatus {
    match decision {
        ReviewDecision::Approve => CoordTaskStatus::Completed,
        ReviewDecision::Reject => CoordTaskStatus::InProgress,
    }
}

/// Soft leader-only guard. `leader` is the resolved team leader (None when the
/// task carries no team / the team can't be read — then we can't verify, so we
/// allow and lean on prompt-gating). Pure / host-testable.
#[must_use]
fn is_authorized(caller: &str, leader: Option<&str>) -> bool {
    leader.is_none_or(|l| l == caller)
}

#[derive(Clone)]
pub struct TaskReviewTool {
    coord_store: Arc<dyn CoordTaskStore>,
    team_store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl TaskReviewTool {
    pub fn new(
        coord_store: Arc<dyn CoordTaskStore>,
        team_store: Arc<dyn TeamStore>,
        current_agent_id: String,
    ) -> Self {
        Self {
            coord_store,
            team_store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for TaskReviewTool {
    const NAME: &'static str = "task_review";
    const DESCRIPTION: &'static str =
        "Accept or reject a team member's submitted deliverable (leader only). \
         Call this after reading the member's artifact (task_read_artifact) for \
         a task the member submitted: decision='approve' marks the task \
         completed and unblocks dependents; decision='reject' sends it back to \
         in_progress for the owner to redo — put what to fix in `feedback`. The \
         member's result is a self-report, not a verified fact: before approving \
         claims with external side-effects (files written, requests sent, things \
         published), verify the handle it returned (path, URL, id) yourself. \
         Tasks created with require_grounding=true bounce approvals lacking the \
         `grounding` evidence field (kind: exit_code | numeric | line_count).";

    type Args = TaskReviewArgs;
    type Output = TaskReviewOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "task_review(task_id='task-3', decision='approve')".to_string(),
            "task_review(task_id='task-3', decision='reject', feedback='缺少错误处理,补上再交')"
                .to_string(),
            "task_review(task_id='task-3', decision='approve', grounding={kind:'exit_code', \
             source:'cargo test -p alephcore --lib', value:'0'})"
                .to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Resolve the task + its team leader for the soft authz check. A missing
        // task is a graceful no-op (the id may be freeform, not a coord_task).
        let Some(task) = self
            .coord_store
            .get_task(&args.task_id)
            .await
            .ok()
            .flatten()
        else {
            return Ok(TaskReviewOutput {
                task_id: args.task_id,
                status: "not_found".into(),
                newly_unblocked: Vec::new(),
                message: "no coord_task with that id — nothing to review".into(),
            });
        };
        let leader = match task.team_id.as_deref() {
            Some(team_id) => self
                .team_store
                .get_team(team_id)
                .await
                .ok()
                .flatten()
                .map(|t| t.leader_id),
            None => None,
        };
        if !is_authorized(&self.current_agent_id, leader.as_deref()) {
            warn!(
                task_id = %args.task_id,
                caller = %self.current_agent_id,
                "task_review ignored: caller is not the team leader"
            );
            return Ok(TaskReviewOutput {
                task_id: args.task_id,
                status: "forbidden".into(),
                newly_unblocked: Vec::new(),
                message: "only the team leader can review tasks — ignored".into(),
            });
        }

        if let Some(g) = &args.grounding {
            if !grounding_kind_valid(&g.kind) {
                return Err(AlephError::tool(format!(
                    "task_review: grounding.kind '{}' invalid — must be one of \
                     exit_code | numeric | line_count",
                    g.kind
                )));
            }
        }
        if needs_grounding_bounce(args.decision, &task.metadata, args.grounding.is_some()) {
            return Ok(TaskReviewOutput {
                task_id: args.task_id,
                status: "grounding_required".into(),
                newly_unblocked: Vec::new(),
                message: "this task requires grounding evidence to approve: run a real \
                          measurement yourself (test/probe → exit_code, count → numeric, \
                          output size → line_count) — or spawn \
                          subagent(agent_type='loop-auditor') to collect it independently \
                          — then re-call task_review with the `grounding` field filled"
                    .into(),
            });
        }

        let status = target_status(args.decision);
        debug!(task_id = %args.task_id, ?status, "task_review verdict");

        let verdict = match args.decision {
            ReviewDecision::Approve => ReviewVerdict::Approved,
            ReviewDecision::Reject => ReviewVerdict::Rejected,
        };
        let _ = self
            .coord_store
            .record_run_review(
                &args.task_id,
                verdict,
                ReviewerKind::LeadAgent,
                Some(&self.current_agent_id),
            )
            .await;
        self.coord_store
            .update_task(
                &args.task_id,
                CoordTaskUpdate {
                    status: Some(status),
                    result: args.feedback.clone(),
                    ..Default::default()
                },
            )
            .await?;
        if let Some(fb) = args.feedback.as_deref().filter(|f| !f.trim().is_empty()) {
            let _ = self
                .coord_store
                .add_task_comment(&args.task_id, &self.current_agent_id, fb)
                .await;
        }
        if let Some(g) = &args.grounding {
            // Durable, migration-free evidence trail: ride the task-comment
            // channel so auditors / loop-auditor can later verify the review
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
                .add_task_comment(&args.task_id, &self.current_agent_id, &line)
                .await;
        }

        let newly_unblocked = match args.decision {
            ReviewDecision::Approve => self
                .coord_store
                .get_newly_unblocked(&args.task_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.subject)
                .collect(),
            ReviewDecision::Reject => Vec::new(),
        };

        let message = match args.decision {
            ReviewDecision::Approve => "approved → completed".to_string(),
            ReviewDecision::Reject => "rejected → back to in_progress".to_string(),
        };
        Ok(TaskReviewOutput {
            task_id: args.task_id,
            status: status.as_str().to_string(),
            newly_unblocked,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_status_maps_verdict() {
        assert_eq!(
            target_status(ReviewDecision::Approve),
            CoordTaskStatus::Completed
        );
        assert_eq!(
            target_status(ReviewDecision::Reject),
            CoordTaskStatus::InProgress
        );
    }

    #[test]
    fn authz_allows_leader_blocks_other_allows_unknown() {
        assert!(is_authorized("alice", Some("alice")), "leader may review");
        assert!(
            !is_authorized("bob", Some("alice")),
            "non-leader is blocked"
        );
        assert!(
            is_authorized("bob", None),
            "unverifiable team → allow (prompt-gating)"
        );
    }

    #[test]
    fn decision_parses_snake_case() {
        let a: ReviewDecision = serde_json::from_str("\"approve\"").unwrap();
        let r: ReviewDecision = serde_json::from_str("\"reject\"").unwrap();
        assert_eq!(a, ReviewDecision::Approve);
        assert_eq!(r, ReviewDecision::Reject);
    }

    #[test]
    fn grounding_bounce_matrix() {
        use serde_json::json;
        let gated = json!({ "require_grounding": true });
        let open = json!({});
        // require on + approve + no evidence → bounce
        assert!(needs_grounding_bounce(
            ReviewDecision::Approve,
            &gated,
            false
        ));
        // require on + approve + evidence → pass
        assert!(!needs_grounding_bounce(
            ReviewDecision::Approve,
            &gated,
            true
        ));
        // require on + reject + no evidence → pass (rejection is conservative)
        assert!(!needs_grounding_bounce(
            ReviewDecision::Reject,
            &gated,
            false
        ));
        // require off + approve + no evidence → pass
        assert!(!needs_grounding_bounce(
            ReviewDecision::Approve,
            &open,
            false
        ));
    }

    #[test]
    fn grounding_kind_is_closed_vocabulary() {
        for k in ["exit_code", "numeric", "line_count"] {
            assert!(grounding_kind_valid(k));
        }
        assert!(!grounding_kind_valid("vibes"));
        assert!(!grounding_kind_valid(""));
    }
}
