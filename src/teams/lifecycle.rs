//! Lifecycle management for team agents.
//!
//! Provides shutdown request/approval and idle notification protocols
//! built on top of the existing message routing system.

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::{MessageType, TeamMessage};

/// Manages agent lifecycle within a team — shutdown and idle protocols.
pub struct LifecycleManager {
    msg_router: Arc<MessageRouter>,
    event_store: Arc<dyn EventLogStore>,
}

impl LifecycleManager {
    pub fn new(msg_router: Arc<MessageRouter>, event_store: Arc<dyn EventLogStore>) -> Self {
        Self {
            msg_router,
            event_store,
        }
    }

    /// Agent requests to shut down — sends ShutdownRequest to the leader.
    pub async fn request_shutdown(
        &self,
        team_id: &str,
        from_agent: &str,
        leader_id: &str,
        reason: &str,
    ) -> Result<TeamMessage> {
        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: from_agent.to_string(),
                to: vec![leader_id.to_string()],
                cc: vec![],
                msg_type: MessageType::ShutdownRequest,
                subject: format!("Shutdown request from {from_agent}"),
                content: reason.to_string(),
                reply_to: None,
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::ShutdownRequested,
                agent_id: from_agent.to_string(),
                payload: serde_json::json!({
                    "message_id": msg.id,
                    "reason": reason,
                }),
            })
            .await;

        Ok(msg)
    }

    /// Leader approves a shutdown request.
    pub async fn approve_shutdown(
        &self,
        team_id: &str,
        leader_id: &str,
        agent_id: &str,
        request_msg_id: &str,
    ) -> Result<TeamMessage> {
        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: leader_id.to_string(),
                to: vec![agent_id.to_string()],
                cc: vec![],
                msg_type: MessageType::ShutdownApproved,
                subject: "Shutdown approved".to_string(),
                content: "Your shutdown request has been approved.".to_string(),
                reply_to: Some(request_msg_id.to_string()),
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::ShutdownResolved,
                agent_id: leader_id.to_string(),
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "approved": true,
                }),
            })
            .await;

        Ok(msg)
    }

    /// Leader rejects a shutdown request.
    pub async fn reject_shutdown(
        &self,
        team_id: &str,
        leader_id: &str,
        agent_id: &str,
        request_msg_id: &str,
        reason: &str,
    ) -> Result<TeamMessage> {
        let msg = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: leader_id.to_string(),
                to: vec![agent_id.to_string()],
                cc: vec![],
                msg_type: MessageType::ShutdownRejected,
                subject: "Shutdown rejected".to_string(),
                content: reason.to_string(),
                reply_to: Some(request_msg_id.to_string()),
                attachments: vec![],
            })
            .await?;

        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.to_string(),
                event_type: TeamEventType::ShutdownResolved,
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

    /// Agent reports idle status to the leader.
    pub async fn send_idle(
        &self,
        team_id: &str,
        agent_id: &str,
        leader_id: &str,
        last_task: Option<&str>,
    ) -> Result<TeamMessage> {
        let content = match last_task {
            Some(task) => format!("Idle. Last completed task: {task}"),
            None => "Idle. No tasks completed.".to_string(),
        };

        self.msg_router
            .send(SendRequest {
                team_id: team_id.to_string(),
                from_agent: agent_id.to_string(),
                to: vec![leader_id.to_string()],
                cc: vec![],
                msg_type: MessageType::Idle,
                subject: format!("{agent_id} is idle"),
                content,
                reply_to: None,
                attachments: vec![],
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::{MessageStore, SqliteMessageStore};

    async fn make_lifecycle() -> (LifecycleManager, Arc<SqliteMessageStore>) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let router = Arc::new(MessageRouter::new(
            msg_store.clone(),
            event_store.clone(),
            EscalationRule::default(),
            None,
        ));
        let lm = LifecycleManager::new(router, event_store);
        (lm, msg_store)
    }

    #[tokio::test]
    async fn test_shutdown_request_sends_message_to_leader() {
        let (lm, msg_store) = make_lifecycle().await;
        let msg = lm
            .request_shutdown("team-1", "worker-1", "leader-1", "All tasks done")
            .await
            .unwrap();
        assert_eq!(msg.msg_type, MessageType::ShutdownRequest);
        assert_eq!(msg.from_agent, "worker-1");
        let inbox = msg_store
            .read_inbox("leader-1", "team-1", Some(&MessageType::ShutdownRequest))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    #[tokio::test]
    async fn test_approve_shutdown() {
        let (lm, msg_store) = make_lifecycle().await;
        let req = lm
            .request_shutdown("team-1", "worker-1", "leader-1", "Done")
            .await
            .unwrap();
        let approval = lm
            .approve_shutdown("team-1", "leader-1", "worker-1", &req.id)
            .await
            .unwrap();
        assert_eq!(approval.msg_type, MessageType::ShutdownApproved);
        let inbox = msg_store
            .read_inbox("worker-1", "team-1", Some(&MessageType::ShutdownApproved))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    #[tokio::test]
    async fn test_reject_shutdown() {
        let (lm, msg_store) = make_lifecycle().await;
        let req = lm
            .request_shutdown("team-1", "worker-1", "leader-1", "Done")
            .await
            .unwrap();
        let rejection = lm
            .reject_shutdown(
                "team-1",
                "leader-1",
                "worker-1",
                &req.id,
                "More work needed",
            )
            .await
            .unwrap();
        assert_eq!(rejection.msg_type, MessageType::ShutdownRejected);
        let inbox = msg_store
            .read_inbox("worker-1", "team-1", Some(&MessageType::ShutdownRejected))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    #[tokio::test]
    async fn test_send_idle() {
        let (lm, msg_store) = make_lifecycle().await;
        let msg = lm
            .send_idle("team-1", "worker-1", "leader-1", Some("task-42"))
            .await
            .unwrap();
        assert_eq!(msg.msg_type, MessageType::Idle);
        assert!(msg.content.contains("task-42"));
        let inbox = msg_store
            .read_inbox("leader-1", "team-1", Some(&MessageType::Idle))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
    }
}
