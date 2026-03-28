//! Message router with event logging and escalation detection.
//!
//! Provides a higher-level API over [`MessageStore`] that automatically logs
//! events and checks thread-based escalation rules to notify the team leader.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::types::{
    MessageType, NewMessage, Recipient, RecipientRole, TeamMessage,
};
use super::store::MessageStore;
use crate::error::Result;
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};

// ---------------------------------------------------------------------------
// EscalationRule
// ---------------------------------------------------------------------------

/// Configures when the router should escalate a thread to the team leader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationRule {
    /// Number of messages in a thread before suggesting a collaborative session.
    pub thread_message_threshold: u32,
    /// Number of review rejections before escalating (reserved for future use).
    pub review_reject_threshold: u32,
    /// Whether escalation checks are enabled.
    pub enabled: bool,
}

impl Default for EscalationRule {
    fn default() -> Self {
        Self {
            thread_message_threshold: 5,
            review_reject_threshold: 3,
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
    /// Team leader agent ID for escalation notifications.
    leader_id: Option<String>,
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
        };

        let msg = self.msg_store.send_message(new_msg).await?;

        // 3. Log MessageSent event
        let _ = self
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
            .await;

        // 4. Check escalation rules
        if self.escalation_rules.enabled {
            if let Some(ref leader) = self.leader_id {
                if reply_to.is_some() {
                    if let Some(ref thread_id) = msg.thread_id {
                        self.maybe_escalate(
                            &team_id,
                            thread_id,
                            leader,
                            &from_agent,
                        )
                        .await;
                    }
                }
            }
        }

        Ok(msg)
    }

    /// If the thread exceeds the threshold and no escalation notification has
    /// been sent yet, send a SystemNotification to the leader.
    async fn maybe_escalate(
        &self,
        team_id: &str,
        thread_id: &str,
        leader_id: &str,
        from_agent: &str,
    ) {
        let count = match self.msg_store.count_thread_messages(thread_id).await {
            Ok(c) => c,
            Err(_) => return,
        };

        if count < self.escalation_rules.thread_message_threshold {
            return;
        }

        // Check if we already sent a SystemNotification in this thread
        let thread_msgs = match self.msg_store.read_thread(thread_id).await {
            Ok(msgs) => msgs,
            Err(_) => return,
        };

        let already_notified = thread_msgs.iter().any(|m| {
            m.msg_type == MessageType::SystemNotification
                && m.from_agent == "system"
        });

        if already_notified {
            return;
        }

        // Send escalation notification to leader
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
        };

        let _ = self.msg_store.send_message(notification).await;
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
        let events = event_store
            .get_events("team-1", None, None)
            .await
            .unwrap();
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
            msg_type: MessageType::Discovery,
            subject: "Finding".to_string(),
            content: "Discovered something".to_string(),
            reply_to: None,
            attachments: vec![],
        };

        let msg = router.send(req).await.unwrap();
        assert_eq!(msg.recipients.len(), 2);

        let to_recip = msg.recipients.iter().find(|r| r.agent_id == "agent-b").unwrap();
        assert_eq!(to_recip.role, RecipientRole::To);

        let cc_recip = msg.recipients.iter().find(|r| r.agent_id == "agent-c").unwrap();
        assert_eq!(cc_recip.role, RecipientRole::Cc);
    }

    #[tokio::test]
    async fn test_escalation_sends_notification_to_leader() {
        let (msg_store, event_store) = make_stores().await;
        let rules = EscalationRule {
            thread_message_threshold: 3,
            review_reject_threshold: 3,
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

    #[tokio::test]
    async fn test_no_escalation_when_disabled() {
        let (msg_store, event_store) = make_stores().await;
        let rules = EscalationRule {
            thread_message_threshold: 2,
            review_reject_threshold: 3,
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
}
