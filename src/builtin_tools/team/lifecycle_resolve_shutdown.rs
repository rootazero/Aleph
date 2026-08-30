//! `LifecycleResolveShutdownTool` — leader approves or rejects a worker's
//! shutdown request. Mirrors the `plan_resolve` two-verb-in-one pattern:
//! `decision: "approve" | "reject"`.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LifecycleResolveShutdownArgs {
    /// Team this shutdown belongs to
    pub team_id: String,
    /// The `message_id` returned by `lifecycle_request_shutdown`
    pub shutdown_request_id: String,
    /// Agent ID of the worker who requested shutdown
    pub requester_agent_id: String,
    /// "approve" or "reject"
    pub decision: String,
    /// Optional feedback (acknowledgement note, or rejection reason)
    #[serde(default)]
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleResolveShutdownOutput {
    /// "approved" or "rejected"
    pub resolution: String,
    /// ID of the resolution message sent back to the worker
    pub message_id: String,
}

#[derive(Clone)]
pub struct LifecycleResolveShutdownTool {
    router: Arc<MessageRouter>,
    current_agent_id: String,
    team_store: Arc<dyn crate::teams::TeamStore>,
}

impl LifecycleResolveShutdownTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    #[must_use]
    pub fn new(
        router: Arc<MessageRouter>,
        current_agent_id: String,
        team_store: Arc<dyn crate::teams::TeamStore>,
    ) -> Self {
        Self {
            router,
            current_agent_id,
            team_store,
        }
    }
}

#[async_trait]
impl AlephTool for LifecycleResolveShutdownTool {
    const NAME: &'static str = "lifecycle_resolve_shutdown";
    const DESCRIPTION: &'static str =
        "Approve or reject a shutdown request submitted via lifecycle_request_shutdown. \
         Set `decision` to 'approve' or 'reject'. Sends a shutdown-approved / \
         shutdown-rejected message back to the requester; the worker is expected to \
         exit only after seeing the approval.";

    type Args = LifecycleResolveShutdownArgs;
    type Output = LifecycleResolveShutdownOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let actor = self.actor();
        // BT-D-R4-23: gate before any store mutation — any agent that knew
        // a team's id could approve / reject any worker's shutdown request.
        super::require_team_auth(&*self.team_store, &args.team_id, &actor).await?;
        let leader_id = if actor.is_empty() {
            "leader".to_string()
        } else {
            actor
        };

        let (msg_type, label) = match args.decision.trim().to_lowercase().as_str() {
            "approve" | "approved" => (MessageType::ShutdownApproved, "approved"),
            "reject" | "rejected" => (MessageType::ShutdownRejected, "rejected"),
            other => {
                return Err(AlephError::invalid_input(format!(
                    "Invalid decision '{other}': expected 'approve' or 'reject'"
                )));
            }
        };

        let feedback = args
            .feedback
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(match label {
                "approved" => "Shutdown approved.",
                _ => "Shutdown rejected.",
            });

        let subject = match label {
            "approved" => format!("Shutdown approved: {}", args.requester_agent_id),
            _ => format!("Shutdown rejected: {}", args.requester_agent_id),
        };

        let msg = self
            .router
            .send(SendRequest {
                team_id: args.team_id,
                from_agent: leader_id,
                to: vec![args.requester_agent_id.clone()],
                cc: vec![],
                msg_type,
                subject,
                content: feedback.to_string(),
                reply_to: Some(args.shutdown_request_id),
                attachments: vec![],
            })
            .await
            .map_err(|e| AlephError::other(format!("Failed to send shutdown resolution: {e}")))?;

        Ok(LifecycleResolveShutdownOutput {
            resolution: label.to_string(),
            message_id: msg.id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::{MessageStore, SqliteMessageStore};
    use crate::teams::store::TeamStore;

    async fn make_router() -> (Arc<MessageRouter>, Arc<SqliteMessageStore>) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let router = Arc::new(MessageRouter::new(
            msg_store.clone(),
            event_store,
            EscalationRule::default(),
            Some("leader-1".into()),
        ));
        (router, msg_store)
    }

    /// In-memory team store with one team seeded and "leader-1" as the
    /// leader + member. Used by tests of the BT-D-R4-23 auth gate so
    /// `require_team_auth` resolves the team. Returns the store AND the real
    /// team id — `create_team` generates a UUID, so the name ("team-1") is
    /// NOT a valid lookup key.
    async fn make_team_store() -> (Arc<dyn TeamStore>, crate::teams::types::TeamId) {
        use crate::teams::store::SqliteTeamStore;
        use crate::teams::types::{NewTeam, NewTeamMember, TeamId};
        use rusqlite::Connection;
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        let store = SqliteTeamStore::new(conn);
        store.migrate().await.expect("migrate");
        let team = store
            .create_team(NewTeam {
                name: "team-1".into(),
                description: String::new(),
                leader_id: "leader-1".into(),
            })
            .await
            .expect("seed team");
        let team_id: TeamId = team.id;
        store
            .add_member(NewTeamMember {
                team_id: team_id.clone(),
                agent_id: "leader-1".into(),
                role: "lead".into(),
                ..Default::default()
            })
            .await
            .expect("seed leader membership");
        store
            .add_member(NewTeamMember {
                team_id: team_id.clone(),
                agent_id: "worker-1".into(),
                role: "member".into(),
                ..Default::default()
            })
            .await
            .expect("seed worker membership");
        (Arc::new(store) as Arc<dyn TeamStore>, team_id)
    }

    #[tokio::test]
    async fn approve_sends_shutdown_approved() {
        let (router, msg_store) = make_router().await;
        let (team_store, team_id) = make_team_store().await;
        let tool = LifecycleResolveShutdownTool::new(router, "leader-1".into(), team_store);

        let out = tool
            .call(LifecycleResolveShutdownArgs {
                team_id: team_id.clone(),
                shutdown_request_id: "req-1".into(),
                requester_agent_id: "worker-1".into(),
                decision: "approve".into(),
                feedback: Some("Great work; safe to exit".into()),
            })
            .await
            .unwrap();
        assert_eq!(out.resolution, "approved");

        let inbox = msg_store
            .read_inbox("worker-1", &team_id, Some(&MessageType::ShutdownApproved))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].msg_type, MessageType::ShutdownApproved);
        assert!(inbox[0].content.contains("safe to exit"));
        assert_eq!(inbox[0].from_agent, "leader-1");
    }

    #[tokio::test]
    async fn reject_sends_shutdown_rejected_with_feedback_default() {
        let (router, msg_store) = make_router().await;
        let (team_store, team_id) = make_team_store().await;
        let tool = LifecycleResolveShutdownTool::new(router, "leader-1".into(), team_store);

        let out = tool
            .call(LifecycleResolveShutdownArgs {
                team_id: team_id.clone(),
                shutdown_request_id: "req-1".into(),
                requester_agent_id: "worker-1".into(),
                decision: "reject".into(),
                feedback: None,
            })
            .await
            .unwrap();
        assert_eq!(out.resolution, "rejected");

        let inbox = msg_store
            .read_inbox("worker-1", &team_id, Some(&MessageType::ShutdownRejected))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert!(inbox[0].content.contains("rejected"));
    }

    #[tokio::test]
    async fn invalid_decision_errors() {
        let (router, _) = make_router().await;
        let (team_store, team_id) = make_team_store().await;
        let tool = LifecycleResolveShutdownTool::new(router, "leader-1".into(), team_store);

        let err = tool
            .call(LifecycleResolveShutdownArgs {
                team_id,
                shutdown_request_id: "req-1".into(),
                requester_agent_id: "worker-1".into(),
                decision: "maybe".into(),
                feedback: None,
            })
            .await
            .expect_err("invalid decision must error");
        assert!(err.to_string().contains("approve"));
    }

    #[tokio::test]
    async fn reply_to_threads_back_to_request() {
        let (router, msg_store) = make_router().await;
        let (team_store, team_id) = make_team_store().await;
        let tool = LifecycleResolveShutdownTool::new(router.clone(), "leader-1".into(), team_store);

        // Seed: send an initial ShutdownRequest from worker so the resolve
        // message has a real reply_to anchor; verify the resolve carries
        // thread_id linkage.
        let original = router
            .send(SendRequest {
                team_id: team_id.clone(),
                from_agent: "worker-1".into(),
                to: vec!["leader-1".into()],
                cc: vec![],
                msg_type: MessageType::ShutdownRequest,
                subject: "Shutdown".into(),
                content: "Done".into(),
                reply_to: None,
                attachments: vec![],
            })
            .await
            .unwrap();

        tool.call(LifecycleResolveShutdownArgs {
            team_id: team_id.clone(),
            shutdown_request_id: original.id.clone(),
            requester_agent_id: "worker-1".into(),
            decision: "approve".into(),
            feedback: None,
        })
        .await
        .unwrap();

        let inbox = msg_store
            .read_inbox("worker-1", &team_id, Some(&MessageType::ShutdownApproved))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].reply_to.as_deref(), Some(original.id.as_str()));
    }
}
