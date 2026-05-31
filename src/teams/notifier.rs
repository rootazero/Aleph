//! TeamNotifier — proactive notification of team task outcomes.
//!
//! An [`EventHandler`] that listens for dispatcher task events and routes a
//! message to the team leader's inbox:
//! - a task **failure** alerts the leader immediately;
//! - a task **completion** notifies the leader only once the whole team's work
//!   has reached a terminal state (avoids per-task noise).
//!
//! This is the "AI comes to you" wire (R5): autonomous team progress flows
//! back through the team's own messaging system rather than requiring the user
//! to poll a board.

use async_trait::async_trait;

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::event::{AlephEvent, EventContext, EventHandler, EventType, HandlerError};
use crate::sync_primitives::Arc;
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::teams::store::TeamStore;

/// Synthetic sender id for dispatcher-originated notifications.
const NOTIFIER_SENDER: &str = "team_dispatcher";

/// Routes team task outcomes to the team leader as inbox messages.
pub struct TeamNotifier {
    team_store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    msg_router: Arc<MessageRouter>,
}

impl TeamNotifier {
    pub fn new(
        team_store: Arc<dyn TeamStore>,
        coord_store: Arc<dyn CoordTaskStore>,
        msg_router: Arc<MessageRouter>,
    ) -> Self {
        Self {
            team_store,
            coord_store,
            msg_router,
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
        let send = self
            .msg_router
            .send(SendRequest {
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
        if let Err(e) = send {
            tracing::debug!(team_id, error = %e, "TeamNotifier: leader notification failed");
        }
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
                CoordTaskStatus::Completed | CoordTaskStatus::Failed | CoordTaskStatus::Cancelled
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
        vec![EventType::TeamTaskCompleted, EventType::TeamTaskFailed]
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
            AlephEvent::TeamTaskCompleted {
                team_id,
                task_id,
                result_summary,
            } => {
                // Only notify once the whole team's work is terminal.
                if self.team_work_finished(team_id).await {
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
            }
            // TeamNotifier subscribes exclusively to TeamTaskCompleted and
            // TeamTaskFailed (see `subscriptions()`). All other event variants
            // that reach this handler are intentionally ignored.
            _ => {}
        }
        Ok(vec![])
    }
}
