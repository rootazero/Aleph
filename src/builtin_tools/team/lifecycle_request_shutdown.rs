//! `LifecycleRequestShutdownTool` — worker asks the leader for permission
//! to terminate. Mirrors `ClawTeam` `lifecycle request-shutdown` and pairs
//! with [`LifecycleResolveShutdownTool`].

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

/// Arguments for requesting a worker shutdown.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LifecycleRequestShutdownArgs {
    /// Team this worker belongs to
    pub team_id: String,
    /// Reason for requesting shutdown
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleRequestShutdownOutput {
    /// Message ID of the shutdown request — pass to `lifecycle_resolve_shutdown`
    pub message_id: String,
    pub message: String,
}

#[derive(Clone)]
pub struct LifecycleRequestShutdownTool {
    router: Arc<MessageRouter>,
    team_store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl LifecycleRequestShutdownTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

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
impl AlephTool for LifecycleRequestShutdownTool {
    const NAME: &'static str = "lifecycle_request_shutdown";
    const DESCRIPTION: &'static str =
        "Request the team leader's permission to terminate this worker. Sends a \
         `shutdown_request` message; the leader replies via `lifecycle_resolve_shutdown`. \
         Do not exit until the leader approves.";

    type Args = LifecycleRequestShutdownArgs;
    type Output = LifecycleRequestShutdownOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let team = self
            .team_store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| AlephError::other(format!("Team '{}' not found", args.team_id)))?;

        if team.leader_id == self.actor() {
            return Err(AlephError::tool(
                "lifecycle_request_shutdown: caller is the team leader; \
                 a leader cannot request its own shutdown via the team mailbox",
            ));
        }

        let reason = args.reason.trim();
        if reason.is_empty() {
            return Err(AlephError::tool(
                "lifecycle_request_shutdown: `reason` cannot be empty",
            ));
        }

        let msg = self
            .router
            .send(SendRequest {
                team_id: args.team_id,
                from_agent: self.actor(),
                to: vec![team.leader_id.clone()],
                cc: vec![],
                msg_type: MessageType::ShutdownRequest,
                subject: format!("Shutdown request: {}", self.actor()),
                content: format!(
                    "Worker `{}` requests shutdown.\n\nReason:\n{reason}",
                    self.actor()
                ),
                reply_to: None,
                attachments: vec![],
            })
            .await
            .map_err(|e| AlephError::other(format!("Failed to send shutdown request: {e}")))?;

        Ok(LifecycleRequestShutdownOutput {
            message_id: msg.id,
            message: format!("Shutdown request sent to leader `{}`", team.leader_id),
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
        String,
    ) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let team_store_raw = SqliteTeamStore::new(Connection::open_in_memory().unwrap());
        team_store_raw.migrate().await.unwrap();
        let team_store: Arc<dyn TeamStore> = Arc::new(team_store_raw);

        let team = team_store
            .create_team(NewTeam {
                name: "Test team".into(),
                description: "Shutdown test".into(),
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
    async fn worker_shutdown_request_reaches_leader_inbox() {
        let (router, msg_store, team_store, team_id) = make_fixture("leader-1").await;
        let tool = LifecycleRequestShutdownTool::new(router, team_store, "worker-1".into());

        let out = tool
            .call(LifecycleRequestShutdownArgs {
                team_id: team_id.clone(),
                reason: "all tasks complete".into(),
            })
            .await
            .unwrap();
        assert!(!out.message_id.is_empty());

        let inbox = msg_store
            .read_inbox("leader-1", &team_id, Some(&MessageType::ShutdownRequest))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].msg_type, MessageType::ShutdownRequest);
        assert_eq!(inbox[0].from_agent, "worker-1");
        assert!(inbox[0].content.contains("all tasks complete"));
    }

    #[tokio::test]
    async fn leader_cannot_request_own_shutdown() {
        let (router, _, team_store, team_id) = make_fixture("leader-1").await;
        let tool = LifecycleRequestShutdownTool::new(router, team_store, "leader-1".into());

        let err = tool
            .call(LifecycleRequestShutdownArgs {
                team_id,
                reason: "done".into(),
            })
            .await
            .expect_err("leader cannot self-shutdown");
        assert!(err.to_string().contains("leader"));
    }

    #[tokio::test]
    async fn empty_reason_rejected() {
        let (router, _, team_store, team_id) = make_fixture("leader-1").await;
        let tool = LifecycleRequestShutdownTool::new(router, team_store, "worker-1".into());

        let err = tool
            .call(LifecycleRequestShutdownArgs {
                team_id,
                reason: "   ".into(),
            })
            .await
            .expect_err("empty reason must be rejected");
        assert!(err.to_string().contains("reason"));
    }
}
