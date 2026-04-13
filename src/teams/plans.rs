//! Plan approval workflow for team agents.
//!
//! Members submit plans as artifacts and request leader approval via messages.

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact, TaskArtifact};
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::{MessageType, TeamMessage};

/// Result of submitting a plan.
#[derive(Debug, Clone)]
pub struct PlanSubmission {
    pub artifact: TaskArtifact,
    pub message: TeamMessage,
}

/// Manages plan submission and approval within a team.
pub struct PlanManager {
    msg_router: Arc<MessageRouter>,
    artifact_store: Arc<dyn ArtifactStore>,
    event_store: Arc<dyn EventLogStore>,
}

impl PlanManager {
    pub fn new(
        msg_router: Arc<MessageRouter>,
        artifact_store: Arc<dyn ArtifactStore>,
        event_store: Arc<dyn EventLogStore>,
    ) -> Self {
        Self {
            msg_router,
            artifact_store,
            event_store,
        }
    }

    /// Submit a plan for leader approval.
    pub async fn submit_plan(
        &self,
        team_id: &str,
        from_agent: &str,
        leader_id: &str,
        title: &str,
        content: &str,
        task_id: &str,
    ) -> Result<PlanSubmission> {
        let artifact = self
            .artifact_store
            .create_artifact(NewArtifact {
                task_id: task_id.to_string(),
                agent_id: from_agent.to_string(),
                artifact_type: ArtifactType::Plan,
                title: title.to_string(),
                content: content.to_string(),
                metadata: serde_json::json!({}),
            })
            .await?;

        let message = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: from_agent.to_string(),
                to: vec![leader_id.to_string()],
                cc: vec![],
                msg_type: MessageType::PlanApprovalRequest,
                subject: format!("Plan approval: {title}"),
                content: format!(
                    "Please review and approve/reject the plan.\nTask: {task_id}\nArtifact: {}",
                    artifact.id
                ),
                reply_to: None,
                attachments: vec![artifact.id.clone()],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::PlanSubmitted,
                agent_id: from_agent.to_string(),
                payload: serde_json::json!({
                    "artifact_id": artifact.id,
                    "message_id": message.id,
                    "task_id": task_id,
                }),
            })
            .await;

        Ok(PlanSubmission { artifact, message })
    }

    /// Leader approves a submitted plan.
    pub async fn approve_plan(
        &self,
        team_id: &str,
        leader_id: &str,
        agent_id: &str,
        plan_msg_id: &str,
        feedback: &str,
    ) -> Result<TeamMessage> {
        let content = if feedback.is_empty() {
            "Plan approved.".to_string()
        } else {
            format!("Plan approved.\n\nFeedback: {feedback}")
        };

        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: leader_id.to_string(),
                to: vec![agent_id.to_string()],
                cc: vec![],
                msg_type: MessageType::PlanApproved,
                subject: "Plan approved".to_string(),
                content,
                reply_to: Some(plan_msg_id.to_string()),
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::PlanResolved,
                agent_id: leader_id.to_string(),
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "approved": true,
                }),
            })
            .await;

        Ok(msg)
    }

    /// Leader rejects a submitted plan.
    pub async fn reject_plan(
        &self,
        team_id: &str,
        leader_id: &str,
        agent_id: &str,
        plan_msg_id: &str,
        reason: &str,
    ) -> Result<TeamMessage> {
        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: leader_id.to_string(),
                to: vec![agent_id.to_string()],
                cc: vec![],
                msg_type: MessageType::PlanRejected,
                subject: "Plan rejected".to_string(),
                content: reason.to_string(),
                reply_to: Some(plan_msg_id.to_string()),
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::PlanResolved,
                agent_id: leader_id.to_string(),
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "approved": false,
                    "reason": reason,
                }),
            })
            .await;

        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::artifacts::{ArtifactStore, SqliteArtifactStore};
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::{MessageStore, SqliteMessageStore};

    async fn make_plan_manager() -> (
        PlanManager,
        Arc<SqliteMessageStore>,
        Arc<SqliteArtifactStore>,
    ) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let artifact_store = Arc::new(SqliteArtifactStore::new_in_memory().await);
        let router = Arc::new(MessageRouter::new(
            msg_store.clone(),
            event_store.clone(),
            EscalationRule::default(),
            None,
        ));
        let pm = PlanManager::new(router, artifact_store.clone(), event_store);
        (pm, msg_store, artifact_store)
    }

    #[tokio::test]
    async fn test_submit_plan_creates_artifact_and_message() {
        let (pm, msg_store, artifact_store) = make_plan_manager().await;
        let submission = pm
            .submit_plan(
                "team-1",
                "worker-1",
                "leader-1",
                "Cache plan",
                "# Plan\n\n1. Benchmark\n2. Implement",
                "task-1",
            )
            .await
            .unwrap();

        assert_eq!(submission.artifact.artifact_type, ArtifactType::Plan);
        assert_eq!(submission.artifact.task_id, "task-1");
        assert_eq!(
            submission.message.msg_type,
            MessageType::PlanApprovalRequest
        );
        assert_eq!(submission.message.attachments.len(), 1);
        assert_eq!(submission.message.attachments[0], submission.artifact.id);

        let inbox = msg_store
            .read_inbox(
                "leader-1",
                "team-1",
                Some(&MessageType::PlanApprovalRequest),
            )
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);

        let stored = artifact_store
            .get_artifact(&submission.artifact.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.artifact_type, ArtifactType::Plan);
    }

    #[tokio::test]
    async fn test_approve_plan() {
        let (pm, msg_store, _) = make_plan_manager().await;
        let submission = pm
            .submit_plan(
                "team-1", "worker-1", "leader-1", "Plan", "Details", "task-1",
            )
            .await
            .unwrap();
        let approval = pm
            .approve_plan(
                "team-1",
                "leader-1",
                "worker-1",
                &submission.message.id,
                "Looks good",
            )
            .await
            .unwrap();
        assert_eq!(approval.msg_type, MessageType::PlanApproved);
        assert!(approval.content.contains("Looks good"));
        let inbox = msg_store
            .read_inbox("worker-1", "team-1", Some(&MessageType::PlanApproved))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    #[tokio::test]
    async fn test_reject_plan() {
        let (pm, msg_store, _) = make_plan_manager().await;
        let submission = pm
            .submit_plan(
                "team-1", "worker-1", "leader-1", "Plan", "Details", "task-1",
            )
            .await
            .unwrap();
        let rejection = pm
            .reject_plan(
                "team-1",
                "leader-1",
                "worker-1",
                &submission.message.id,
                "Missing error handling",
            )
            .await
            .unwrap();
        assert_eq!(rejection.msg_type, MessageType::PlanRejected);
        assert!(rejection.content.contains("Missing error handling"));
        let inbox = msg_store
            .read_inbox("worker-1", "team-1", Some(&MessageType::PlanRejected))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }
}
