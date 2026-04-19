//! Projects a session event stream into message-shaped views for agent_loop.
//!
//! Phase 1 bridge: agent_loop used to read UnifiedMessage arrays from
//! SessionManager; during the migration it reads events from SessionService
//! and projects them into the same shape here.

use crate::session::events::{SessionEvent, SessionEventRecord};

#[derive(Debug, Clone)]
pub struct ProjectedMessage {
    pub role: MessageRole,
    pub text: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// Turn a raw event stream into the message-view agent_loop expects.
pub fn project_messages(events: &[SessionEventRecord]) -> Vec<ProjectedMessage> {
    events
        .iter()
        .filter_map(|record| match &record.event {
            SessionEvent::UserMessage { content, .. } => Some(ProjectedMessage {
                role: MessageRole::User,
                text: content.text.clone(),
                at_ms: record.created_at_ms,
            }),
            SessionEvent::AssistantMessage { content, .. } => Some(ProjectedMessage {
                role: MessageRole::Assistant,
                text: content.text.clone(),
                at_ms: record.created_at_ms,
            }),
            SessionEvent::SystemMessage { content, .. } => Some(ProjectedMessage {
                role: MessageRole::System,
                text: content.clone(),
                at_ms: record.created_at_ms,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, MessageContent};

    fn rec(seq: u64, ev: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event: ev,
            created_at_ms: now_ms(),
        }
    }

    #[test]
    fn projects_only_message_variants() {
        let tid = uuid::Uuid::new_v4();
        let events = vec![
            rec(
                1,
                SessionEvent::TurnStarted {
                    turn_id: tid,
                    trigger: crate::session::events::TurnTrigger::UserMessage,
                    at: now_ms(),
                },
            ),
            rec(
                2,
                SessionEvent::UserMessage {
                    turn_id: tid,
                    content: MessageContent {
                        text: "hi".into(),
                        blocks: vec![],
                    },
                    at: now_ms(),
                },
            ),
            rec(
                3,
                SessionEvent::AssistantMessage {
                    turn_id: tid,
                    content: MessageContent {
                        text: "hello".into(),
                        blocks: vec![],
                    },
                    at: now_ms(),
                },
            ),
        ];
        let msgs = project_messages(&events);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, MessageRole::User);
        assert_eq!(msgs[1].role, MessageRole::Assistant);
    }
}
