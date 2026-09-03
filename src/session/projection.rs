//! Projects a session event stream into message-shaped views.
//!
//! Phase 1 bridge: `agent_loop` used to read `UnifiedMessage` arrays from
//! `SessionManager`; during the migration it reads events from `SessionService`
//! and projects them into the same shape here.

use crate::session::events::{ErrorKind, SessionEvent};

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
        // A refusal receipt. `system` is what makes it a centred notice rather
        // than a bubble attributed to somebody — nobody said this, the run did.
        // The text the user already saw live on the `RunError` frame, so this
        // discloses nothing new; it only survives the reload.
        //
        // The label is matched off `kind`, not prefixed blind: a second kind
        // must pick its own wording rather than inherit this one's.
        SessionEvent::Error { kind, message, .. } => {
            let label = match kind {
                ErrorKind::Guardrail => "Input blocked",
            };
            Some(plain("system", format!("{label}: {message}")))
        }
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

/// The order a transcript is READ in, given rows stored in INSERT order.
///
/// # Why insert order stopped being the answer
///
/// A projected row's place in the conversation is its source event's seq; the
/// row's position in the store is the order it was appended. The two agreed
/// for as long as the projector only ever appended ABOVE the newest seq it had
/// written — an agreement by coincidence, not by construction. The seq-set
/// heal (`session_projector`) fills holes BELOW the newest row, so a message
/// recovered after a crash appended last and read last: it surfaced at the
/// bottom of the transcript instead of where it was said.
///
/// So order by the seq, in the one index space that owns the fact.
///
/// # Rows that have no seq
///
/// Not every row is projected. Legacy (pre-SSOT) transcripts have no seqs at
/// all, and boot-time orphan notices are appended directly into a live
/// session. Those rows have no position in seq space — the only position they
/// ever had is where they landed among their neighbours, so each takes the seq
/// of the newest projected row that PRECEDED it. A row with nothing projected
/// before it anchors to `None`, which sorts first and keeps a legacy prefix at
/// the head of a session that was later event-sourced.
///
/// Ties keep insert order: the sort is stable, deliberately, because that is
/// the only ordering two rows sharing an anchor ever had.
///
/// # The SQL spelling of this same rule
///
/// [`TRANSCRIPT_ANCHOR_SQL`] is this function pushed down into SQLite, and it
/// lives in this file for exactly that reason — two spellings of one rule in
/// two files drift, and the last time these two backends answered differently
/// about a row id it cost a whole class of readers their source seq. The
/// cross-backend equivalence is pinned by a test, not by this paragraph.
pub fn order_by_source_seq<T>(rows: &mut Vec<T>, seq_of: impl Fn(&T) -> Option<u64>) {
    let mut newest_before: Option<u64> = None;
    let mut anchored: Vec<(Option<u64>, T)> = std::mem::take(rows)
        .into_iter()
        .map(|row| {
            let anchor = match seq_of(&row) {
                Some(seq) => {
                    newest_before = newest_before.max(Some(seq));
                    Some(seq)
                }
                // `newest_before`, NOT this row's absent seq: an unprojected
                // row inherits the position it was inserted at.
                None => newest_before,
            };
            (anchor, row)
        })
        .collect();
    // Stable: equal anchors stay in insert order, matching the `, id` tail of
    // the SQL below. `None` sorts before `Some`, matching SQLite's NULLS FIRST
    // on an ASC ordering.
    anchored.sort_by(|left, right| left.0.cmp(&right.0));
    rows.extend(anchored.into_iter().map(|(_, row)| row));
}

/// [`order_by_source_seq`]'s anchor, as a SQL expression over `messages`.
///
/// Selected as `anchor` by the transcript reads, which then `ORDER BY anchor,
/// id` (ASC) or `ORDER BY anchor DESC, id DESC` (taking the trailing N). The
/// window frame is STRICTLY PRECEDING on purpose: an unbounded frame that
/// included the current row would return the running maximum, which for a
/// healed row — the whole reason this exists — is some later seq rather than
/// its own.
///
/// No `PARTITION BY`: every reader applies `WHERE session_key = ?` first, so
/// the window already sees exactly one session's rows.
pub const TRANSCRIPT_ANCHOR_SQL: &str = "COALESCE(source_seq, MAX(source_seq) OVER (\
     ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING))";

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

    /// A refusal receipt is user-facing: it becomes a `system` row carrying a
    /// label the reader can act on, plus the reason verbatim. Without the row
    /// the durable receipt never reaches `chat.history`, which is the only
    /// thing a reloading client reads.
    #[test]
    fn project_row_labels_a_refusal_receipt_for_the_reader() {
        let row = project_row(&SessionEvent::Error {
            turn_id: None,
            kind: ErrorKind::Guardrail,
            message: "blocked by pii guardrail".into(),
            recoverable: false,
            at: 0,
        })
        .expect("a refusal receipt must be visible to the user");
        assert_eq!(row.role, "system");
        assert_eq!(row.text, "Input blocked: blocked by pii guardrail");
    }
}

#[cfg(test)]
mod transcript_order_tests {
    use super::order_by_source_seq;

    /// `(seq, label)` in the order the rows were appended.
    fn ordered(rows: &[(Option<u64>, &str)]) -> Vec<String> {
        let mut v: Vec<(Option<u64>, String)> =
            rows.iter().map(|(s, l)| (*s, (*l).to_string())).collect();
        order_by_source_seq(&mut v, |(seq, _)| *seq);
        v.into_iter().map(|(_, l)| l).collect()
    }

    /// The heal shape: a row for seq 3 appended after seqs 4 and 5 belongs
    /// back between them, not at the end where it was written.
    #[test]
    fn a_seq_appended_out_of_order_sorts_back_into_place() {
        assert_eq!(
            ordered(&[
                (Some(1), "a"),
                (Some(2), "b"),
                (Some(4), "d"),
                (Some(5), "e"),
                (Some(3), "c"),
            ]),
            vec!["a", "b", "c", "d", "e"]
        );
    }

    /// An unprojected row anchors to the newest seq BEFORE it — the position
    /// it was inserted at — so it neither drifts to the end nor jumps to the
    /// front. The frame is strictly preceding for this reason: a running
    /// maximum that included the current row would place the healed row of the
    /// previous test at the tail, exactly where the defect had it.
    #[test]
    fn an_unprojected_row_keeps_the_place_it_was_inserted_at() {
        assert_eq!(
            ordered(&[
                (Some(1), "a"),
                (Some(2), "b"),
                (None, "notice"),
                (Some(3), "c"),
            ]),
            vec!["a", "b", "notice", "c"]
        );
    }

    /// Nothing projected yet: the anchor is `None`, which sorts first and so
    /// keeps a legacy prefix at the head of a session that was event-sourced
    /// later. Sorting unprojected rows to the END would reverse this
    /// conversation.
    #[test]
    fn an_unprojected_prefix_sorts_before_everything_seq_bearing() {
        assert_eq!(
            ordered(&[(None, "old-1"), (None, "old-2"), (Some(9), "new")]),
            vec!["old-1", "old-2", "new"]
        );
    }

    /// No seqs at all is a legacy transcript, and the only order it ever had
    /// is the one it is already in. Every anchor ties, so this is the case
    /// that fails the moment the sort stops being stable.
    #[test]
    fn a_transcript_with_no_seqs_is_left_exactly_as_it_came() {
        assert_eq!(
            ordered(&[(None, "c"), (None, "a"), (None, "b")]),
            vec!["c", "a", "b"]
        );
    }

    /// Two rows sharing an anchor — the row that owns the seq, and the
    /// unprojected row that inherited it — stay in insert order. This is the
    /// `, id` tail of the SQL, and the reason the sort is stable rather than
    /// merely correct on distinct keys.
    #[test]
    fn rows_sharing_an_anchor_keep_insert_order() {
        assert_eq!(
            ordered(&[(Some(2), "owner"), (None, "first"), (None, "second")]),
            vec!["owner", "first", "second"]
        );
    }
}
