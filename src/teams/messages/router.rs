//! Message router with event logging and escalation detection.
//!
//! Provides a higher-level API over [`MessageStore`] that automatically logs
//! events and checks thread-based escalation rules to notify the team leader.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};

use super::store::MessageStore;
use super::types::{MessageType, NewMessage, Recipient, RecipientRole, TeamMessage};
use crate::error::Result;
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};
use crate::teams::TeamStore;

// ---------------------------------------------------------------------------
// EscalationRule
// ---------------------------------------------------------------------------

/// Configures when the router should escalate a thread to the team leader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationRule {
    /// Number of messages in a thread before suggesting a collaborative session.
    pub thread_message_threshold: u32,
    /// Whether escalation checks are enabled.
    pub enabled: bool,
}

impl Default for EscalationRule {
    fn default() -> Self {
        Self {
            thread_message_threshold: 5,
            enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// SendRequest
// ---------------------------------------------------------------------------

/// A high-level request to send a team message via the router.
#[derive(Debug, Clone)]
pub struct SendRequest {
    pub team_id: String,
    pub from_agent: String,
    /// Agents who must process the message (To recipients).
    pub to: Vec<String>,
    /// Agents to inform (Cc recipients).
    pub cc: Vec<String>,
    pub msg_type: MessageType,
    pub subject: String,
    pub content: String,
    pub reply_to: Option<String>,
    pub attachments: Vec<String>,
}

// ---------------------------------------------------------------------------
// MessageRouter
// ---------------------------------------------------------------------------

/// Routes messages through the store, logs events, and checks escalation rules.
pub struct MessageRouter {
    msg_store: Arc<dyn MessageStore>,
    event_store: Arc<dyn EventLogStore>,
    escalation_rules: EscalationRule,
    /// Construction-time leader override for escalation notifications. The
    /// production router is a GLOBAL singleton while leaders are per-team, so
    /// production passes `None` and resolves the leader per message via
    /// [`Self::with_team_store`]; a fixed id is only meaningful for
    /// single-team callers (tests).
    leader_id: Option<String>,
    /// Per-message leader resolution: `get_team(team_id).leader_id`. Without
    /// this (and with `leader_id: None`) escalation never fires — which is
    /// exactly what shipped for a while: the whole thread-escalation feature
    /// was dead because the one production construction had neither.
    team_store: Option<Arc<dyn TeamStore>>,
}

impl MessageRouter {
    pub fn new(
        msg_store: Arc<dyn MessageStore>,
        event_store: Arc<dyn EventLogStore>,
        escalation_rules: EscalationRule,
        leader_id: Option<String>,
    ) -> Self {
        Self {
            msg_store,
            event_store,
            escalation_rules,
            leader_id,
            team_store: None,
        }
    }

    /// Attach a team store so escalation can resolve each message's team
    /// leader at send time (the router is team-agnostic; leaders are not).
    #[must_use]
    pub fn with_team_store(mut self, team_store: Arc<dyn TeamStore>) -> Self {
        self.team_store = Some(team_store);
        self
    }

    /// The escalation recipient for `team_id`: the construction-time override
    /// if set, else the live team's leader. `None` (no override, no store, or
    /// unknown team) disables escalation for this message.
    async fn resolve_leader(&self, team_id: &str) -> Option<String> {
        if let Some(leader) = &self.leader_id {
            return Some(leader.clone());
        }
        let store = self.team_store.as_ref()?;
        match store.get_team(team_id).await {
            Ok(team) => team.map(|t| t.leader_id),
            Err(e) => {
                tracing::debug!(team_id = %team_id, error = %e,
                    "team router: leader lookup failed; skipping escalation check");
                None
            }
        }
    }

    /// Send a message, log the event, and check escalation rules.
    pub async fn send(&self, req: SendRequest) -> Result<TeamMessage> {
        // 1. Build recipients from to + cc lists
        let mut recipients = Vec::with_capacity(req.to.len() + req.cc.len());
        for agent_id in &req.to {
            recipients.push(Recipient {
                agent_id: agent_id.clone(),
                role: RecipientRole::To,
            });
        }
        for agent_id in &req.cc {
            recipients.push(Recipient {
                agent_id: agent_id.clone(),
                role: RecipientRole::Cc,
            });
        }

        let team_id = req.team_id.clone();
        let from_agent = req.from_agent.clone();
        let reply_to = req.reply_to.clone();

        // 2. Send via store
        let new_msg = NewMessage {
            team_id: req.team_id,
            from_agent: req.from_agent,
            msg_type: req.msg_type,
            subject: req.subject,
            content: req.content,
            recipients,
            reply_to: req.reply_to,
            attachments: req.attachments,
            author_user_id: None,
        };

        let msg = self.msg_store.send_message(new_msg).await?;

        // 3. Log the send into the team event log. (An `EventBus` publish
        // branch used to sit here behind a `with_bus` builder nobody ever
        // called — the direct log below was always the live path; the dead
        // half and its `TeamMessageSent` global-event chain were removed.)
        if let Err(e) = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.clone(),
                event_type: TeamEventType::MessageSent,
                agent_id: from_agent.clone(),
                payload: serde_json::json!({
                    "message_id": msg.id,
                    "subject": msg.subject,
                }),
            })
            .await
        {
            tracing::warn!(team_id = %team_id, error = %e,
                "team router: failed to log message-sent event");
        }

        // 4. Check escalation rules (leader resolved per message — the router
        // is a global singleton, leaders are per-team).
        if self.escalation_rules.enabled && reply_to.is_some() {
            if let Some(ref thread_id) = msg.thread_id {
                if let Some(leader) = self.resolve_leader(&team_id).await {
                    self.maybe_escalate(&team_id, thread_id, &leader, &from_agent)
                        .await;
                }
            }
        }

        Ok(msg)
    }

    /// If the thread exceeds the threshold and no escalation notification has
    /// been sent yet, send a `SystemNotification` to the leader.
    async fn maybe_escalate(
        &self,
        team_id: &str,
        thread_id: &str,
        leader_id: &str,
        from_agent: &str,
    ) {
        let count = match self
            .msg_store
            .count_thread_messages(team_id, thread_id)
            .await
        {
            Ok(c) => c,
            Err(_) => return,
        };

        if count < self.escalation_rules.thread_message_threshold {
            return;
        }

        let ttl = chrono::Duration::minutes(15);
        let notification = NewMessage {
            team_id: team_id.to_string(),
            from_agent: "system".to_string(),
            msg_type: MessageType::SystemNotification,
            subject: "Thread escalation suggestion".to_string(),
            content: format!(
                "Thread {thread_id} has reached {count} messages. \
                 Consider starting a collaborative session to resolve the discussion. \
                 Last message from: {from_agent}."
            ),
            recipients: vec![Recipient {
                agent_id: leader_id.to_string(),
                role: RecipientRole::To,
            }],
            reply_to: Some(thread_id.to_string()),
            attachments: vec![],
            author_user_id: None,
        };

        match self
            .msg_store
            .try_send_first_system_notification(notification, ttl, team_id, thread_id)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                tracing::debug!(
                    thread_id = %thread_id,
                    leader_id = %leader_id,
                    "team router: thread-escalation already notified; skipping duplicate send"
                );
            }
            Err(e) => {
                tracing::warn!(
                    thread_id = %thread_id,
                    leader_id = %leader_id,
                    error = %e,
                    "team router: failed to deliver thread-escalation suggestion"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::store::SqliteMessageStore;

    async fn make_stores() -> (Arc<SqliteMessageStore>, Arc<SqliteEventLogStore>) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        (msg_store, event_store)
    }

    fn simple_request(team_id: &str, from: &str, to: Vec<&str>) -> SendRequest {
        SendRequest {
            team_id: team_id.to_string(),
            from_agent: from.to_string(),
            to: to.into_iter().map(String::from).collect(),
            cc: vec![],
            msg_type: MessageType::Message,
            subject: "Test".to_string(),
            content: "Test content".to_string(),
            reply_to: None,
            attachments: vec![],
        }
    }

    #[tokio::test]
    async fn test_router_sends_and_logs_event() {
        let (msg_store, event_store) = make_stores().await;
        let router = MessageRouter::new(
            msg_store.clone(),
            event_store.clone(),
            EscalationRule::default(),
            None,
        );

        let msg = router
            .send(simple_request("team-1", "agent-a", vec!["agent-b"]))
            .await
            .unwrap();

        // Verify message was delivered
        assert_eq!(msg.team_id, "team-1");
        assert_eq!(msg.from_agent, "agent-a");
        assert_eq!(msg.recipients.len(), 1);
        assert_eq!(msg.recipients[0].agent_id, "agent-b");
        assert_eq!(msg.recipients[0].role, RecipientRole::To);

        // Verify event was logged
        let events = event_store.get_events("team-1", None, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, TeamEventType::MessageSent);
        assert_eq!(events[0].agent_id, "agent-a");
    }

    #[tokio::test]
    async fn test_router_merges_to_and_cc() {
        let (msg_store, event_store) = make_stores().await;
        let router = MessageRouter::new(
            msg_store.clone(),
            event_store.clone(),
            EscalationRule::default(),
            None,
        );

        let req = SendRequest {
            team_id: "team-1".to_string(),
            from_agent: "agent-a".to_string(),
            to: vec!["agent-b".to_string()],
            cc: vec!["agent-c".to_string()],
            msg_type: MessageType::Custom("discovery".into()),
            subject: "Finding".to_string(),
            content: "Discovered something".to_string(),
            reply_to: None,
            attachments: vec![],
        };

        let msg = router.send(req).await.unwrap();
        assert_eq!(msg.recipients.len(), 2);

        let to_recip = msg
            .recipients
            .iter()
            .find(|r| r.agent_id == "agent-b")
            .unwrap();
        assert_eq!(to_recip.role, RecipientRole::To);

        let cc_recip = msg
            .recipients
            .iter()
            .find(|r| r.agent_id == "agent-c")
            .unwrap();
        assert_eq!(cc_recip.role, RecipientRole::Cc);
    }

    #[tokio::test]
    async fn test_escalation_sends_notification_to_leader() {
        let (msg_store, event_store) = make_stores().await;
        let rules = EscalationRule {
            thread_message_threshold: 3,
            enabled: true,
        };
        let router = MessageRouter::new(
            msg_store.clone(),
            event_store.clone(),
            rules,
            Some("leader-1".to_string()),
        );

        // Send first message (starts thread)
        let msg1 = router
            .send(simple_request("team-1", "agent-a", vec!["agent-b"]))
            .await
            .unwrap();
        let thread_id = msg1.thread_id.clone().unwrap();

        // Send replies to build up the thread
        let req2 = SendRequest {
            reply_to: Some(msg1.id.clone()),
            ..simple_request("team-1", "agent-b", vec!["agent-a"])
        };
        router.send(req2).await.unwrap();

        // Third message should trigger escalation (threshold = 3)
        let req3 = SendRequest {
            reply_to: Some(msg1.id.clone()),
            ..simple_request("team-1", "agent-a", vec!["agent-b"])
        };
        router.send(req3).await.unwrap();

        // Check that a SystemNotification was sent to leader-1
        let inbox = msg_store
            .read_inbox("leader-1", "team-1", Some(&MessageType::SystemNotification))
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].msg_type, MessageType::SystemNotification);
        assert!(inbox[0].content.contains(&thread_id));

        // Send a 4th message — should NOT trigger another notification
        let req4 = SendRequest {
            reply_to: Some(msg1.id.clone()),
            ..simple_request("team-1", "agent-b", vec!["agent-a"])
        };
        router.send(req4).await.unwrap();

        let inbox2 = msg_store
            .read_inbox("leader-1", "team-1", Some(&MessageType::SystemNotification))
            .await
            .unwrap();
        // Still just 1 notification (the first one, if unread)
        assert_eq!(inbox2.len(), 1);
    }

    /// The production wiring: no construction-time leader (the router is a
    /// global singleton), leader resolved per message from the team store.
    #[tokio::test]
    async fn escalation_resolves_leader_from_team_store() {
        let (msg_store, event_store) = make_stores().await;
        let teams_conn = rusqlite::Connection::open_in_memory().expect("open teams");
        let team_store = Arc::new(crate::teams::SqliteTeamStore::new(teams_conn));
        team_store.migrate().await.expect("teams migrate");
        let team = team_store
            .create_team(crate::teams::types::NewTeam {
                name: "Alpha".into(),
                description: "".into(),
                leader_id: "store-leader".into(),
            })
            .await
            .unwrap();

        let rules = EscalationRule {
            thread_message_threshold: 2,
            enabled: true,
        };
        let router = MessageRouter::new(msg_store.clone(), event_store.clone(), rules, None)
            .with_team_store(team_store.clone() as Arc<dyn TeamStore>);

        let msg1 = router
            .send(simple_request(&team.id, "agent-a", vec!["agent-b"]))
            .await
            .unwrap();
        let req2 = SendRequest {
            reply_to: Some(msg1.id.clone()),
            ..simple_request(&team.id, "agent-b", vec!["agent-a"])
        };
        router.send(req2).await.unwrap();

        let inbox = msg_store
            .read_inbox(
                "store-leader",
                &team.id,
                Some(&MessageType::SystemNotification),
            )
            .await
            .unwrap();
        assert_eq!(
            inbox.len(),
            1,
            "escalation must reach the store-resolved leader"
        );
    }

    #[tokio::test]
    async fn test_no_escalation_when_disabled() {
        let (msg_store, event_store) = make_stores().await;
        let rules = EscalationRule {
            thread_message_threshold: 2,
            enabled: false,
        };
        let router = MessageRouter::new(
            msg_store.clone(),
            event_store.clone(),
            rules,
            Some("leader-1".to_string()),
        );

        // First message
        let msg1 = router
            .send(simple_request("team-1", "agent-a", vec!["agent-b"]))
            .await
            .unwrap();

        // Reply (would be threshold=2 if enabled)
        let req2 = SendRequest {
            reply_to: Some(msg1.id.clone()),
            ..simple_request("team-1", "agent-b", vec!["agent-a"])
        };
        router.send(req2).await.unwrap();

        // No notification should exist
        let inbox = msg_store
            .read_inbox("leader-1", "team-1", Some(&MessageType::SystemNotification))
            .await
            .unwrap();
        assert!(inbox.is_empty());
    }

    #[tokio::test]
    async fn try_send_first_system_notification_dedups_within_thread() {
        use super::super::store::MessageStore;
        let (msg_store, _event_store) = make_stores().await;

        let notification = NewMessage {
            team_id: "team-dedup".into(),
            from_agent: "system".into(),
            msg_type: MessageType::SystemNotification,
            subject: "Escalation".into(),
            content: "first".into(),
            recipients: vec![Recipient {
                agent_id: "leader-1".into(),
                role: RecipientRole::To,
            }],
            reply_to: Some("thread-x".into()),
            attachments: vec![],
            author_user_id: None,
        };

        let first = msg_store
            .try_send_first_system_notification(
                notification.clone(),
                chrono::Duration::minutes(15),
                "team-dedup",
                "thread-x",
            )
            .await
            .unwrap();
        assert!(first.is_some(), "first call inserts the notification");

        let second = msg_store
            .try_send_first_system_notification(
                notification,
                chrono::Duration::minutes(15),
                "team-dedup",
                "thread-x",
            )
            .await
            .unwrap();
        assert!(
            second.is_none(),
            "second call within the same thread must NOT insert a duplicate"
        );

        let inbox = msg_store
            .read_inbox(
                "leader-1",
                "team-dedup",
                Some(&MessageType::SystemNotification),
            )
            .await
            .unwrap();
        assert_eq!(
            inbox.len(),
            1,
            "exactly one SystemNotification despite two send attempts"
        );
    }

    #[tokio::test]
    async fn try_send_first_system_notification_isolates_threads() {
        use super::super::store::MessageStore;
        let (msg_store, _event_store) = make_stores().await;

        let build = |content: &str| NewMessage {
            team_id: "team-iso".into(),
            from_agent: "system".into(),
            msg_type: MessageType::SystemNotification,
            subject: "Escalation".into(),
            content: content.into(),
            recipients: vec![Recipient {
                agent_id: "leader-1".into(),
                role: RecipientRole::To,
            }],
            reply_to: Some(content.into()),
            attachments: vec![],
            author_user_id: None,
        };

        let a = msg_store
            .try_send_first_system_notification(
                build("thread-A"),
                chrono::Duration::minutes(15),
                "team-iso",
                "thread-A",
            )
            .await
            .unwrap();
        let b = msg_store
            .try_send_first_system_notification(
                build("thread-B"),
                chrono::Duration::minutes(15),
                "team-iso",
                "thread-B",
            )
            .await
            .unwrap();
        assert!(a.is_some() && b.is_some(), "different threads both insert");

        let inbox = msg_store
            .read_inbox(
                "leader-1",
                "team-iso",
                Some(&MessageType::SystemNotification),
            )
            .await
            .unwrap();
        assert_eq!(
            inbox.len(),
            2,
            "one SystemNotification per thread, not per team"
        );
    }
}
