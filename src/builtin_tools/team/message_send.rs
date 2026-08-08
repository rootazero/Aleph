//! `MessageSendTool` — send a message to team members with to/cc routing.

use std::collections::HashSet;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::messages::mentions::{extract_mentions, MENTION_ALL};
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

const fn default_msg_type() -> MessageType {
    MessageType::Message
}

/// Arguments for sending a team message.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MessageSendArgs {
    /// The team this message belongs to
    pub team_id: String,
    /// Agent IDs who must process the message (To recipients)
    pub to: Vec<String>,
    /// Agent IDs to inform (Cc recipients)
    #[serde(default)]
    pub cc: Vec<String>,
    /// Message type for filtering and TTL
    #[serde(default = "default_msg_type")]
    pub msg_type: MessageType,
    /// Short subject line
    pub subject: String,
    /// Full message content (Markdown supported)
    pub content: String,
    /// Message ID to reply to (creates/continues a thread)
    pub reply_to: Option<String>,
    /// Attachment references (optional)
    #[serde(default)]
    pub attachments: Vec<String>,
    /// If true, send to all team members (except sender). The `to` field is ignored.
    #[serde(default)]
    pub broadcast: bool,
}

/// Output from `message_send`.
#[derive(Debug, Clone, Serialize)]
pub struct MessageSendOutput {
    pub message_id: String,
    pub thread_id: Option<String>,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that sends a message to team members via the `MessageRouter`.
#[derive(Clone)]
pub struct MessageSendTool {
    router: Arc<MessageRouter>,
    team_store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl MessageSendTool {
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
impl AlephTool for MessageSendTool {
    const NAME: &'static str = "message_send";
    const DESCRIPTION: &'static str = "Send a message to team members. \
        @mentions in `content` (e.g. `@alice`) are auto-added to recipients \
        when they resolve to real members; `@all` or `@everyone` triggers \
        a broadcast equivalent to `broadcast=true`.";

    type Args = MessageSendArgs;
    type Output = MessageSendOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Extract @mentions from content. `@all` / `@everyone` upgrade the
        // request to a broadcast; plain `@<id>` mentions are union-merged
        // into `to` after validating against actual team membership.
        let mentions = extract_mentions(&args.content);
        let mention_all = mentions.iter().any(|m| m == MENTION_ALL);
        let effective_broadcast = args.broadcast || mention_all;
        let mention_ids: Vec<String> = mentions.into_iter().filter(|m| m != MENTION_ALL).collect();

        let to = if effective_broadcast {
            // Fan out to every member except the sender. Plain @mentions
            // are already covered by this set, so they need no extra work.
            let members = self
                .team_store
                .get_members(&args.team_id)
                .await
                .map_err(|e| {
                    AlephError::other(format!("Failed to resolve broadcast recipients: {e}"))
                })?;
            members
                .into_iter()
                .map(|m| m.agent_id)
                .filter(|id| id != &self.actor())
                .collect::<Vec<_>>()
        } else if mention_ids.is_empty() {
            args.to
        } else {
            // Union explicit `to` with @mentions that resolve to real members.
            // Unknown mention handles are silently dropped — chat content
            // should never error on a typo.
            let members = self
                .team_store
                .get_members(&args.team_id)
                .await
                .map_err(|e| {
                    AlephError::other(format!("Failed to resolve mention recipients: {e}"))
                })?;
            let member_ids: HashSet<String> = members.into_iter().map(|m| m.agent_id).collect();
            let mut to = args.to;
            let already: HashSet<&str> = to.iter().map(|s| s.as_str()).collect();
            let mut additions: Vec<String> = Vec::new();
            for id in mention_ids {
                if id == self.actor() {
                    continue;
                }
                if !member_ids.contains(&id) {
                    continue;
                }
                if already.contains(id.as_str()) || additions.contains(&id) {
                    continue;
                }
                additions.push(id);
            }
            to.extend(additions);
            to
        };

        debug!(
            team_id = %args.team_id,
            to = ?to,
            cc = ?args.cc,
            msg_type = %args.msg_type.as_str(),
            subject = %args.subject,
            from = %self.actor(),
            broadcast = args.broadcast,
            mention_all,
            "message_send: sending message"
        );

        if to.is_empty() && args.cc.is_empty() {
            return Err(AlephError::tool(
                "message_send: at least one recipient (to or cc) is required",
            ));
        }

        let msg = self
            .router
            .send(SendRequest {
                team_id: args.team_id,
                from_agent: self.actor(),
                to,
                cc: args.cc,
                msg_type: args.msg_type,
                subject: args.subject.clone(),
                content: args.content,
                reply_to: args.reply_to,
                attachments: args.attachments,
            })
            .await
            .map_err(|e| AlephError::other(format!("Failed to send message: {e}")))?;

        Ok(MessageSendOutput {
            message_id: msg.id,
            thread_id: msg.thread_id,
            message: format!("Message '{}' sent successfully", args.subject),
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::{MessageStore, SqliteMessageStore};
    use crate::teams::store::SqliteTeamStore;
    use crate::teams::types::{NewTeam, NewTeamMember};
    use rusqlite::Connection;

    /// Build a tool with a fresh in-memory team, three members
    /// (`alice`, `bob`, `carol`) plus a `leader`, and `current_agent_id = leader`.
    async fn setup() -> (MessageSendTool, Arc<SqliteMessageStore>, String) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let router = Arc::new(MessageRouter::new(
            msg_store.clone(),
            event_store,
            EscalationRule {
                thread_message_threshold: 1000,
                enabled: false,
            },
            None,
        ));

        let conn = Connection::open_in_memory().unwrap();
        let team_store = Arc::new(SqliteTeamStore::new(conn));
        team_store.migrate().await.unwrap();

        let team = team_store
            .create_team(NewTeam {
                name: "Alpha".into(),
                description: "test".into(),
                leader_id: "leader".into(),
            })
            .await
            .unwrap();

        for id in ["leader", "alice", "bob", "carol"] {
            team_store
                .add_member(NewTeamMember::for_agent(team.id.clone(), id, "member"))
                .await
                .unwrap();
        }

        let team_store_dyn: Arc<dyn TeamStore> = team_store;
        let tool = MessageSendTool::new(router, team_store_dyn, "leader".into());
        (tool, msg_store, team.id)
    }

    fn args(team_id: &str, to: Vec<&str>, content: &str) -> MessageSendArgs {
        MessageSendArgs {
            team_id: team_id.into(),
            to: to.into_iter().map(String::from).collect(),
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "test".into(),
            content: content.into(),
            reply_to: None,
            attachments: vec![],
            broadcast: false,
        }
    }

    async fn inbox_for(store: &Arc<SqliteMessageStore>, agent: &str, team: &str) -> usize {
        let s: &dyn MessageStore = store.as_ref();
        s.read_inbox(agent, team, None).await.unwrap().len()
    }

    #[tokio::test]
    async fn plain_mention_added_to_recipients() {
        let (tool, msg_store, team_id) = setup().await;
        tool.call(args(&team_id, vec![], "@alice please review"))
            .await
            .unwrap();
        assert_eq!(inbox_for(&msg_store, "alice", &team_id).await, 1);
        assert_eq!(inbox_for(&msg_store, "bob", &team_id).await, 0);
    }

    #[tokio::test]
    async fn mention_all_triggers_broadcast() {
        let (tool, msg_store, team_id) = setup().await;
        tool.call(args(&team_id, vec![], "@everyone heads up"))
            .await
            .unwrap();
        // alice + bob + carol receive; leader (sender) does NOT.
        assert_eq!(inbox_for(&msg_store, "alice", &team_id).await, 1);
        assert_eq!(inbox_for(&msg_store, "bob", &team_id).await, 1);
        assert_eq!(inbox_for(&msg_store, "carol", &team_id).await, 1);
        assert_eq!(inbox_for(&msg_store, "leader", &team_id).await, 0);
    }

    #[tokio::test]
    async fn unknown_mention_silently_dropped() {
        let (tool, msg_store, team_id) = setup().await;
        // `nobody` is not a member; this should NOT error and should NOT
        // add anyone — but the call needs at least one explicit recipient.
        tool.call(args(&team_id, vec!["bob"], "fyi @nobody"))
            .await
            .unwrap();
        assert_eq!(inbox_for(&msg_store, "bob", &team_id).await, 1);
        // No phantom row for the unknown mention.
        assert_eq!(inbox_for(&msg_store, "nobody", &team_id).await, 0);
    }

    #[tokio::test]
    async fn self_mention_excluded() {
        let (tool, msg_store, team_id) = setup().await;
        // The sender is `leader`. `@leader` in own content must not echo back.
        tool.call(args(&team_id, vec!["alice"], "@leader noted"))
            .await
            .unwrap();
        assert_eq!(inbox_for(&msg_store, "alice", &team_id).await, 1);
        assert_eq!(inbox_for(&msg_store, "leader", &team_id).await, 0);
    }

    #[tokio::test]
    async fn mention_union_dedupes_with_explicit_to() {
        let (tool, msg_store, team_id) = setup().await;
        // alice is both explicit and mentioned — must land exactly once.
        tool.call(args(&team_id, vec!["alice"], "@alice please ack"))
            .await
            .unwrap();
        assert_eq!(inbox_for(&msg_store, "alice", &team_id).await, 1);
    }

    #[tokio::test]
    async fn no_recipients_no_mentions_errors() {
        let (tool, _, team_id) = setup().await;
        let err = tool
            .call(args(&team_id, vec![], "no recipients, no mentions"))
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("at least one recipient"));
    }

    #[tokio::test]
    async fn mention_in_code_block_ignored() {
        let (tool, msg_store, team_id) = setup().await;
        let body = "see snippet:\n```\n@alice in code\n```\nFYI";
        tool.call(args(&team_id, vec!["bob"], body)).await.unwrap();
        // bob got the message, alice did NOT (code block suppressed mention).
        assert_eq!(inbox_for(&msg_store, "bob", &team_id).await, 1);
        assert_eq!(inbox_for(&msg_store, "alice", &team_id).await, 0);
    }
}
