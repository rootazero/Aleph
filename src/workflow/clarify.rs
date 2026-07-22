//! Shared types for the workflow `clarify` step — a step that pauses the run,
//! asks the user a structured question through their channel, and resumes the
//! DAG once they answer.
//!
//! This module is the single source of truth shared by the three layers that
//! touch a clarify step:
//! - the **compiler** ([`crate::workflow::compile`]) stamps a [`ClarifyTaskMeta`]
//!   into the materialised `coord_task`'s metadata, owned by [`CLARIFY_OWNER`];
//! - the **dispatcher** ([`crate::teams::dispatcher`]) detects the task via
//!   [`is_clarify_task`], delivers the question, and parks the task in `Paused`;
//! - the **inbound router** ([`crate::gateway::inbound_router`]) resolves the
//!   user's reply by completing the parked task with their answer.
//!
//! The `coord_task` row itself is the durable awaiting record: the question,
//! choices and originating channel all live in its metadata, so a clarify step
//! survives a process restart with no in-memory state to reconstruct (R10 —
//! no new scheduler, no new persistence layer; reuse the task table).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents::swarm::tasks::CoordTask;
use crate::clarification::{ClarificationOption, ClarificationRequest};

/// Sentinel `coord_task.owner` for clarify steps. Never a real team member —
/// the dispatcher branches on [`is_clarify_task`] before owner resolution, so a
/// clarify task is never routed to an agent. Giving it an owner only satisfies
/// the scheduler's "has an owner" selection predicate.
pub const CLARIFY_OWNER: &str = "__clarify__";

/// Metadata key under which [`ClarifyTaskMeta`] is stored on a `coord_task`.
pub const CLARIFY_META_KEY: &str = "clarify";

/// Metadata key stamped (epoch seconds) TOGETHER with the `Paused` park, i.e.
/// BEFORE channel delivery. Its presence on a Paused clarify task means "the
/// dispatcher parked this but delivery has not been confirmed" — the
/// redelivery janitor resets such tasks (past a grace window) back to
/// `Pending` so a crash between park and send cannot stall the DAG forever.
/// Replaced by [`CLARIFY_DELIVERED_AT_KEY`] once the send succeeds. A clarify
/// task manually paused by an operator carries NEITHER key and is never
/// touched by the janitor.
pub const CLARIFY_DELIVERY_PENDING_KEY: &str = "clarify_delivery_pending";

/// Metadata key stamped (epoch seconds) after the question was successfully
/// delivered to the originating channel. The inbound router only consumes a
/// session reply as the clarify answer when this key is present — a question
/// that was never asked must never eat the user's next message.
pub const CLARIFY_DELIVERED_AT_KEY: &str = "clarify_delivered_at";

/// Read the park-time delivery-pending stamp, if any (epoch seconds).
#[must_use]
pub fn clarify_delivery_pending_at(metadata: &Value) -> Option<u64> {
    metadata.get(CLARIFY_DELIVERY_PENDING_KEY)?.as_u64()
}

/// Whether the clarify question was confirmed delivered to the channel.
#[must_use]
pub fn clarify_delivered(metadata: &Value) -> bool {
    metadata.get(CLARIFY_DELIVERED_AT_KEY).is_some()
}

/// The originating channel address captured when a workflow run starts, threaded
/// into each clarify step so the dispatcher knows where to push the question and
/// the inbound router knows which session's reply completes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClarifyContext {
    pub channel_id: String,
    pub conversation_id: String,
    pub session_key: String,
}

/// Durable awaiting record for one clarify step, stored verbatim in the
/// `coord_task` metadata under [`CLARIFY_META_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarifyTaskMeta {
    /// The question shown to the user (`{input}` already substituted).
    pub question: String,
    /// Optional menu; empty → free-text answer, non-empty → pick-one.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Channel the question is delivered to (e.g. `"telegram"`).
    #[serde(default)]
    pub channel_id: String,
    /// Conversation within the channel.
    #[serde(default)]
    pub conversation_id: String,
    /// Session key whose next reply is taken as the answer. Matches the value
    /// the inbound router derives for an incoming message, exactly like the
    /// `ask_user` tool's session-keyed clarification.
    #[serde(default)]
    pub session_key: String,
}

impl ClarifyTaskMeta {
    /// Serialise to the JSON value stored under [`CLARIFY_META_KEY`].
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self)
            .expect("ClarifyTaskMeta is plain types; serialisation cannot fail")
    }

    /// Recover the clarify record from a `coord_task`'s metadata, if present.
    #[must_use]
    pub fn from_metadata(metadata: &Value) -> Option<Self> {
        let raw = metadata.get(CLARIFY_META_KEY)?;
        serde_json::from_value(raw.clone()).ok()
    }

    /// Whether this is a pick-one (menu) clarification.
    #[must_use]
    pub const fn is_select(&self) -> bool {
        !self.choices.is_empty()
    }

    /// Build the [`ClarificationRequest`] used to interpret the user's reply —
    /// mirrors `AskUserTool::build_request` so number/label/free-text matching
    /// behaves identically for workflow clarifications and `ask_user`.
    #[must_use]
    pub fn build_request(&self, request_id: &str) -> ClarificationRequest {
        if self.choices.is_empty() {
            ClarificationRequest::text(request_id, &self.question, None)
        } else {
            let options: Vec<ClarificationOption> = self
                .choices
                .iter()
                .map(|c| ClarificationOption::new(c, c))
                .collect();
            ClarificationRequest::select(request_id, &self.question, options)
        }
    }

    /// Render the channel-facing prompt — identical shape to `ask_user`'s so the
    /// user sees a familiar numbered menu / free-text cue.
    #[must_use]
    pub fn rendered_prompt(&self) -> String {
        if self.choices.is_empty() {
            format!("❓ {}\n\nReply with your answer.", self.question)
        } else {
            let mut menu = String::new();
            for (i, choice) in self.choices.iter().enumerate() {
                menu.push_str(&format!("{}. {}\n", i + 1, choice));
            }
            format!(
                "❓ {}\n\n{menu}\nReply with the number or your answer.",
                self.question
            )
        }
    }
}

/// Whether `task` is a workflow clarify step — detected by the presence of the
/// clarify metadata block, independent of the owner sentinel so a hand-built
/// task is recognised too.
#[must_use]
pub fn is_clarify_task(task: &CoordTask) -> bool {
    task.metadata.get(CLARIFY_META_KEY).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clarification::ClarificationType;

    fn select_meta() -> ClarifyTaskMeta {
        ClarifyTaskMeta {
            question: "Deploy where?".into(),
            choices: vec!["staging".into(), "production".into()],
            channel_id: "telegram".into(),
            conversation_id: "user-1".into(),
            session_key: "telegram:bot:1:user-1".into(),
        }
    }

    fn text_meta() -> ClarifyTaskMeta {
        ClarifyTaskMeta {
            question: "Which file?".into(),
            choices: vec![],
            channel_id: "telegram".into(),
            conversation_id: "user-1".into(),
            session_key: "sess".into(),
        }
    }

    #[test]
    fn value_roundtrips_through_metadata() {
        let meta = select_meta();
        let metadata = serde_json::json!({ CLARIFY_META_KEY: meta.to_value() });
        let back = ClarifyTaskMeta::from_metadata(&metadata).expect("present");
        assert_eq!(back, meta);
    }

    #[test]
    fn from_metadata_absent_returns_none() {
        let metadata = serde_json::json!({ "managed_by": "dispatcher" });
        assert!(ClarifyTaskMeta::from_metadata(&metadata).is_none());
    }

    #[test]
    fn build_request_text_vs_select() {
        assert_eq!(
            text_meta().build_request("id").clarification_type,
            ClarificationType::Text
        );
        let req = select_meta().build_request("id");
        assert_eq!(req.clarification_type, ClarificationType::Select);
        assert_eq!(req.options.as_ref().map(|o| o.len()), Some(2));
    }

    #[test]
    fn rendered_prompt_numbers_choices() {
        let p = select_meta().rendered_prompt();
        assert!(p.contains("1. staging"));
        assert!(p.contains("2. production"));
        assert!(p.contains("number"));
        assert!(text_meta()
            .rendered_prompt()
            .contains("Reply with your answer"));
    }

    #[test]
    fn is_select_reflects_choices() {
        assert!(select_meta().is_select());
        assert!(!text_meta().is_select());
    }
}
