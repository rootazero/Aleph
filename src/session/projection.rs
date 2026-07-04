//! Projects a session event stream into message-shaped views for `agent_loop`.
//!
//! Phase 1 bridge: `agent_loop` used to read `UnifiedMessage` arrays from
//! `SessionManager`; during the migration it reads events from `SessionService`
//! and projects them into the same shape here.

use crate::session::events::{SessionEvent, SessionEventRecord};

#[derive(Debug, Clone)]
pub struct ProjectedMessage {
    pub role: MessageRole,
    pub text: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// Turn a raw event stream into the message-view `agent_loop` expects.
#[must_use]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedRow {
    pub role: String,
    pub text: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
}

/// Pure map: one session event → at most one projected message row.
/// Internal markers (turn/run/llm/budget/session lifecycle) yield None.
#[must_use]
pub fn project_row(event: &SessionEvent) -> Option<ProjectedRow> {
    let plain = |role: &str, text: String| ProjectedRow {
        role: role.into(),
        text,
        tool_call_id: None,
        tool_name: None,
    };
    match event {
        SessionEvent::UserMessage { content, .. } => Some(plain("user", content.text.clone())),
        SessionEvent::AssistantMessage { content, .. } => {
            Some(plain("assistant", content.text.clone()))
        }
        SessionEvent::SystemMessage { content, .. } => Some(plain("system", content.clone())),
        SessionEvent::ToolCallRequested {
            call_id,
            name,
            input,
            ..
        } => Some(ProjectedRow {
            role: "tool".into(),
            text: input.to_string(),
            tool_call_id: Some(call_id.clone()),
            tool_name: Some(name.clone()),
        }),
        SessionEvent::ToolResult {
            call_id, output, ..
        } => Some(ProjectedRow {
            role: "tool".into(),
            text: output.value.to_string(),
            tool_call_id: Some(call_id.clone()),
            tool_name: None,
        }),
        SessionEvent::ToolError { call_id, error, .. } => Some(ProjectedRow {
            role: "tool".into(),
            text: error.clone(),
            tool_call_id: Some(call_id.clone()),
            tool_name: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, MessageContent, TurnTrigger};

    fn rec(seq: u64, ev: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event: ev,
            created_at_ms: now_ms(),
        }
    }

    #[test]
    fn project_row_maps_message_and_tool_events() {
        let tid = uuid::Uuid::new_v4();
        let user = SessionEvent::UserMessage {
            turn_id: tid,
            content: MessageContent {
                text: "hi".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: 1,
            synthetic: false,
        };
        let r = project_row(&user).unwrap();
        assert_eq!(r.role, "user");
        assert_eq!(r.text, "hi");

        let call = SessionEvent::ToolCallRequested {
            turn_id: tid,
            call_id: "c1".into(),
            name: "bash_exec".into(),
            input: serde_json::json!({"cmd": "ls"}),
            at: 2,
        };
        let r = project_row(&call).unwrap();
        assert_eq!(r.role, "tool");
        assert_eq!(r.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(r.tool_name.as_deref(), Some("bash_exec"));

        // 内部标记不投影
        assert!(project_row(&SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at: 3
        })
        .is_none());
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
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                    synthetic: false,
                },
            ),
            rec(
                3,
                SessionEvent::AssistantMessage {
                    turn_id: tid,
                    content: MessageContent {
                        text: "hello".into(),
                        blocks: vec![],
                        thinking: None,
                        thinking_signature: None,
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

    #[test]
    fn projects_system_message() {
        let tid = uuid::Uuid::new_v4();
        let events = vec![rec(
            1,
            SessionEvent::SystemMessage {
                turn_id: tid,
                content: "sys prompt".into(),
                at: now_ms(),
            },
        )];
        let msgs = project_messages(&events);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, MessageRole::System);
        assert_eq!(msgs[0].text, "sys prompt");
    }

    #[test]
    fn empty_events_yield_empty_messages() {
        let msgs: Vec<ProjectedMessage> = project_messages(&[]);
        assert!(msgs.is_empty());
    }
}
