//! `TeamNotifier` — proactive notification of team task outcomes.
//!
//! An [`EventHandler`] that listens for dispatcher task events and routes a
//! message to the team leader's inbox:
//! - a task **failure** alerts the leader immediately;
//! - a task parked in **`waiting_review`** alerts the leader immediately — the
//!   review gate only resolves through `workflow_step_review`, so without
//!   this nudge a gated DAG would stall silently;
//! - a task **completion** notifies the leader only once the whole team's work
//!   has reached a terminal state (avoids per-task noise).
//!
//! This is the "AI comes to you" wire (R5): autonomous team progress flows
//! back through the team's own messaging system rather than requiring the user
//! to poll a board.
//!
//! ## Once-only completion, coalesced failures
//!
//! The dispatcher runs team tasks **concurrently**, so the final batch of tasks
//! can reach a terminal state within the same instant: every `TeamTaskCompleted`
//! handler then observes `team_work_finished() == true`. Two layers guard the
//! leader's inbox against the resulting burst:
//!
//! - **Completion is idempotent per team.** A `completed_teams` claim set lets
//!   exactly one handler win the terminal "Team work complete" notification; the
//!   rest short-circuit. This is true once-only delivery — the leader receives a
//!   single message with a single task summary, not N near-identical blocks.
//! - **Failure storms coalesce.** Outbound traffic still flows through the
//!   [`Aggregator`] rather than the raw router. When N tasks fail in the same
//!   instant the per-task alerts share the `(team_id, from, to, msg_type,
//!   subject)` key, so they merge into one delivery inside the flush window
//!   instead of N separate alerts. Decision-loaded traffic (approvals/shutdowns)
//!   bypasses batching automatically; the dispatcher only ever emits
//!   `SystemNotification`.

use async_trait::async_trait;
use std::collections::HashSet;

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::event::{AlephEvent, EventContext, EventHandler, EventType, HandlerError};
use crate::sync_primitives::{Arc, Mutex};
use crate::teams::messages::aggregator::Aggregator;
use crate::teams::messages::router::SendRequest;
use crate::teams::messages::types::MessageType;
use crate::teams::store::TeamStore;

/// Synthetic sender id for dispatcher-originated notifications.
const NOTIFIER_SENDER: &str = "team_dispatcher";

/// Routes team task outcomes to the team leader as inbox messages.
pub struct TeamNotifier {
    team_store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    /// Batched outbound sink. Wraps the team
    /// [`MessageRouter`](crate::teams::messages::MessageRouter) so that a
    /// failure storm (N tasks failing at once) coalesces into one leader
    /// delivery. See the module docs.
    sink: Arc<Aggregator>,
    /// Teams whose terminal "Team work complete" notification has already been
    /// claimed. Guards the concurrent dispatcher from firing the completion
    /// message once per simultaneously-finishing task; the first handler to
    /// insert a team id wins, the rest short-circuit. Monotonic — a team id is
    /// only ever inserted once, bounding growth by the number of distinct teams
    /// that complete over the daemon's lifetime.
    completed_teams: Mutex<HashSet<String>>,
}

impl TeamNotifier {
    pub fn new(
        team_store: Arc<dyn TeamStore>,
        coord_store: Arc<dyn CoordTaskStore>,
        sink: Arc<Aggregator>,
    ) -> Self {
        Self {
            team_store,
            coord_store,
            sink,
            completed_teams: Mutex::new(HashSet::new()),
        }
    }

    /// Send a system notification to the team's leader (best-effort).
    async fn notify_leader(&self, team_id: &str, subject: &str, content: String) {
        if team_id.is_empty() {
            return; // teamless task — nobody to notify
        }
        let leader = match self.team_store.get_team(team_id).await {
            Ok(Some(team)) => team.leader_id,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!(team_id, error = %e, "TeamNotifier: get_team failed");
                return;
            }
        };
        // Batched, fire-and-forget: same-subject bursts collapse to one delivery
        // inside the flush window. Errors during the deferred flush are logged
        // by the aggregator itself (the caller has no thread to receive them).
        self.sink
            .send_batched(SendRequest {
                team_id: team_id.to_string(),
                from_agent: NOTIFIER_SENDER.to_string(),
                to: vec![leader],
                cc: vec![],
                msg_type: MessageType::SystemNotification,
                subject: subject.to_string(),
                content,
                reply_to: None,
                attachments: vec![],
            })
            .await;
    }

    /// Whether every task on the team has reached a terminal state.
    async fn team_work_finished(&self, team_id: &str) -> bool {
        let tasks = match self
            .coord_store
            .list_tasks(CoordTaskFilter {
                team_id: Some(team_id.to_string()),
                ..Default::default()
            })
            .await
        {
            Ok(t) => t,
            Err(_) => return false,
        };
        if tasks.is_empty() {
            return false;
        }
        tasks.iter().all(|t| {
            matches!(
                t.status,
                CoordTaskStatus::Completed
                    | CoordTaskStatus::Failed
                    | CoordTaskStatus::Cancelled
                    | CoordTaskStatus::Skipped
            )
        })
    }
}

#[async_trait]
impl EventHandler for TeamNotifier {
    fn name(&self) -> &'static str {
        "TeamNotifier"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::TeamTaskCompleted,
            EventType::TeamTaskFailed,
            // Carries the "waiting_review" status transition (review-gated
            // tasks parked by the dispatcher). All other status updates on
            // this event type are ignored in `handle`.
            EventType::TeamTaskUpdated,
        ]
    }

    async fn handle(
        &self,
        event: &AlephEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        match event {
            AlephEvent::TeamTaskFailed {
                team_id,
                task_id,
                error,
            } => {
                self.notify_leader(
                    team_id,
                    "Team task failed",
                    format!("Task `{task_id}` failed.\n\n{error}"),
                )
                .await;
            }
            // Review-gated task parked by the dispatcher. The gate only
            // resolves through `workflow_step_review`, so the leader must be
            // told now — otherwise the DAG stalls with nobody polling the
            // board. Other status transitions on this event are noise here.
            AlephEvent::TeamTaskUpdated {
                team_id,
                task_id,
                status,
                ..
            } if status.as_str() == "waiting_review" => {
                self.notify_leader(
                    team_id,
                    "Team task awaiting review",
                    format!(
                        "Task `{task_id}` finished its run and is waiting for \
                         your review. The member's output is a self-report, \
                         not a verified fact — check it against the task's \
                         acceptance criteria (and verify any claimed \
                         side-effects via their handles: URLs, paths, ids) \
                         before deciding. Resolve with `workflow_step_review` \
                         (approve / reject / retry / skip); downstream steps \
                         stay blocked until you do."
                    ),
                )
                .await;
            }
            AlephEvent::TeamTaskCompleted {
                team_id,
                task_id,
                result_summary,
            }
                // Only notify once the whole team's work is terminal.
                if self.team_work_finished(team_id).await => {
                    // Claim the terminal notification exactly once per team.
                    // Under concurrent dispatch the final tasks finish in the
                    // same instant and each observes `team_work_finished()`;
                    // only the first handler to insert the id proceeds. The
                    // guard is dropped before the await — no lock held across
                    // `.await`.
                    let first_claim = {
                        let mut done = self
                            .completed_teams
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        done.insert(team_id.clone())
                    };
                    if !first_claim {
                        return Ok(vec![]);
                    }
                    let summary = result_summary.as_deref().unwrap_or("");
                    self.notify_leader(
                        team_id,
                        "Team work complete",
                        format!(
                            "All tasks for this team have finished. \
                             Last completed task: `{task_id}`.\n\n{summary}"
                        ),
                    )
                    .await;
                }
            // TeamNotifier subscribes exclusively to TeamTaskCompleted and
            // TeamTaskFailed (see `subscriptions()`). All other event variants
            // that reach this handler are intentionally ignored.
            _ => {}
        }
        Ok(vec![])
    }
}
