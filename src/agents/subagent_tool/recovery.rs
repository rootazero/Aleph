//! Durable recovery for background sub-agents the in-memory tracker forgot.
//!
//! [`crate::agents::background_tracker::BackgroundAgentTracker`] is process
//! memory by design — two `RwLock<HashMap>`s and a `Notify`. That is the right
//! shape for a live registry, but it means a daemon restart erases every
//! background sub-agent this run ever spawned, including the ones that had
//! already **finished**. The model, whose only handle is the `request_id` the
//! spawn returned, then gets `"No background sub-agent found"` for work that
//! completed successfully and whose full output is sitting in the database.
//!
//! It is not sitting there by accident. `subagent_spawner::spawn` emits a
//! `SubagentSpawned { child_id }` into the **parent** session's durable event
//! log before the child's first turn, and `SubagentReturned { child_id, summary }`
//! after its last — and `subagent_spawner::ephemeral_for` mints that `child_id`
//! from the caller's `request_id`. So the id the model is holding addresses the
//! durable pair directly, and this module is the read side that says so.
//!
//! Two deliberate shapes:
//!
//! * **Structural, not positional.** One turn can spawn several background
//!   children, so their `SubagentSpawned` events share a `turn_id` and cannot be
//!   told apart by order — the same parallel-batch ambiguity that broke the
//!   session-log scan `tools::scoped::dispatch` replaced with an ambient call
//!   identity. The `request_id` is carried *inside* `child_id`, so matching is
//!   exact.
//! * **Lazy, not boot-scanned.** Nothing here runs unless the tracker has
//!   already reported an id it has never seen. A sensor must not create what it
//!   measures, and a boot pass over every session's log would cost the whole
//!   database to serve a case that mostly does not happen. One `get_events` per
//!   tool call serves every unknown id in that call.
//!
//! What this module does **not** do: restart anything. Per R7 the decision to
//! re-run an interrupted child is the model's — this reports what is known and
//! points at the child's own session so the model can read its partial
//! transcript before deciding.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::agents::subagent_spawner::{parent_session_id_of, SUBAGENT_BG_CHILD_PREFIX};
use crate::routing::session_key::SessionKey;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::tools::runtime::ToolResult;

use super::types::LIST_RESULT_PREVIEW_CHARS;

/// What the parent's durable log still knows about a background child that the
/// in-memory tracker has forgotten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Recovered {
    /// The child ran to completion; `summary` is its actual final text, read
    /// back from `SubagentReturned`. This is the case worth the whole module:
    /// the work is done and the answer survived.
    Completed {
        child_session: String,
        summary: String,
    },
    /// The child was spawned and never returned — the process died while it was
    /// running. Its partial transcript lives in `child_session`.
    Interrupted { child_session: String, flow: String },
}

/// True when `child_id` is the child session minted for `request_id`.
///
/// The shape (`Ephemeral`, `sub-bg-<request_id>`) is `ephemeral_for`'s contract;
/// `SUBAGENT_BG_CHILD_PREFIX` is shared with it so there is one answer, not two.
/// Foreground / batch children carry the plain `sub-` prefix and a bare nonce —
/// they address nothing and must never match here.
fn addresses(child_id: &SessionKey, request_id: &str) -> bool {
    match child_id {
        SessionKey::Ephemeral { ephemeral_id, .. } => ephemeral_id
            .strip_prefix(SUBAGENT_BG_CHILD_PREFIX)
            .is_some_and(|rest| rest == request_id),
        _ => false,
    }
}

/// Classify one `request_id` against a parent session's event log.
///
/// `Returned` beats `Spawned` regardless of order in the slice: a completed
/// child is strictly more informative than the fact that it started, and the
/// two events are guaranteed to refer to the same child because both are
/// matched on `child_id`.
pub(crate) fn classify(events: &[SessionEventRecord], request_id: &str) -> Option<Recovered> {
    let mut interrupted: Option<Recovered> = None;
    for record in events {
        match &record.event {
            SessionEvent::SubagentReturned {
                child_id, summary, ..
            } if addresses(child_id, request_id) => {
                // Terminal and unambiguous — stop looking.
                return Some(Recovered::Completed {
                    child_session: child_id.to_string(),
                    summary: summary.clone(),
                });
            }
            SessionEvent::SubagentSpawned { child_id, flow, .. }
                if addresses(child_id, request_id) =>
            {
                interrupted = Some(Recovered::Interrupted {
                    child_session: child_id.to_string(),
                    flow: flow.clone(),
                });
            }
            _ => {}
        }
    }
    interrupted
}

/// The `request_id` a child session key was minted from, if it is one.
fn request_id_of(child_id: &SessionKey) -> Option<&str> {
    match child_id {
        SessionKey::Ephemeral { ephemeral_id, .. } => {
            ephemeral_id.strip_prefix(SUBAGENT_BG_CHILD_PREFIX)
        }
        _ => None,
    }
}

/// Every sub-agent this session's durable log knows about, minus the ones the
/// live tracker already lists.
///
/// Spawn order is preserved and each id appears once — a `SubagentReturned`
/// upgrades the entry its `SubagentSpawned` created rather than adding a second
/// row. (Two rows for one child is the append-vs-overwrite trap: the log is
/// append-only and a child legitimately produces two events.)
pub(crate) fn enumerate(
    events: &[SessionEventRecord],
    known: &[String],
) -> Vec<(String, Recovered)> {
    let mut order: Vec<String> = Vec::new();
    let mut found: HashMap<String, Recovered> = HashMap::new();

    for record in events {
        let (child_id, upgrade) = match &record.event {
            SessionEvent::SubagentSpawned { child_id, flow, .. } => (
                child_id,
                Recovered::Interrupted {
                    child_session: child_id.to_string(),
                    flow: flow.clone(),
                },
            ),
            SessionEvent::SubagentReturned {
                child_id, summary, ..
            } => (
                child_id,
                Recovered::Completed {
                    child_session: child_id.to_string(),
                    summary: summary.clone(),
                },
            ),
            _ => continue,
        };
        let Some(request_id) = request_id_of(child_id) else {
            continue;
        };
        if known.iter().any(|k| k == request_id) {
            continue;
        }
        if found.insert(request_id.to_string(), upgrade).is_none() {
            order.push(request_id.to_string());
        }
    }

    order
        .into_iter()
        .filter_map(|id| found.remove(&id).map(|rec| (id, rec)))
        .collect()
}

/// Resolve the parent session id this tool's children were logged under.
///
/// Reads the raw string with the *emitter's* interpretation
/// ([`parent_session_id_of`]) rather than a second parse of its own — a reader
/// that disagreed with the writer about which session holds the events would
/// find an empty log and report "unknown" forever, with nothing logged.
fn parent_session(raw: Option<&str>) -> Option<SessionKey> {
    parent_session_id_of(raw?)
}

/// Render a recovered child as a tool result.
///
/// Both arms are `Success`, including `Interrupted`. A restart is not a verdict
/// on the call the model is making right now, and the repo already learned that
/// lesson at the dispatch throat: an interruption is reported *in* a success,
/// not as a failure, so it does not trip the harness failure counter and does
/// not read to the model as "this tool is broken".
pub(crate) fn to_result(request_id: &str, recovered: &Recovered) -> ToolResult {
    ToolResult::Success {
        output: to_json(request_id, recovered),
    }
}

/// The JSON body of a recovered child, also used when annotating a multi-id
/// `wait` report.
pub(crate) fn to_json(request_id: &str, recovered: &Recovered) -> Value {
    match recovered {
        Recovered::Completed {
            child_session,
            summary,
        } => json!({
            "status": "completed_recovered",
            "request_id": request_id,
            "final_text": summary,
            "child_session": child_session,
            "note": "This sub-agent finished, but the server restarted before you read its \
                     result, so the live tracker entry is gone. The text above was recovered \
                     from the durable session log and is the sub-agent's actual output — do \
                     NOT re-run the task. Iteration/token counters did not survive; the full \
                     child transcript is at child_session.",
        }),
        Recovered::Interrupted {
            child_session,
            flow,
        } => json!({
            "status": "interrupted",
            "request_id": request_id,
            "agent": flow,
            "child_session": child_session,
            "note": "This sub-agent was still running when the server restarted, so it never \
                     produced a result and is not running now. Whatever it had already done \
                     — including any file writes or commands — has landed. Its partial \
                     transcript is at child_session; read that before deciding whether to \
                     spawn the task again.",
        }),
    }
}

/// Render a recovered child as a **directory row**: the summary is previewed,
/// not carried in full.
///
/// `to_json` is the answer to "tell me about this one sub-agent" and rides the
/// whole output. `list` is the answer to "what is in this session", it can hold
/// dozens of rows, and it already caps its live half at
/// [`LIST_RESULT_PREVIEW_CHARS`] / `MAX_LISTED_COMPLETED` for exactly that
/// reason. A recovered row carrying every byte of every finished sub-agent's
/// output would blow that budget past the point where the directory is usable —
/// and it would grow with session age, because entries reach this path
/// permanently once the tracker's TTL prunes them. `result_chars` states the
/// real size so the preview never reads as the whole thing; `check_status` on
/// the id returns the full text.
pub(crate) fn to_list_row(request_id: &str, recovered: &Recovered) -> Value {
    match recovered {
        Recovered::Completed {
            child_session,
            summary,
        } => {
            let head: String = summary.chars().take(LIST_RESULT_PREVIEW_CHARS).collect();
            let preview = if head.chars().count() < summary.chars().count() {
                format!("{head}…")
            } else {
                head
            };
            json!({
                "status": "completed_recovered",
                "request_id": request_id,
                "result_preview": preview,
                "result_chars": summary.chars().count(),
                "child_session": child_session,
            })
        }
        Recovered::Interrupted {
            child_session,
            flow,
        } => json!({
            "status": "interrupted",
            "request_id": request_id,
            "agent": flow,
            "child_session": child_session,
        }),
    }
}

impl super::SubagentTool {
    /// Ask the parent's durable event log about ids the in-memory tracker has
    /// never seen.
    ///
    /// One `get_events` serves every id in the call, so the cost is per tool
    /// call and not per id — a model that passes twenty bad ids pays one read,
    /// the same as one bad id. Returns empty (never an error) on every
    /// unavailable-substrate path: this is a best-effort enrichment of an
    /// answer the caller can already give.
    pub(super) async fn recover_from_log(&self, ids: &[String]) -> HashMap<String, Recovered> {
        let mut out = HashMap::new();
        if ids.is_empty() {
            return out;
        }
        let Some(parent_id) = parent_session(self.parent_session_id.as_deref()) else {
            // No owning session (CLI / direct construction) — there is no log
            // to consult, and that is not an error.
            return out;
        };
        let events = match self.session.get_events(&parent_id, None, None).await {
            Ok(events) => events,
            Err(error) => {
                tracing::debug!(
                    %error,
                    "subagent recovery: parent event log unreadable; reporting unknown"
                );
                return out;
            }
        };
        for id in ids {
            if let Some(found) = classify(&events, id) {
                out.insert(id.clone(), found);
            }
        }
        out
    }

    /// The durable half of the `list` directory: every sub-agent this session's
    /// log knows about that the live tracker does not.
    ///
    /// `list` documents itself as the place to "recover a request_id you no
    /// longer hold". After a restart the tracker holds none of them, so without
    /// this the directory confidently reports an empty session — the failure
    /// mode where the directory itself is the thing that lies.
    ///
    /// Costs one read per `list` call. `list` is an on-demand directory, not a
    /// hot path, and a directory that is cheap and wrong is not the trade worth
    /// making.
    pub(super) async fn list_from_log(&self, known: &[String]) -> Vec<(String, Recovered)> {
        let Some(parent_id) = parent_session(self.parent_session_id.as_deref()) else {
            return Vec::new();
        };
        match self.session.get_events(&parent_id, None, None).await {
            Ok(events) => enumerate(&events, known),
            Err(error) => {
                tracing::debug!(%error, "subagent list: parent event log unreadable");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, SessionEventRecord};

    fn rec(event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq: 1,
            event,
            created_at_ms: now_ms(),
        }
    }

    fn child(request_id: &str) -> SessionKey {
        SessionKey::Ephemeral {
            agent_id: "researcher".to_string(),
            ephemeral_id: format!("{SUBAGENT_BG_CHILD_PREFIX}{request_id}"),
        }
    }

    fn spawned(request_id: &str) -> SessionEventRecord {
        rec(SessionEvent::SubagentSpawned {
            turn_id: uuid::Uuid::nil(),
            child_id: child(request_id),
            flow: "researcher".to_string(),
            at: now_ms(),
        })
    }

    fn returned(request_id: &str, summary: &str) -> SessionEventRecord {
        rec(SessionEvent::SubagentReturned {
            turn_id: uuid::Uuid::nil(),
            child_id: child(request_id),
            summary: summary.to_string(),
            at: now_ms(),
        })
    }

    #[test]
    fn a_returned_child_recovers_its_actual_summary() {
        let events = vec![spawned("r1"), returned("r1", "the answer is 42")];
        match classify(&events, "r1") {
            Some(Recovered::Completed { summary, .. }) => assert_eq!(summary, "the answer is 42"),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn a_spawned_but_never_returned_child_is_interrupted() {
        let events = vec![spawned("r1")];
        match classify(&events, "r1") {
            Some(Recovered::Interrupted {
                flow,
                child_session,
            }) => {
                assert_eq!(flow, "researcher");
                assert!(child_session.contains("sub-bg-r1"), "got {child_session}");
            }
            other => panic!("expected Interrupted, got {other:?}"),
        }
    }

    #[test]
    fn an_id_with_no_events_recovers_nothing() {
        let events = vec![spawned("r1"), returned("r1", "x")];
        assert_eq!(classify(&events, "typo"), None);
    }

    /// The defect this module exists to prevent: several children spawned in one
    /// turn share a `turn_id`, so anything correlating by position or by turn
    /// would hand back the wrong child's output. Matching is on `child_id`.
    #[test]
    fn concurrent_siblings_in_one_turn_do_not_cross_talk() {
        let events = vec![
            spawned("r1"),
            spawned("r2"),
            spawned("r3"),
            returned("r2", "second child's answer"),
            returned("r1", "first child's answer"),
        ];
        match classify(&events, "r1") {
            Some(Recovered::Completed { summary, .. }) => {
                assert_eq!(summary, "first child's answer");
            }
            other => panic!("expected r1 Completed, got {other:?}"),
        }
        match classify(&events, "r2") {
            Some(Recovered::Completed { summary, .. }) => {
                assert_eq!(summary, "second child's answer");
            }
            other => panic!("expected r2 Completed, got {other:?}"),
        }
        // r3 started and never came back — it must NOT inherit a sibling's text.
        assert!(matches!(
            classify(&events, "r3"),
            Some(Recovered::Interrupted { .. })
        ));
    }

    /// A prefix collision must not resolve: `sub-r1` and `sub-r10` differ, and a
    /// `starts_with` test would conflate them.
    #[test]
    fn request_id_match_is_exact_not_prefix() {
        let events = vec![returned("r10", "ten")];
        assert_eq!(classify(&events, "r1"), None);
    }

    /// Foreground / batch children go through the very same spawner and write
    /// the very same durable events, but their key is `sub-<nonce>` — a nonce
    /// the caller never held. Enumerating them would fill `subagent list` with
    /// every foreground sub-agent the session ever ran, each labelled
    /// recoverable, which is the directory lying in the opposite direction from
    /// the one this module exists to fix.
    #[test]
    fn anonymous_foreground_children_are_not_enumerated() {
        let anon = SessionKey::Ephemeral {
            agent_id: "researcher".to_string(),
            ephemeral_id: "sub-7f3c1e00-0000-0000-0000-000000000000".to_string(),
        };
        let events = vec![
            rec(SessionEvent::SubagentSpawned {
                turn_id: uuid::Uuid::nil(),
                child_id: anon.clone(),
                flow: "researcher".to_string(),
                at: now_ms(),
            }),
            rec(SessionEvent::SubagentReturned {
                turn_id: uuid::Uuid::nil(),
                child_id: anon,
                summary: "inline result already handed to the caller".to_string(),
                at: now_ms(),
            }),
            spawned("bg1"),
        ];
        let listed = enumerate(&events, &[]);
        assert_eq!(
            listed.len(),
            1,
            "only the background child is recoverable, got {listed:?}"
        );
        assert_eq!(listed[0].0, "bg1");
        // And it is not reachable by classify() under its own nonce either.
        assert_eq!(
            classify(&events, "7f3c1e00-0000-0000-0000-000000000000"),
            None
        );
    }

    /// A directory row previews; the single-subject answer does not. Recovered
    /// entries never leave the durable list once the tracker has forgotten them,
    /// so a row carrying the whole output would make `list` grow with session
    /// age until it is unreadable.
    #[test]
    fn list_rows_preview_the_summary_while_to_json_carries_it_whole() {
        let long = "x".repeat(LIST_RESULT_PREVIEW_CHARS * 3);
        let rec = Recovered::Completed {
            child_session: "agent:ephemeral:sub-bg-r1".to_string(),
            summary: long.clone(),
        };

        let row = to_list_row("r1", &rec);
        let preview = row["result_preview"].as_str().unwrap();
        assert_eq!(
            preview.chars().count(),
            LIST_RESULT_PREVIEW_CHARS + 1,
            "preview is the cap plus an ellipsis"
        );
        assert!(preview.ends_with('…'));
        // The real size must be stated, or the preview reads as the whole thing.
        assert_eq!(row["result_chars"].as_u64().unwrap() as usize, long.len());
        assert!(
            row.get("final_text").is_none(),
            "list rows carry no full text"
        );

        // `check_status` on the same id still returns every byte.
        assert_eq!(to_json("r1", &rec)["final_text"].as_str().unwrap(), long);
    }

    /// Non-ephemeral keys never address a sub-agent child, so a same-named DM or
    /// group session cannot be mistaken for one.
    #[test]
    fn only_ephemeral_keys_address_a_child() {
        let key = SessionKey::Ephemeral {
            agent_id: "a".to_string(),
            ephemeral_id: "not-a-subagent".to_string(),
        };
        assert!(!addresses(&key, "not-a-subagent"));
    }

    /// The write side mints `sub-<request_id>`; this is the read side agreeing.
    /// If `ephemeral_for` ever changes shape, background recovery goes silently
    /// blind — this is the test that goes red first.
    #[test]
    fn child_key_roundtrips_through_the_request_id() {
        let minted = child("abc-123");
        assert!(addresses(&minted, "abc-123"));
        assert!(!addresses(&minted, "abc"));
    }
}
