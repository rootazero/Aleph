//! `LifecycleIdleTool` — worker reports it is idle and awaiting work.
//!
//! Wraps a `message_send` of `MessageType::Idle` with auto-resolved
//! leader recipient. Mirrors the `ClawTeam` `clawteam lifecycle idle`
//! command but stays a pure tool — convention-over-config: the leader
//! decides whether to act on the signal, not the harness.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

/// Arguments for reporting a worker as idle.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LifecycleIdleArgs {
    /// Team this worker belongs to
    pub team_id: String,
    /// Short summary of why the worker is idle (e.g. "auth module complete")
    #[serde(default)]
    pub summary: Option<String>,
    /// Optional ID of the task that just completed
    #[serde(default)]
    pub last_task_id: Option<String>,
}

/// Output from a `lifecycle_idle` call.
#[derive(Debug, Clone, Serialize)]
pub struct LifecycleIdleOutput {
    /// `Some(id)` when an idle notification was actually sent to the leader.
    /// `None` when the caller is the leader (no-op) — a non-error so the
    /// LLM doesn't need to special-case its own role.
    pub message_id: Option<String>,
    pub message: String,
}

/// Tool that lets a worker report its idle state to the team leader.
#[derive(Clone)]
pub struct LifecycleIdleTool {
    router: Arc<MessageRouter>,
    team_store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl LifecycleIdleTool {
    pub fn new(
        router: Arc<MessageRouter>,
        team_store: Arc<dyn TeamStore>,
        current_agent_id: String,
    ) -> Self {
        Self {
            router,
            team_store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for LifecycleIdleTool {
    const NAME: &'static str = "lifecycle_idle";
    const DESCRIPTION: &'static str =
        "Report that this worker is idle and awaiting more work. Sends an \
         `idle` message to the team leader; the leader can then assign a new \
         task or approve shutdown. No-op when called by the team leader.";

    type Args = LifecycleIdleArgs;
    type Output = LifecycleIdleOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let team = self
            .team_store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| AlephError::other(format!("Team '{}' not found", args.team_id)))?;

        if team.leader_id == self.current_agent_id {
            return Ok(LifecycleIdleOutput {
                message_id: None,
                message: "Caller is the team leader; idle signal is a no-op".to_string(),
            });
        }

        let summary = args
            .summary
            .as_deref()
            .unwrap_or("ready for next task")
            .to_string();
        let subject = format!("Idle: {summary}");
        let content = format!(
            "Worker `{}` is idle.\n\nLast task: {}\n\n{}",
            self.current_agent_id,
            args.last_task_id.as_deref().unwrap_or("(none)"),
            summary,
        );

        let msg = self
            .router
            .send(SendRequest {
                team_id: args.team_id,
                from_agent: self.current_agent_id.clone(),
                to: vec![team.leader_id.clone()],
                cc: vec![],
                msg_type: MessageType::Idle,
                subject,
                content,
                reply_to: None,
                attachments: vec![],
            })
            .await
            .map_err(|e| AlephError::other(format!("Failed to send idle signal: {e}")))?;

        Ok(LifecycleIdleOutput {
            message_id: Some(msg.id),
            message: format!("Idle signal sent to leader `{}`", team.leader_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::{MessageStore, SqliteMessageStore};
    use crate::teams::store::SqliteTeamStore;
    use crate::teams::types::NewTeam;
    use rusqlite::Connection;

    async fn make_fixture(
        leader_id: &str,
    ) -> (
        Arc<MessageRouter>,
        Arc<SqliteMessageStore>,
        Arc<dyn TeamStore>,
        String, // team_id
    ) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let team_store_raw = SqliteTeamStore::new(Connection::open_in_memory().unwrap());
        team_store_raw.migrate().await.unwrap();
        let team_store: Arc<dyn TeamStore> = Arc::new(team_store_raw);

        let team = team_store
            .create_team(NewTeam {
                name: "Test team".into(),
                description: "Lifecycle idle test".into(),
                leader_id: leader_id.to_string(),
            })
            .await
            .unwrap();

        let router = Arc::new(MessageRouter::new(
            msg_store.clone(),
            event_store,
            EscalationRule::default(),
            Some(leader_id.to_string()),
        ));

        (router, msg_store, team_store, team.id)
    }

    #[tokio::test]
    async fn worker_idle_message_lands_in_leader_inbox() {
        let (router, msg_store, team_store, team_id) = make_fixture("leader-1").await;
        let tool = LifecycleIdleTool::new(router, team_store, "worker-1".into());

        let out = tool
            .call(LifecycleIdleArgs {
                team_id: team_id.clone(),
                summary: Some("auth done".into()),
                last_task_id: Some("task-7".into()),
            })
            .await
            .unwrap();

        assert!(out.message_id.is_some(), "should send an idle message");

        let inbox = msg_store
            .read_inbox("leader-1", &team_id, Some(&MessageType::Idle))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].msg_type, MessageType::Idle);
        assert_eq!(inbox[0].from_agent, "worker-1");
        assert!(inbox[0].subject.contains("auth done"));
        assert!(inbox[0].content.contains("worker-1"));
        assert!(inbox[0].content.contains("task-7"));
    }

    #[tokio::test]
    async fn leader_calling_idle_is_a_noop() {
        let (router, msg_store, team_store, team_id) = make_fixture("leader-1").await;
        let tool = LifecycleIdleTool::new(router, team_store, "leader-1".into());

        let out = tool
            .call(LifecycleIdleArgs {
                team_id: team_id.clone(),
                summary: None,
                last_task_id: None,
            })
            .await
            .unwrap();

        assert!(out.message_id.is_none(), "leader idle must not enqueue");
        let inbox = msg_store
            .read_inbox("leader-1", &team_id, Some(&MessageType::Idle))
            .await
            .unwrap();
        assert!(inbox.is_empty());
    }

    #[tokio::test]
    async fn unknown_team_returns_error() {
        let (router, _msg_store, team_store, _team_id) = make_fixture("leader-1").await;
        let tool = LifecycleIdleTool::new(router, team_store, "worker-1".into());

        let err = tool
            .call(LifecycleIdleArgs {
                team_id: "does-not-exist".into(),
                summary: None,
                last_task_id: None,
            })
            .await
            .expect_err("unknown team must error");
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn idle_default_summary_is_filled_in() {
        let (router, msg_store, team_store, team_id) = make_fixture("leader-1").await;
        let tool = LifecycleIdleTool::new(router, team_store, "worker-1".into());

        tool.call(LifecycleIdleArgs {
            team_id: team_id.clone(),
            summary: None,
            last_task_id: None,
        })
        .await
        .unwrap();

        let inbox = msg_store
            .read_inbox("leader-1", &team_id, Some(&MessageType::Idle))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert!(inbox[0].subject.contains("ready for next task"));
    }
}
