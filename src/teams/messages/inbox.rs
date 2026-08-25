//! Inbox helper for common message-reading operations.
//!
//! Wraps [`MessageStore`] with convenience methods for reading messages,
//! optionally marking them as read and logging read events.

use crate::sync_primitives::Arc;

use serde::Serialize;

use super::store::MessageStore;
use super::types::{MessageType, TeamMessage};
use crate::error::Result;
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};

// ---------------------------------------------------------------------------
// PeekCount
// ---------------------------------------------------------------------------

/// Unread message counts returned by [`Inbox::peek_count`].
#[derive(Debug, Clone, Serialize)]
pub struct PeekCount {
    pub to: u64,
    pub cc: u64,
}

// ---------------------------------------------------------------------------
// Inbox
// ---------------------------------------------------------------------------

/// Convenience wrapper for common inbox operations with optional event logging.
pub struct Inbox {
    msg_store: Arc<dyn MessageStore>,
    event_store: Arc<dyn EventLogStore>,
}

impl Inbox {
    pub fn new(msg_store: Arc<dyn MessageStore>, event_store: Arc<dyn EventLogStore>) -> Self {
        Self {
            msg_store,
            event_store,
        }
    }

    /// Read inbox messages for an agent, optionally filtered by type.
    ///
    /// When `mark_read` is true, each returned message is marked as read and
    /// a [`TeamEventType::MessageRead`] event is logged.
    pub async fn read(
        &self,
        agent_id: &str,
        team_id: &str,
        msg_type: Option<&MessageType>,
        mark_read: bool,
    ) -> Result<Vec<TeamMessage>> {
        let messages = self
            .msg_store
            .read_inbox(agent_id, team_id, msg_type)
            .await?;

        if mark_read {
            for msg in &messages {
                // mark_read failure is no longer swallowed silently: a
                // dropped mark would let the next `read` return the same
                // message and the agent would double-deliver. Log a
                // structured warn so an operator can correlate an SQLITE_BUSY
                // or disk-full blip with a subsequent duplicate-delivery
                // symptom, then continue so a single message failure does
                // not block the rest of the inbox.
                if let Err(e) = self.msg_store.mark_read(&msg.id, agent_id).await {
                    tracing::warn!(
                        target: "teams.messages.inbox",
                        agent_id = %agent_id,
                        team_id = %team_id,
                        message_id = %msg.id,
                        error = %e,
                        "mark_read failed; message may reappear on next read",
                    );
                }
                // log_event failures were already swallowed, but the
                // audit-hole is narrower than mark_read's user-visible
                // inconsistency. Promote it to a warn so silent
                // event-log gaps are observable too.
                if let Err(e) = self
                    .event_store
                    .log_event(NewTeamEvent {
                        team_id: team_id.to_string(),
                        event_type: TeamEventType::MessageRead,
                        agent_id: agent_id.to_string(),
                        payload: serde_json::json!({
                            "message_id": msg.id,
                        }),
                    })
                    .await
                {
                    tracing::warn!(
                        target: "teams.messages.inbox",
                        agent_id = %agent_id,
                        team_id = %team_id,
                        message_id = %msg.id,
                        error = %e,
                        "MessageRead event log failed; audit gap",
                    );
                }
            }
        }

        Ok(messages)
    }

    /// Read all messages in a thread, ordered by creation time. Scoped by
    /// `team_id` for namespace isolation.
    pub async fn read_thread(&self, team_id: &str, thread_id: &str) -> Result<Vec<TeamMessage>> {
        self.msg_store.read_thread(team_id, thread_id).await
    }

    /// Get unread counts as `(to_count, cc_count)` for an agent in a team.
    pub async fn get_unread_counts(&self, agent_id: &str, team_id: &str) -> Result<(u32, u32)> {
        self.msg_store.get_unread_counts(agent_id, team_id).await
    }

    /// Non-destructive read — returns unread messages without marking as read.
    pub async fn peek(
        &self,
        agent_id: &str,
        team_id: &str,
        msg_type: Option<&MessageType>,
    ) -> Result<Vec<TeamMessage>> {
        self.msg_store.read_inbox(agent_id, team_id, msg_type).await
    }

    /// Returns unread message counts without reading content.
    pub async fn peek_count(&self, agent_id: &str, team_id: &str) -> Result<PeekCount> {
        let (to, cc) = self.msg_store.get_unread_counts(agent_id, team_id).await?;
        Ok(PeekCount {
            to: u64::from(to),
            cc: u64::from(cc),
        })
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
    use crate::teams::messages::types::{NewMessage, Recipient, RecipientRole};

    async fn make_inbox() -> (Inbox, Arc<SqliteMessageStore>, Arc<SqliteEventLogStore>) {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let inbox = Inbox::new(msg_store.clone(), event_store.clone());
        (inbox, msg_store, event_store)
    }

    fn sample_msg(team_id: &str, from: &str, to: &str) -> NewMessage {
        NewMessage {
            team_id: team_id.to_string(),
            from_agent: from.to_string(),
            msg_type: MessageType::Message,
            subject: "Test".to_string(),
            content: "Hello".to_string(),
            recipients: vec![Recipient {
                agent_id: to.to_string(),
                role: RecipientRole::To,
            }],
            reply_to: None,
            attachments: vec![],
        }
    }

    #[tokio::test]
    async fn test_inbox_read_without_mark() {
        let (inbox, msg_store, event_store) = make_inbox().await;

        msg_store
            .send_message(sample_msg("team-1", "agent-a", "agent-b"))
            .await
            .unwrap();

        let msgs = inbox.read("agent-b", "team-1", None, false).await.unwrap();
        assert_eq!(msgs.len(), 1);

        // Message should still be unread
        let msgs2 = inbox.read("agent-b", "team-1", None, false).await.unwrap();
        assert_eq!(msgs2.len(), 1);

        // No events logged
        let events = event_store.get_events("team-1", None, None).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_inbox_read_with_mark() {
        let (inbox, msg_store, event_store) = make_inbox().await;

        msg_store
            .send_message(sample_msg("team-1", "agent-a", "agent-b"))
            .await
            .unwrap();

        let msgs = inbox.read("agent-b", "team-1", None, true).await.unwrap();
        assert_eq!(msgs.len(), 1);

        // Message should now be marked as read (inbox empty)
        let msgs2 = inbox.read("agent-b", "team-1", None, false).await.unwrap();
        assert!(msgs2.is_empty());

        // MessageRead event should be logged
        let events = event_store.get_events("team-1", None, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, TeamEventType::MessageRead);
    }

    #[tokio::test]
    async fn test_inbox_unread_counts() {
        let (inbox, msg_store, _) = make_inbox().await;

        msg_store
            .send_message(sample_msg("team-1", "agent-a", "agent-b"))
            .await
            .unwrap();

        let (to, cc) = inbox.get_unread_counts("agent-b", "team-1").await.unwrap();
        assert_eq!(to, 1);
        assert_eq!(cc, 0);
    }

    #[tokio::test]
    async fn test_peek_does_not_mark_read() {
        let (inbox, msg_store, event_store) = make_inbox().await;

        msg_store
            .send_message(sample_msg("team-1", "agent-a", "agent-b"))
            .await
            .unwrap();

        // First peek returns the message
        let msgs = inbox.peek("agent-b", "team-1", None).await.unwrap();
        assert_eq!(msgs.len(), 1);

        // Second peek still returns it (not marked as read)
        let msgs2 = inbox.peek("agent-b", "team-1", None).await.unwrap();
        assert_eq!(msgs2.len(), 1);

        // No events logged
        let events = event_store.get_events("team-1", None, None).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_peek_count() {
        let (inbox, msg_store, _) = make_inbox().await;

        msg_store
            .send_message(sample_msg("team-1", "agent-a", "agent-b"))
            .await
            .unwrap();

        let counts = inbox.peek_count("agent-b", "team-1").await.unwrap();
        assert_eq!(counts.to, 1);
        assert_eq!(counts.cc, 0);

        // Unknown agent has zero counts
        let counts2 = inbox.peek_count("unknown-agent", "team-1").await.unwrap();
        assert_eq!(counts2.to, 0);
        assert_eq!(counts2.cc, 0);
    }

    #[tokio::test]
    async fn test_inbox_read_thread() {
        let (inbox, msg_store, _) = make_inbox().await;

        let msg1 = msg_store
            .send_message(sample_msg("team-1", "agent-a", "agent-b"))
            .await
            .unwrap();
        let thread_id = msg1.thread_id.unwrap();

        // Reply in same thread
        msg_store
            .send_message(NewMessage {
                reply_to: Some(msg1.id.clone()),
                ..sample_msg("team-1", "agent-b", "agent-a")
            })
            .await
            .unwrap();

        let thread = inbox.read_thread("team-1", &thread_id).await.unwrap();
        assert_eq!(thread.len(), 2);
    }
}
