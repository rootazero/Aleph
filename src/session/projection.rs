//! Projects a session event stream into message-shaped views.
//!
//! Phase 1 bridge: `agent_loop` used to read `UnifiedMessage` arrays from
//! `SessionManager`; during the migration it reads events from `SessionService`
//! and projects them into the same shape here.

use crate::session::events::SessionEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedRow {
    pub role: String,
    pub text: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    /// Who typed this, in a multi-human project room (spec §6.2). Carried here
    /// rather than resolved later because this projection is the ONLY place the
    /// event and the row exist at the same time — `MessageRecord` has no event
    /// to go back to.
    ///
    /// This is the *user-facing* half of the same fact the prompt renders as
    /// `[label]:`. The two are deliberately separate paths (CLAUDE.md §1: the
    /// `messages` table is what the USER sees, `session_events` is what the
    /// MODEL sees) and neither is derived from the other.
    pub author_user_id: Option<String>,
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
        author_user_id: None,
    };
    match event {
        SessionEvent::UserMessage {
            content,
            author_user_id,
            ..
        } => Some(ProjectedRow {
            // rust-doctor-disable-next-line excessive-clone
            author_user_id: author_user_id.clone(),
            ..plain("user", content.text.clone())
        }),
        SessionEvent::AssistantMessage { content, .. } => {
            // rust-doctor-disable-next-line excessive-clone
            Some(plain("assistant", content.text.clone()))
        }
        // rust-doctor-disable-next-line excessive-clone
        SessionEvent::SystemMessage { content, .. } => Some(plain("system", content.clone())),
        SessionEvent::ToolCallRequested {
            call_id,
            name,
            input,
            ..
        } => Some(ProjectedRow {
            role: "tool".into(),
            text: input.to_string(),
            // rust-doctor-disable-next-line excessive-clone
            tool_call_id: Some(call_id.clone()),
            // rust-doctor-disable-next-line excessive-clone
            tool_name: Some(name.clone()),
            author_user_id: None,
        }),
        SessionEvent::ToolResult {
            call_id, output, ..
        } => Some(ProjectedRow {
            role: "tool".into(),
            text: output.value.to_string(),
            // rust-doctor-disable-next-line excessive-clone
            tool_call_id: Some(call_id.clone()),
            tool_name: None,
            author_user_id: None,
        }),
        SessionEvent::ToolError { call_id, error, .. } => Some(ProjectedRow {
            role: "tool".into(),
            // rust-doctor-disable-next-line excessive-clone
            text: error.clone(),
            // rust-doctor-disable-next-line excessive-clone
            tool_call_id: Some(call_id.clone()),
            tool_name: None,
            author_user_id: None,
        }),
        _ => None,
    }
}

/// Row id a projected row carries: `"{session_key}:{seq}"`, where `seq` is the
/// source event's seq. This is the ONLY correlation between a projection row
/// and the event it came from — there is no source-seq column in the file
/// backend's transcript.
#[must_use]
pub fn row_id(session_key: &str, seq: u64) -> String {
    format!("{session_key}:{seq}")
}

/// Inverse of [`row_id`]: recover the source event seq from a projected row id.
///
/// Returns `Some(seq)` only when the prefix equals `session_key` exactly, so
/// rows that were not written by the projector (boot-time orphan notices,
/// legacy / pre-SSOT transcripts) report `None` and are left alone by
/// seq-scoped deletes. `session_key` may itself contain `':'`; the split is on
/// the LAST `':'`, which is the separator [`row_id`] appended.
#[must_use]
pub fn parse_source_seq(id: &str, session_key: &str) -> Option<u64> {
    let (prefix, suffix) = id.rsplit_once(':')?;
    if prefix != session_key {
        return None;
    }
    suffix.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, TurnTrigger};

    #[test]
    fn row_id_round_trips_through_parse_source_seq() {
        let key = "agent:main:reflect";
        let id = row_id(key, 42);
        assert_eq!(id, "agent:main:reflect:42");
        assert_eq!(parse_source_seq(&id, key), Some(42));
    }

    #[test]
    fn parse_source_seq_rejects_foreign_ids() {
        // Boot-time orphan notice / legacy rows carry no projector id.
        assert_eq!(parse_source_seq("orphan-1", "agent:main"), None);
        assert_eq!(parse_source_seq("other:7", "agent:main"), None);
        assert_eq!(parse_source_seq("agent:main:xyz", "agent:main"), None);
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
            author_user_id: None,
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

        // internal markers not projected
        assert!(project_row(&SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at: 3
        })
        .is_none());
    }
}
