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
//!   tool call classifies every unknown id in that call, plus one more per
//!   interrupted id on the detail face (`resolve_forgotten`'s enrichment loop).
//!
//! What this module does **not** do: restart anything. Per R7 the decision to
//! re-run an interrupted child is the model's — this reports what is known and
//! points at the child's own session so the model can read its partial
//! transcript before deciding.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::agents::subagent_spawner::{
    background_child_session_key, parent_session_id_of, SUBAGENT_BG_CHILD_PREFIX,
};
use crate::routing::session_key::SessionKey;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::session::reduction::{DanglingCall, LogContradiction, RunProgress};
use crate::tools::runtime::ToolResult;

use super::types::LIST_RESULT_PREVIEW_CHARS;

/// How much of one recovered transcript line the parent is shown.
const CHILD_TAIL_CHARS: usize = 400;
/// How many trailing assistant/tool lines of the child's log are carried.
const CHILD_TAIL_LINES: usize = 3;

/// One line of a child's transcript tail.
///
/// A pointer at `child_session` is only a pointer: the model has to spend
/// another tool call to learn whether the child had produced anything at all,
/// and the answer is already in the events this module just read. `text` is
/// bounded because this rides a `check_status` result, not a transcript view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TailLine {
    pub role: &'static str,
    pub text: String,
}

/// The last few assistant/tool lines of a child's own scope.
///
/// Reads the durable `session_events` slice the caller already loaded — never
/// the `messages` projection, which is lossy by construction and would make
/// this face disagree with `progress`, computed from the same slice.
fn child_tail(events: &[SessionEventRecord]) -> Vec<TailLine> {
    let mut out: Vec<TailLine> = Vec::new();
    for record in events.iter().rev() {
        let (role, text) = match &record.event {
            SessionEvent::AssistantMessage { content, .. } if !content.text.is_empty() => {
                ("assistant", content.text.clone())
            }
            SessionEvent::ToolResult { output, .. } => (
                "tool",
                output
                    .value
                    .as_str()
                    .map_or_else(|| output.value.to_string(), str::to_string),
            ),
            SessionEvent::ToolError { error, .. } => ("tool_error", error.clone()),
            _ => continue,
        };
        out.push(TailLine {
            role,
            text: text.chars().take(CHILD_TAIL_CHARS).collect(),
        });
        if out.len() == CHILD_TAIL_LINES {
            break;
        }
    }
    out.reverse();
    out
}

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
    /// The child was spawned and never returned — the process died while it
    /// was running. Its partial transcript lives in `child_session`.
    Interrupted {
        /// Kept as the key the emitter minted, not a string parsed back out of
        /// one: `classify` and `enumerate` both hold it already, and a second
        /// parse would be a second answer to "which session is this".
        child_session: SessionKey,
        flow: String,
        /// What the child got done before it stopped, when this face paid to
        /// find out. `None` means **this face did not ask** — never "no
        /// progress". Only the detail face (`resolve_forgotten`) fills it in;
        /// the directory (`list_from_log`) leaves it `None` on purpose.
        progress: Option<RunProgress>,
        /// Calls the child dispatched with no recorded result, filled in the
        /// same pass as `progress` and empty for the same two reasons (the log
        /// was clean, or this face did not ask). These are the calls whose
        /// outcome is *unknown* — the model must not read them as landed.
        in_flight: Vec<DanglingCall>,
        /// The last few lines of the child's own transcript, so the pointer at
        /// `child_session` carries evidence instead of only an address.
        tail: Vec<TailLine>,
        /// What the child's log says that it must not say, read in the same
        /// pass as `progress`. Empty when the log is consistent or when this
        /// face did not ask; holds the REJECT kind alone when the reducer
        /// refused the log (then `progress` is `None` — unknown, not zero).
        contradictions: Vec<LogContradiction>,
    },
    /// Known only to the cross-process sidecar
    /// ([`crate::agents::background_persistence`]).
    ///
    /// The two durable sources do not cover the same set. `SubagentSpawned` is
    /// emitted *after* the spawner takes its concurrency permit, so a child
    /// that was still queued behind `max_concurrent_subagents` when the daemon
    /// died has no event-log record at all — while the sidecar's `record_start`
    /// ran before the task was even spawned. The sidecar also carries the
    /// activity trail and the settled outcome, neither of which the log has.
    ///
    /// It does not *replace* the log, though, which is what it used to do: the
    /// sidecar arm was inserted over whatever `recover_from_log` had found, so
    /// a child that was both recorded here AND spawned into the parent's log
    /// lost its `child_session` pointer, its progress counters and its
    /// in-flight calls — the sidecar knows the phase, only the log knows what
    /// the child actually did. Both halves ride together now.
    Sidecar {
        record: Box<crate::agents::background_persistence::RecoveredRun>,
        /// Address of the child's own transcript, derived from the record the
        /// same way the spawner minted it. `None` only when the record carries
        /// no agent to mint it from.
        child_session: Option<SessionKey>,
        /// Same contract as [`Recovered::Interrupted::progress`]: `None` means
        /// **this face did not ask**, never "no progress". A `Settled` record
        /// is not read at all — its work is over and its result is the answer.
        progress: Option<RunProgress>,
        in_flight: Vec<DanglingCall>,
        tail: Vec<TailLine>,
        contradictions: Vec<LogContradiction>,
    },
}

impl Recovered {
    /// The sidecar arm with nothing read from the child log yet — the shape
    /// every face starts from, and the only shape the directory face keeps.
    fn sidecar(run: crate::agents::background_persistence::RecoveredRun) -> Self {
        let child_session = (!run.record.agent.is_empty())
            .then(|| background_child_session_key(&run.record.agent, &run.record.request_id));
        Self::Sidecar {
            record: Box::new(run),
            child_session,
            progress: None,
            in_flight: Vec::new(),
            tail: Vec::new(),
            contradictions: Vec::new(),
        }
    }

    /// The child log this row still needs read, plus the four slots that read
    /// fills — or `None` when there is nothing to ask.
    ///
    /// One accessor rather than one enrichment loop per arm: the loop used to
    /// name `Interrupted` alone, and every arm added afterwards inherited
    /// "silently not enriched" as its default.
    fn enrichment_slots(
        &mut self,
    ) -> Option<(
        SessionKey,
        &mut Option<RunProgress>,
        &mut Vec<DanglingCall>,
        &mut Vec<TailLine>,
        &mut Vec<LogContradiction>,
    )> {
        match self {
            Self::Interrupted {
                child_session,
                progress,
                in_flight,
                tail,
                contradictions,
                ..
            } => Some((
                child_session.clone(),
                progress,
                in_flight,
                tail,
                contradictions,
            )),
            Self::Sidecar {
                record,
                child_session,
                progress,
                in_flight,
                tail,
                contradictions,
            } => {
                // A settled run is not asked about: its work is over and its
                // recorded result is the answer, so a progress count would only
                // invite the model to second-guess a finished job.
                if record.record.phase == crate::agents::background_persistence::RunPhase::Settled {
                    return None;
                }
                let child = child_session.clone()?;
                Some((child, progress, in_flight, tail, contradictions))
            }
            // Its actual final text is already here; there is nothing a child
            // log could add and a read would be pure cost.
            Self::Completed { .. } => None,
        }
    }
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
                    child_session: child_id.clone(),
                    flow: flow.clone(),
                    progress: None,
                    in_flight: Vec::new(),
                    tail: Vec::new(),
                    contradictions: Vec::new(),
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
                    child_session: child_id.clone(),
                    flow: flow.clone(),
                    progress: None,
                    in_flight: Vec::new(),
                    tail: Vec::new(),
                    contradictions: Vec::new(),
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
            progress,
            in_flight,
            tail,
            contradictions,
        } => json!({
            "status": "interrupted",
            "request_id": request_id,
            "agent": flow,
            "child_session": child_session.to_string(),
            "progress": progress_json(progress.as_ref()),
            "in_flight_calls": in_flight_json(in_flight),
            "child_tail": tail_json(tail),
            // The child log's contradictions, by finding tag, so the model
            // reads a refused or inconsistent log as "unknown" rather than
            // as a clean run with zero progress.
            "log_contradictions": contradictions
                .iter()
                .map(LogContradiction::tag)
                .collect::<Vec<_>>(),
            "note": interrupted_note(
                "This sub-agent was still running when the server restarted, so it never \
                 produced a result and is not running now.",
                progress.as_ref(),
                in_flight,
            ),
        }),
        Recovered::Sidecar {
            record: run,
            child_session,
            progress,
            in_flight,
            tail,
            contradictions,
        } => {
            use crate::agents::background_persistence::{settled_label, RunPhase};
            let note = if run.record.phase == RunPhase::Settled {
                settled_note(&run.record).to_string()
            } else {
                interrupted_note(
                    "This sub-agent belonged to a previous daemon process, so its live state \
                     is gone and it is not running now. Nothing about the task itself was \
                     judged.",
                    progress.as_ref(),
                    in_flight,
                )
            };
            json!({
                "status": settled_label(&run.record),
                "request_id": request_id,
                "task": run.record.task,
                "agent": run.record.agent,
                "started_ms": run.record.started_ms,
                "last_activity_ms": run.last_activity_ms,
                "partial_result": run.partial_result,
                "outcome": run.record.outcome,
                "child_session": child_session.as_ref().map(SessionKey::to_string),
                "progress": progress_json(progress.as_ref()),
                "in_flight_calls": in_flight_json(in_flight),
                "child_tail": tail_json(tail),
                "log_contradictions": contradictions
                    .iter()
                    .map(LogContradiction::tag)
                    .collect::<Vec<_>>(),
                "note": note,
            })
        }
    }
}

/// The progress counters, or `null` for **"this face did not ask"**. One
/// renderer for both arms: two `json!` literals is two answers to "what are
/// these keys called".
fn progress_json(progress: Option<&RunProgress>) -> Value {
    progress.map_or(Value::Null, |p| {
        json!({
            "tool_calls_dispatched": p.tool_calls_dispatched,
            "tool_calls_answered": p.tool_calls_answered,
            "assistant_messages": p.assistant_messages,
            "last_activity_ms": p.last_activity_at,
        })
    })
}

/// The calls that crossed the dispatch line with no recorded result.
///
/// `provenance` is deliberately not rendered: every dangling call in a *child*
/// log is `EarlierRun` (the child has no open run of its own once its process
/// is gone), so the field would be a constant dressed as a finding.
fn in_flight_json(calls: &[DanglingCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                json!({
                    "tool_name": c.tool_name,
                    "call_id": c.call_id,
                    "denied": c.denied,
                })
            })
            .collect(),
    )
}

fn tail_json(tail: &[TailLine]) -> Value {
    Value::Array(
        tail.iter()
            .map(|l| json!({ "role": l.role, "text": l.text }))
            .collect(),
    )
}

/// What a settled sidecar record earns as a note — read off its **outcome**,
/// not off its phase.
///
/// `phase == Settled` says the run reached a terminal state in the process that
/// owned it; it does not say the terminal state was success. Telling the model
/// "the work is done, do NOT re-run it" about a child that timed out, was
/// cancelled or failed is the expensive direction of that collapse: the task
/// silently never gets done and the transcript says it did.
fn settled_note(record: &crate::agents::background_persistence::PersistedRun) -> &'static str {
    match record.outcome.as_deref() {
        Some("completed") => {
            "This sub-agent FINISHED in a previous daemon process. What it recorded before \
             that process ended is above — the work is done, do NOT re-run it."
        }
        _ => {
            "This sub-agent ended without success in a previous daemon process — see \
             `outcome` for how. Whatever it recorded is above; the task itself may still be \
             undone. Read it and decide whether to re-run."
        }
    }
}

/// The shared tail of every "it stopped without a result" note.
///
/// The old sentence promised that "whatever it had already done — including any
/// file writes or commands — has landed", which is false for precisely the
/// calls that matter: a dispatch with no recorded result may have run, may have
/// half-run, may never have reached the tool at all, and a call the approval
/// gate denied definitely did not run. Naming the calls is what lets the model
/// decide; claiming they landed decides for it.
fn interrupted_note(
    opening: &str,
    progress: Option<&RunProgress>,
    in_flight: &[DanglingCall],
) -> String {
    use std::fmt::Write as _;
    let mut note = opening.to_string();
    if let Some(p) = progress {
        let calls_word = if p.tool_calls_dispatched == 1 {
            "call"
        } else {
            "calls"
        };
        let messages_word = if p.assistant_messages == 1 {
            "message"
        } else {
            "messages"
        };
        let _ = write!(
            note,
            " Before it stopped it had dispatched {} tool {}, {} of which recorded a result, \
             and produced {} assistant {}. Calls that recorded a result have landed.",
            p.tool_calls_dispatched, calls_word, p.tool_calls_answered, p.assistant_messages,
            messages_word
        );
    }
    let (denied, unknown): (Vec<&DanglingCall>, Vec<&DanglingCall>) =
        in_flight.iter().partition(|c| c.denied);
    if !unknown.is_empty() {
        let names: Vec<&str> = unknown.iter().map(|c| c.tool_name.as_str()).collect();
        let _ = write!(
            note,
            " These calls were dispatched with no recorded result — their outcome is unknown: \
             [{}].",
            names.join(", ")
        );
    }
    if !denied.is_empty() {
        let names: Vec<&str> = denied.iter().map(|c| c.tool_name.as_str()).collect();
        let _ = write!(
            note,
            " These calls were denied by the approval gate and did not run: [{}].",
            names.join(", ")
        );
    }
    note.push_str(
        " Read child_session before deciding whether to spawn the task again — this is a \
         report of what happened, not a verdict on what is left.",
    );
    note
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
            ..
        } => json!({
            "status": "interrupted",
            "request_id": request_id,
            "agent": flow,
            "child_session": child_session.to_string(),
        }),
        Recovered::Sidecar { record: run, .. } => {
            let text = &run.partial_result;
            let head: String = text.chars().take(LIST_RESULT_PREVIEW_CHARS).collect();
            let preview = if head.chars().count() < text.chars().count() {
                format!("{head}…")
            } else {
                head
            };
            json!({
                "status": crate::agents::background_persistence::settled_label(&run.record),
                "request_id": request_id,
                "task": run.record.task,
                "agent": run.record.agent,
                "result_preview": preview,
                "result_chars": text.chars().count(),
                "last_activity_ms": run.last_activity_ms,
            })
        }
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

    /// **The** answer to "the tracker has never heard of this id — what does
    /// anything durable know?".
    ///
    /// There are two durable sources and they do not cover the same set, so
    /// consulting one of them is consulting half the evidence. Before this
    /// existed, four faces of the same tool each picked one: `check_status` and
    /// single-id `wait` asked only the sidecar; multi-id `wait`, `cancel` and
    /// `list` asked only the event log. The visible symptom was self-
    /// contradiction — `list` rendering an id as `completed_recovered` with a
    /// result preview while `check_status` on that same id answered "No
    /// background sub-agent found" — plus a hard-coded note telling the model
    /// those ids "will never complete; drop them", said about children whose
    /// output the sidecar was holding.
    ///
    /// Precedence, in one place:
    /// 1. the event log's `Completed` — it carries the child's *actual* final
    ///    text, which the sidecar's activity trail only approximates;
    /// 2. the sidecar — phase, outcome and the masked progress trail, and the
    ///    only source that knows about a child that died still queued;
    /// 3. the event log's `Interrupted` — a pointer at the child transcript.
    pub(super) async fn resolve_forgotten(
        &self,
        ids: &[String],
        scope: Option<&str>,
    ) -> HashMap<String, Recovered> {
        let mut out = self.recover_from_log(ids).await;
        for id in ids {
            if matches!(out.get(id), Some(Recovered::Completed { .. })) {
                continue;
            }
            if let Some(run) = crate::agents::background_persistence::lookup(id, scope) {
                out.insert(id.clone(), Recovered::sidecar(run));
            }
        }
        // The detail face pays for progress; the directory does not (see the
        // `progress` field's doc). One extra read per interrupted child, and
        // only for the ids the caller actually named.
        //
        // BOTH unfinished arms, not just `Interrupted`: the sidecar arm
        // overwrites the log arm above, so gating the enrichment on
        // `Interrupted` meant every child the sidecar also knew about — which
        // is every background child spawned by a daemon with persistence on —
        // silently lost its progress, its in-flight calls and its transcript
        // tail. The one that reached the enrichment was the one the sidecar had
        // never heard of.
        for recovered in out.values_mut() {
            let Some((child, progress, in_flight, tail, contradictions)) =
                recovered.enrichment_slots()
            else {
                continue;
            };
            match self.session.get_events(&child, None, None).await {
                Ok(events) => {
                    // A forked child's log opens with a copy of the parent's
                    // transcript; charging those dispatches to the child would
                    // report the parent's calls as the child's in-flight work.
                    let own = &events[crate::session::reduction::own_work_start(&events)..];
                    *tail = child_tail(own);
                    match crate::session::reduction::reduce_run(own) {
                        Ok(reduction) => {
                            *progress = Some(reduction.progress);
                            *in_flight = reduction.dangling;
                            *contradictions = reduction.contradictions;
                        }
                        // Refused: progress stays absent (unknown, not zero)
                        // and the refusal itself is what the model sees.
                        Err(contradiction) => {
                            *progress = None;
                            tracing::warn!(
                                contradiction = %contradiction,
                                "subagent recovery: child log refused by the reducer"
                            );
                            *contradictions = vec![contradiction];
                        }
                    }
                }
                Err(error) => {
                    // Absent, not zero. A store that could not be read has
                    // not told us the child did nothing.
                    tracing::debug!(%error, "subagent recovery: child event log unreadable");
                }
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
    pub(super) async fn list_from_log(
        &self,
        known: &[String],
        scope: Option<&str>,
    ) -> Vec<(String, Recovered)> {
        let mut out = match parent_session(self.parent_session_id.as_deref()) {
            Some(parent_id) => match self.session.get_events(&parent_id, None, None).await {
                Ok(events) => enumerate(&events, known),
                Err(error) => {
                    tracing::debug!(%error, "subagent list: parent event log unreadable");
                    Vec::new()
                }
            },
            // No owning session (CLI / direct construction) — no log to read,
            // which is not an error and must not skip the sidecar half below.
            None => Vec::new(),
        };
        // The sidecar's half of the directory: children the log cannot see
        // because they never got as far as `SubagentSpawned`. Same precedence
        // as `resolve_forgotten` — the log wins where both know an id.
        let mut seen: Vec<String> = known.to_vec();
        seen.extend(out.iter().map(|(id, _)| id.clone()));
        for run in crate::agents::background_persistence::list_for_scope(scope, &seen) {
            out.push((run.record.request_id.clone(), Recovered::sidecar(run)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::background_persistence::{PersistedRun, RecoveredRun, RunPhase};
    use crate::session::events::{now_ms, SessionEventRecord, TurnId};

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
                progress,
                ..
            }) => {
                assert_eq!(flow, "researcher");
                assert!(
                    child_session.to_string().contains("sub-bg-r1"),
                    "got {child_session}"
                );
                assert!(progress.is_none(), "classify never pays for progress");
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

    // -------------------------------------------------------------------
    // G5 — progress evidence fixtures
    // -------------------------------------------------------------------

    /// Like [`child`], but with a caller-chosen `agent_id` — G5's fixtures
    /// need that to vary while `child`'s callers do not.
    fn bg_child(agent: &str, request_id: &str) -> SessionKey {
        SessionKey::Ephemeral {
            agent_id: agent.to_string(),
            ephemeral_id: format!("{SUBAGENT_BG_CHILD_PREFIX}{request_id}"),
        }
    }

    /// Unlike [`rec`] (which always stamps `seq: 1`), takes an explicit
    /// ascending `seq`. `reduce_run`'s anchor/in-scope split compares by
    /// `seq`, not by vector position, so a child-log fixture with more than
    /// one record after a `RunStarted` needs its `ToolCallRequested` to carry
    /// a `seq` strictly greater than the anchor's — otherwise it reads as
    /// belonging to an earlier run and `progress` undercounts it.
    fn seqed(seq: crate::session::events::EventSeq, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: now_ms(),
        }
    }

    /// A `SessionService` test double that counts every `get_events` call and
    /// answers from a fixed parent log plus an optional child log, dispatched
    /// by session id. This is what proves G5: the directory face
    /// (`list_from_log`) must read only the parent, the detail face
    /// (`resolve_forgotten`) reads the parent AND the child it names.
    struct CountingSessionService {
        counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        parent_id: SessionKey,
        parent_log: Vec<SessionEventRecord>,
        child: Option<(SessionKey, Vec<SessionEventRecord>)>,
    }

    #[async_trait::async_trait]
    impl crate::session::service::SessionService for CountingSessionService {
        async fn attach(
            &self,
            id: crate::session::service::SessionId,
        ) -> Result<crate::session::service::SessionHandle, crate::session::service::SessionError>
        {
            Ok(crate::session::service::SessionHandle { id, head_seq: 0 })
        }

        async fn get_events(
            &self,
            id: &crate::session::service::SessionId,
            _from: Option<crate::session::events::EventSeq>,
            _to: Option<crate::session::events::EventSeq>,
        ) -> Result<Vec<SessionEventRecord>, crate::session::service::SessionError> {
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if *id == self.parent_id {
                return Ok(self.parent_log.clone());
            }
            if let Some((child_id, child_log)) = &self.child {
                if id == child_id {
                    return Ok(child_log.clone());
                }
            }
            Ok(Vec::new())
        }

        async fn emit_event(
            &self,
            _id: &crate::session::service::SessionId,
            _event: SessionEvent,
        ) -> Result<crate::session::events::EventSeq, crate::session::service::SessionError>
        {
            unimplemented!("not exercised by recovery::tests")
        }

        async fn subscribe(
            &self,
            _id: &crate::session::service::SessionId,
        ) -> Result<
            tokio::sync::broadcast::Receiver<SessionEventRecord>,
            crate::session::service::SessionError,
        > {
            unimplemented!("not exercised by recovery::tests")
        }

        async fn wake(
            &self,
            id: &crate::session::service::SessionId,
        ) -> Result<crate::session::service::SessionHandle, crate::session::service::SessionError>
        {
            self.attach(id.clone()).await
        }

        async fn detach(
            &self,
            _id: &crate::session::service::SessionId,
        ) -> Result<(), crate::session::service::SessionError> {
            Ok(())
        }
    }

    /// A minimal no-op `ToolService` — G5's tool never dispatches a tool call,
    /// it only exercises the recovery read paths.
    struct NoopToolService;

    #[async_trait::async_trait]
    impl crate::tools::service::ToolService for NoopToolService {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
            Err(crate::tools::service::ToolError::NotFound {
                name: "test".into(),
            })
        }
        async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _: &str) -> Option<crate::tools::service::ToolDefinition> {
            None
        }
        fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    /// The parent session id every G5 fixture spawns under. Never a value
    /// `bg_child(...)` could mint, so it can never collide with a child key.
    fn g5_parent_id() -> SessionKey {
        SessionKey::main("recovery-g5-parent")
    }

    /// Addressing scope for the G5 fixtures. See
    /// `the_directory_face_reads_only_the_parent_log` for why it is not `None`.
    const G5_SCOPE: &str = "recovery-g5-scope-owned-by-nobody-else";

    fn g5_tool(
        session: std::sync::Arc<dyn crate::session::service::SessionService>,
    ) -> super::super::SubagentTool {
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(crate::providers::mock::MockProvider::new("mock"));
        let chain = crate::harness::chain_context::ChainContext::new();
        super::super::SubagentTool::new(
            provider,
            chain,
            std::sync::Arc::new(crate::agents::AgentRegistry::with_builtins()),
            std::sync::Arc::new(crate::agents::background_tracker::BackgroundAgentTracker::new()),
            session,
            std::sync::Arc::new(NoopToolService),
        )
        .with_parent_session_id(g5_parent_id().to_key_string())
    }

    /// A `SubagentTool` whose durable parent log is `parent_events`, wired to a
    /// `SessionService` that counts every `get_events` call.
    fn counting_tool(
        counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        parent_events: Vec<SessionEventRecord>,
    ) -> super::super::SubagentTool {
        let session: std::sync::Arc<dyn crate::session::service::SessionService> =
            std::sync::Arc::new(CountingSessionService {
                counter,
                parent_id: g5_parent_id(),
                parent_log: parent_events,
                child: None,
            });
        g5_tool(session)
    }

    /// Like [`counting_tool`], plus a child log reachable at the `child_id` of
    /// the `SubagentSpawned` event `parent_events` carries.
    fn counting_tool_with_child(
        counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        parent_events: Vec<SessionEventRecord>,
        child_events: Vec<SessionEventRecord>,
    ) -> super::super::SubagentTool {
        let child_id = parent_events
            .iter()
            .find_map(|r| match &r.event {
                SessionEvent::SubagentSpawned { child_id, .. } => Some(child_id.clone()),
                _ => None,
            })
            .expect("counting_tool_with_child requires a SubagentSpawned event in parent_events");
        let session: std::sync::Arc<dyn crate::session::service::SessionService> =
            std::sync::Arc::new(CountingSessionService {
                counter,
                parent_id: g5_parent_id(),
                parent_log: parent_events,
                child: Some((child_id, child_events)),
            });
        g5_tool(session)
    }

    /// G5 — the directory face must not pay for progress. `list_from_log`
    /// serves dozens of rows; one extra child-log read per interrupted child
    /// turns a cheap directory into an N-read one.
    ///
    /// Asserted on the READ COUNT, not on the rows: "asked and got nothing"
    /// and "did not ask" render identically in the output.
    ///
    /// `G5_SCOPE`, not `None`: `list_from_log`'s second half reads
    /// `background_persistence`, whose `INDEX` is a **process-global**
    /// `LazyLock<Mutex<HashMap>>`. Any other test in the same binary that
    /// enables the sidecar leaks its records into this one, and `scope = None`
    /// is documented to see *everything* — so under `--test-threads` > 1 this
    /// test read rows it never wrote and its row assertion went red at random.
    /// A scope no other fixture uses makes the sidecar half deterministically
    /// empty through the same `addressable` predicate production uses.
    #[tokio::test]
    async fn the_directory_face_reads_only_the_parent_log() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tool = counting_tool(
            counter.clone(),
            vec![rec(SessionEvent::SubagentSpawned {
                turn_id: TurnId::new_v4(),
                child_id: bg_child("agent-a", "req-1"),
                flow: "explore".into(),
                at: 1,
            })],
        );
        let rows = tool.list_from_log(&[], Some(G5_SCOPE)).await;
        assert_eq!(rows.len(), 1);
        assert!(
            matches!(&rows[0].1, Recovered::Interrupted { progress: None, .. }),
            "the directory row carries no progress"
        );
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "exactly one get_events: the parent log"
        );
    }

    /// The detail face DOES pay, because it is the answer to "tell me about
    /// this one" and already carries the child's whole text.
    #[tokio::test]
    async fn the_detail_face_loads_the_childs_progress() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tool = counting_tool_with_child(
            counter.clone(),
            /* parent */
            vec![rec(SessionEvent::SubagentSpawned {
                turn_id: TurnId::new_v4(),
                child_id: bg_child("agent-a", "req-1"),
                flow: "explore".into(),
                at: 1,
            })],
            /* child */
            vec![
                seqed(
                    1,
                    SessionEvent::RunStarted {
                        run_id: "r1".into(),
                        at: 1,
                        project_root: None,
                        envelope: None,
                    },
                ),
                seqed(
                    2,
                    SessionEvent::ToolCallRequested {
                        turn_id: TurnId::new_v4(),
                        call_id: "c1".into(),
                        name: "bash_exec".into(),
                        input: serde_json::json!({}),
                        at: 2,
                    },
                ),
            ],
        );
        let out = tool
            .resolve_forgotten(&["req-1".to_string()], Some(G5_SCOPE))
            .await;
        let Some(Recovered::Interrupted {
            progress: Some(p),
            in_flight,
            ..
        }) = out.get("req-1")
        else {
            panic!(
                "the detail face must carry progress, got {:?}",
                out.get("req-1")
            );
        };
        assert_eq!(p.tool_calls_dispatched, 1);
        assert_eq!(p.tool_calls_answered, 0);
        // The same pass that counted the dispatch names it: a count alone
        // cannot tell the model WHICH call it must not assume landed.
        assert_eq!(in_flight.len(), 1, "the dangling dispatch is named");
        assert_eq!(in_flight[0].tool_name, "bash_exec");
        assert!(!in_flight[0].denied);
    }

    // -- C: the sub-agent faces state facts -----------------------------

    fn persisted(request_id: &str, phase: RunPhase, outcome: Option<&str>) -> RecoveredRun {
        RecoveredRun {
            record: PersistedRun {
                request_id: request_id.to_string(),
                root_session: G5_SCOPE.to_string(),
                task: "count the widgets".to_string(),
                agent: "agent-a".to_string(),
                started_ms: 1,
                phase,
                ended_ms: None,
                outcome: outcome.map(str::to_string),
                partial_result_file: None,
                announced: false,
            },
            partial_result: "half a widget".to_string(),
            last_activity_ms: 2,
        }
    }

    fn dangling(tool_name: &str, denied: bool) -> DanglingCall {
        DanglingCall {
            call_id: format!("call-{tool_name}"),
            tool_name: tool_name.to_string(),
            turn_id: TurnId::new_v4(),
            seq: 7,
            provenance: crate::session::reduction::DanglingProvenance::EarlierRun,
            denied,
        }
    }

    /// C1 — the sidecar arm used to *replace* whatever the event log had
    /// found, so a child both sources knew about lost its child-session
    /// pointer, its progress and its in-flight calls. A row that is not
    /// settled is asked, and it knows which log to ask.
    #[test]
    fn a_running_sidecar_row_still_gets_asked_about_its_child_log() {
        let mut row = Recovered::sidecar(persisted("req-live", RunPhase::Running, None));
        let (child, progress, in_flight, tail, contradictions) = row
            .enrichment_slots()
            .expect("a running sidecar row is enriched from the child log");
        assert_eq!(child, bg_child("agent-a", "req-live"));
        assert!(progress.is_none(), "not asked yet");
        assert!(in_flight.is_empty() && tail.is_empty() && contradictions.is_empty());
    }

    /// The other side of the same gate: a settled run's work is over, so the
    /// detail face does not pay a read to second-guess it.
    #[test]
    fn a_settled_sidecar_row_is_not_asked_about() {
        let mut row = Recovered::sidecar(persisted("req-done", RunPhase::Settled, Some("completed")));
        assert!(row.enrichment_slots().is_none());
    }

    /// C2 — `phase == Settled` is not `outcome == completed`. The old label
    /// answered "completed" for all four terminal outcomes and the note told
    /// the model not to re-run the task.
    #[test]
    fn a_settled_but_failed_run_is_not_labelled_completed() {
        let row = Recovered::sidecar(persisted("req-bad", RunPhase::Settled, Some("failed")));
        let json = to_json("req-bad", &row);
        assert_eq!(json["status"], "failed");
        let note = json["note"].as_str().unwrap();
        assert!(
            note.contains("ended without success"),
            "the note must say the outcome was not success: {note}"
        );
        assert!(
            !note.contains("do NOT re-run"),
            "a failed run must not be sealed as done: {note}"
        );
        // And the row face agrees with the detail face about the word.
        assert_eq!(to_list_row("req-bad", &row)["status"], "failed");
    }

    /// The control case: an actually-completed run keeps its seal.
    #[test]
    fn a_completed_run_keeps_the_do_not_re_run_seal() {
        let row = Recovered::sidecar(persisted("req-ok", RunPhase::Settled, Some("completed")));
        let json = to_json("req-ok", &row);
        assert_eq!(json["status"], "completed");
        assert!(json["note"].as_str().unwrap().contains("do NOT re-run"));
    }

    /// C5 — the note used to promise that everything the child had done "has
    /// landed", which is false for exactly the calls that have no recorded
    /// result. Those are named, and the model is told their outcome is
    /// unknown.
    #[test]
    fn the_interrupted_note_names_the_calls_whose_outcome_is_unknown() {
        let row = Recovered::Interrupted {
            child_session: bg_child("agent-a", "req-1"),
            flow: "explore".into(),
            progress: Some(RunProgress {
                tool_calls_dispatched: 2,
                tool_calls_answered: 1,
                assistant_messages: 1,
                last_activity_at: Some(9),
            }),
            in_flight: vec![dangling("apply_patch", false)],
            tail: Vec::new(),
            contradictions: Vec::new(),
        };
        let json = to_json("req-1", &row);
        let note = json["note"].as_str().unwrap();
        assert!(
            note.contains("their outcome is unknown") && note.contains("apply_patch"),
            "the in-flight call must be named as unknown: {note}"
        );
        assert!(
            !note.contains("including any file writes or commands — has landed"),
            "the blanket landed-claim is gone: {note}"
        );
        assert_eq!(json["in_flight_calls"][0]["tool_name"], "apply_patch");
        assert_eq!(json["in_flight_calls"][0]["denied"], false);
    }

    /// A denied call did not run. Rendering it in the same breath as the
    /// unknown ones would make the model re-check something the approval gate
    /// already refused.
    #[test]
    fn a_denied_call_reads_as_did_not_run_not_as_unknown() {
        let row = Recovered::Interrupted {
            child_session: bg_child("agent-a", "req-1"),
            flow: "explore".into(),
            progress: None,
            in_flight: vec![dangling("bash_exec", true)],
            tail: Vec::new(),
            contradictions: Vec::new(),
        };
        let note = to_json("req-1", &row)["note"].as_str().unwrap().to_string();
        assert!(
            note.contains("denied by the approval gate and did not run")
                && note.contains("bash_exec"),
            "{note}"
        );
        assert!(
            !note.contains("their outcome is unknown"),
            "a denied call has a known outcome: {note}"
        );
    }

    /// C8 — the pointer at `child_session` carries evidence. The tail comes
    /// from the durable event slice, oldest-of-the-last-three first.
    #[test]
    fn the_child_tail_carries_the_last_lines_of_the_childs_own_log() {
        let assistant = |text: &str| {
            rec(SessionEvent::AssistantMessage {
                turn_id: TurnId::new_v4(),
                content: crate::session::events::MessageContent {
                    text: text.to_string(),
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                usage: None,
                at: 1,
            })
        };
        let events = vec![
            assistant("one"),
            assistant("two"),
            assistant("three"),
            assistant("four"),
        ];
        let tail = child_tail(&events);
        assert_eq!(
            tail.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            vec!["two", "three", "four"],
            "the LAST three, in log order"
        );
        assert!(tail.iter().all(|l| l.role == "assistant"));
    }

    /// The bound is a cap, not a promise of length.
    #[test]
    fn a_long_tail_line_is_bounded() {
        let long = "x".repeat(CHILD_TAIL_CHARS * 3);
        let events = vec![rec(SessionEvent::ToolError {
            turn_id: TurnId::new_v4(),
            call_id: "c1".into(),
            error: long,
            at: 1,
        })];
        let tail = child_tail(&events);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].role, "tool_error");
        assert_eq!(tail[0].text.chars().count(), CHILD_TAIL_CHARS);
    }
}
