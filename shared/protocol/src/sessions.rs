//! The `sessions.list` row, as one type both halves use.
//!
//! This is `alephcore`'s `db_handlers::types::SessionInfo` moved verbatim, plus
//! [`SessionListRow::last_run`]. It moved for the reason
//! [`crate::session_thread`]'s header gives: the clients that render this list
//! cannot depend on `alephcore`, so each one hand-wrote the field names it
//! wanted and each one got a different subset. The TUI's session picker read
//! `name` — a key this row has never had — and fell back to the session key, so
//! every conversation in the picker was titled by its key while `topic` and
//! `label` rode the wire unread. The Panel's own `SessionRow` and
//! `shared/client`'s were two further partial copies.
//!
//! A field a client wants is now added **here**, which makes it a compile-time
//! fact on the server too.
//!
//! # Every field has a `#[serde(default)]`
//!
//! A list row is rendered by clients of several vintages against servers of
//! several vintages, and a single missing key must not cost the whole row. The
//! contract is the opposite of [`crate::resume::ResumeReceipt::status`]'s: a
//! receipt has one field that decides what happened and it must fail loudly; a
//! list row has twenty-five independent facts and a row that fails to parse
//! renders as a session that does not exist.

use serde::{Deserialize, Serialize};

use crate::session_thread::LastRunState;

/// One conversation, as `sessions.list` reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionListRow {
    /// Canonical session key (`agent:{id}:main:s{n}`, …).
    #[serde(default)]
    pub key: String,
    /// Agent this conversation is bound to.
    #[serde(default)]
    pub agent_id: String,
    /// `main` / `peer` / `task` / `ephemeral`.
    #[serde(default)]
    pub session_type: String,
    /// Messages recorded on this conversation.
    #[serde(default)]
    pub message_count: u32,
    /// Creation time, RFC 3339.
    #[serde(default)]
    pub created_at: String,
    /// Last activity, RFC 3339. Sorts chronologically as a string because
    /// every row is rendered through the same `to_rfc3339`.
    #[serde(default)]
    pub last_active_at: String,
    /// Display title — the explicit topic, the identity-meta topic, or the
    /// derived one, resolved server-side so every surface shows one answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Session status (e.g. `"closed"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Current lifecycle state.
    ///
    /// **A lifecycle hint, not run state. Do not render it as one.** It records
    /// where the session sits in its own lifecycle (active / archived / …) and
    /// says nothing about whether a run is going, finished or interrupted —
    /// [`Self::last_run`] is the field that answers that. A surface that
    /// painted this word as a run badge would report "active" over a run that
    /// crashed hours ago.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// User-facing label, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Cumulative prompt tokens billed to this conversation.
    #[serde(default)]
    pub input_tokens: u64,
    /// Cumulative completion tokens billed to this conversation.
    #[serde(default)]
    pub output_tokens: u64,
    /// The model that last served a run here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The provider that last served a run here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// Parent session key, for a forked or sub-agent conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_key: Option<String>,
    /// How many times this conversation has been compacted.
    #[serde(default)]
    pub compaction_count: u64,
    /// Originating channel (`"telegram"`, `"gui:chat"`, …), derived from
    /// session identity metadata. `None` for legacy / unknown-origin sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Last-active time as Unix epoch seconds — the field time-grouping and
    /// sort read, beside the RFC 3339 string above.
    #[serde(default)]
    pub updated_at: i64,
    /// User-chosen project working directory
    /// (`identity_meta.custom["project_root"]`). `None` ⇒ the default
    /// `~/.aleph/workspaces/{agent_id}` workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    /// Per-session execution-tier override. `None` ⇒ follows the global
    /// `[policies.exec_tier]` — never render `None` as a concrete tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_tier: Option<String>,
    /// Per-session usage-mode override. `None` ⇒ follows the global mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Per-session reasoning-depth override. `None` ⇒ follows the global depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_level: Option<String>,
    /// Per-session memory mode (`"on"` / `"off"`). `None` ⇒ follows
    /// `[memory] enabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mode: Option<String>,
    /// The model this conversation is **pinned** to by `select_model`.
    ///
    /// Distinct from [`Self::model`], which records what last *served*: a pick
    /// applies from the next run, so for one turn the two disagree and a
    /// surface showing only `model` names the model the user just left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_pin: Option<String>,

    /// What this conversation's newest run did.
    ///
    /// `None` means the server did not answer — an older core, or one with no
    /// event store to ask. It never means "the run was fine": a list face that
    /// rendered absence as clean would hide exactly the sessions this field
    /// exists to surface.
    ///
    /// On this face [`LastRunState::inspected`] is `false`: the list reads run
    /// **markers** only, which is enough for the disposition word and nothing
    /// else. `dangling` and `progress` are the attach face's answer
    /// (`chat.history`), and [`LastRunState::dangling`] returns `None` here
    /// rather than an empty list that reads as "no calls were lost".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<LastRunState>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_thread::LastRunDisposition;

    /// A row from a server that sends only the two keys the oldest client
    /// needed still parses. Every field carries a default precisely so one
    /// missing key does not delete a conversation from the list.
    #[test]
    fn a_row_with_only_a_key_still_parses() {
        let row: SessionListRow = serde_json::from_value(serde_json::json!({
            "key": "agent:main:main"
        }))
        .expect("parse");
        assert_eq!(row.key, "agent:main:main");
        assert_eq!(row.message_count, 0);
        assert_eq!(row.last_run, None);
    }

    /// …and an entirely empty object parses too, which is the census: if any
    /// field ever loses its `#[serde(default)]` this is what goes red.
    #[test]
    fn every_field_has_a_default() {
        let row: SessionListRow = serde_json::from_value(serde_json::json!({})).expect("parse");
        assert_eq!(row, SessionListRow::default());
    }

    /// The two fields the TUI's picker could not read, because it looked for a
    /// key named `name` that this row has never had.
    #[test]
    fn topic_and_label_survive_a_round_trip() {
        let row = SessionListRow {
            key: "agent:main:main:s2".into(),
            topic: Some("crash recovery".into()),
            label: Some("r2".into()),
            ..SessionListRow::default()
        };
        let back: SessionListRow =
            serde_json::from_value(serde_json::to_value(&row).unwrap()).unwrap();
        assert_eq!(back.topic.as_deref(), Some("crash recovery"));
        assert_eq!(back.label.as_deref(), Some("r2"));
        assert_eq!(back, row);
    }

    /// An unset knob stays absent rather than becoming `null` or a value the
    /// row picked: absent is the wire spelling of "follows the global default".
    #[test]
    fn unset_knobs_are_absent_not_null() {
        let v = serde_json::to_value(SessionListRow::default()).expect("serialize");
        for knob in [
            "exec_tier",
            "mode",
            "think_level",
            "memory_mode",
            "last_run",
        ] {
            assert!(v.get(knob).is_none(), "{knob} must be absent: {v:?}");
        }
    }

    /// The list face fills the disposition and nothing else, and says so.
    #[test]
    fn the_list_face_marks_its_answer_uninspected() {
        let row = SessionListRow {
            last_run: Some(LastRunState::from_markers(
                LastRunState::INTERRUPTED,
                Some("run-7".into()),
                2,
            )),
            ..SessionListRow::default()
        };
        let last = row.last_run.as_ref().unwrap();
        assert_eq!(last.disposition(), LastRunDisposition::Interrupted);
        assert_eq!(
            last.dangling(),
            None,
            "a marker-only read must not answer \"no calls were lost\""
        );
    }
}
