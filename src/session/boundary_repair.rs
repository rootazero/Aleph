//! Crash-boundary repair — the one place a dangling tool call gets answered.
//!
//! A run that died between `ToolCallRequested` and its receipt leaves a
//! `tool_use` block with no `tool_result`. `harness::agent::prompt` drops such
//! an orphan from the replay, so the model stops seeing that the call ever
//! happened — while its side effects may still be on disk. A missing row reads
//! as "there was no value"; this module makes the log say the true thing
//! instead.
//!
//! # Why it lives here and not in the resume coordinator
//!
//! It used to live in `gateway::resume_coordinator`, which made it a boot-scan
//! private: `ProjectionReconciler`, the sub-agent recovery path and the doctor
//! check all needed the same three sentences and could not reach them without
//! pulling in a coordinator, a semaphore and an execution adapter. The repair
//! is a pure derivation over a [`RunReduction`] plus two store calls; nothing
//! about it is a gateway concern.
//!
//! # Idempotence
//!
//! [`repair_boundary`] appends exactly the repairs its argument reduction
//! names. It does **not** re-read the log to decide — the caller does, and that
//! is what makes a second call a no-op: once the first pass appended a
//! `ToolError` per dangling call, a reduction taken over the re-read log has no
//! dangling calls left and `repairs_for` returns an empty vector. Reducing
//! inside would hide a caller that repairs from a stale reduction.
//!
//! The append itself is still a read-then-append (`load_head_seq`, then N
//! appends), so two concurrent repairs of one session would both compute the
//! same set. Serialising that is the caller's job — `ResumeCoordinator` holds
//! its `in_flight` slot across the whole candidate, and the team path holds the
//! dispatcher's task-row lock.

use crate::session::events::{now_ms, SessionEvent};
use crate::session::reduction::{DanglingProvenance, RunReduction};
use crate::session::service::{SessionError, SessionId};
use crate::session::store::SessionEventStore;

/// One extra true sentence the caller wants every repaired call to carry.
///
/// The crash boundary is the only place a resumed run can be told something
/// about *itself* before it reads its first token, so a resume that had to
/// degrade — a pinned model that no longer exists, a `project_root` that is
/// gone — says so here rather than nowhere. Deliberately a sentence and not a
/// code: the consumer is the model, and R7 leaves the judgement to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DegradeNote {
    pub sentence: String,
}

impl DegradeNote {
    #[must_use]
    pub fn new(sentence: impl Into<String>) -> Self {
        Self {
            sentence: sentence.into(),
        }
    }
}

/// What [`repair_boundary`] actually wrote.
///
/// A count, not a bool: "did the repair run" is answerable by a caller that
/// appended nothing, and the two must not read alike.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepairReport {
    pub appended: usize,
}

/// The instruction every arm ends with. Shared so the three sentences cannot
/// drift apart on the one point that tells the model what to *do*.
const VERIFY_CLOSE: &str =
    "Verify the current state before deciding whether to repeat it.";

/// The sentence a dangling call is answered with.
///
/// Deliberately **not** a safety-level classifier. `ToolSafetyLevel` exists and
/// could sort read-only calls from destructive ones, but deciding "is this safe
/// to redo?" from a tool name and its arguments is exactly the reasoning R7
/// reserves for the model. State the fact; let it judge.
///
/// Three arms because there are three true sentences, and the third one is the
/// reason `denied` is a field on [`crate::session::reduction::DanglingCall`]
/// rather than a detail of the approval path: a call the approval gate refused
/// **did not run**, so telling it "this may have completed and its side effects
/// have already landed" is a fabrication, and the model's most likely reaction
/// to that fabrication is to go looking for state that does not exist.
#[must_use]
pub fn boundary_repair_text(
    tool: &str,
    provenance: DanglingProvenance,
    denied: bool,
    degrade: Option<&DegradeNote>,
) -> String {
    let body = if denied {
        format!(
            "NOT EXECUTED — this `{tool}` call was denied by the approval gate and did not \
             run, and no result was ever recorded for it. Nothing it would have done has \
             happened: no file writes, no commands, no network calls, no change to external \
             state. {VERIFY_CLOSE}"
        )
    } else {
        let lead = match provenance {
            DanglingProvenance::ThisRestart => format!(
                "the server restarted after this `{tool}` call was dispatched but before its \
                 result was recorded"
            ),
            DanglingProvenance::EarlierRun => format!(
                "an earlier run in this session ended without recording the result of this \
                 `{tool}` call"
            ),
        };
        format!(
            "OUTCOME UNKNOWN — {lead}. This is NOT a report that the call failed: it may have \
             completed, and any side effects it has (file writes, commands, network calls, \
             external state) have already landed. {VERIFY_CLOSE}"
        )
    };
    match degrade {
        Some(note) => format!("{body} {}", note.sentence),
        None => body,
    }
}

/// Turn a reduction's dangling set into appendable answer events.
///
/// **Both provenances get an event.** Leaving the older ones unanswered is not
/// the cheaper option: `build_prompt` drops an orphan `tool_use` whose result
/// never arrives, so the model stops seeing that the call ever happened — while
/// its side effects may still be on disk.
///
/// The answer is shaped as `ToolError` because there is no result to hand back:
/// a synthetic `ToolResult` would make an invented payload indistinguishable
/// from the tool's real output.
///
/// `degrade` rides on the **first** repair only. It is one fact about the
/// resume, not one fact per call, and repeating it N times would read to the
/// model as N separate degradations.
#[must_use]
pub fn repairs_for(reduction: &RunReduction, degrade: Option<&DegradeNote>) -> Vec<SessionEvent> {
    let at = now_ms();
    reduction
        .dangling
        .iter()
        .enumerate()
        .map(|(i, call)| SessionEvent::ToolError {
            turn_id: call.turn_id,
            call_id: call.call_id.clone(),
            error: boundary_repair_text(
                &call.tool_name,
                call.provenance,
                call.denied,
                if i == 0 { degrade } else { None },
            ),
            at,
        })
        .collect()
}

/// Append a synthetic `ToolError` for every dangling call `reduction` names.
///
/// # Errors
///
/// Propagates the store's own errors. A failed `load_head_seq` is never
/// defaulted to `1`: a transient read failure is indistinguishable from an
/// empty session, and guessing `1` for a non-empty session would overwrite its
/// first event.
pub async fn repair_boundary(
    store: &dyn SessionEventStore,
    session: &SessionId,
    reduction: &RunReduction,
    degrade: Option<&DegradeNote>,
) -> Result<RepairReport, SessionError> {
    let repairs = repairs_for(reduction, degrade);
    if repairs.is_empty() {
        // A degrade note with nothing to attach it to is the caller's problem
        // to place (`SystemMessage`), not this function's to invent a carrier
        // for. Saying so here rather than silently dropping it.
        return Ok(RepairReport::default());
    }
    let mut next = store.load_head_seq(session).await? + 1;
    let mut appended = 0usize;
    for ev in repairs {
        store.append(session, next, &ev, now_ms()).await?;
        next += 1;
        appended += 1;
    }
    Ok(RepairReport { appended })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord, ToolOutput, TurnId};
    use crate::session::reduction::reduce_run;

    fn rec(seq: EventSeq, event: SessionEvent, created_at_ms: i64) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms,
        }
    }

    fn run_started(at: i64) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: format!("run-{at}"),
            at,
            project_root: None,
            envelope: None,
        }
    }

    fn tool_requested(call_id: &str) -> SessionEvent {
        SessionEvent::ToolCallRequested {
            turn_id: TurnId::new_v4(),
            call_id: call_id.to_string(),
            name: "bash_exec".to_string(),
            input: serde_json::json!({}),
            at: 0,
        }
    }

    fn tool_denied(call_id: &str) -> SessionEvent {
        SessionEvent::ToolCallDenied {
            turn_id: TurnId::new_v4(),
            call_id: call_id.to_string(),
            reason: "operator said no".to_string(),
            at: 0,
        }
    }

    fn tool_result(call_id: &str) -> SessionEvent {
        SessionEvent::ToolResult {
            turn_id: TurnId::new_v4(),
            call_id: call_id.to_string(),
            output: ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: 0,
        }
    }

    /// Every arm must carry the five semantic points, asserted on MEANING
    /// rather than bytes: `!contains("failed")` gets hit by the text's own
    /// negation sentence, which is how the first version of this guard went red
    /// for the wrong reason.
    fn assert_shared_points(text: &str, tool: &str) {
        assert!(
            text.contains(tool),
            "must name the tool so the model knows what to verify, got: {text}"
        );
        assert!(
            text.contains(VERIFY_CLOSE),
            "must tell the model to check state before repeating, got: {text}"
        );
    }

    #[test]
    fn the_three_arms_are_three_different_sentences() {
        let restart = boundary_repair_text("bash_exec", DanglingProvenance::ThisRestart, false, None);
        let earlier = boundary_repair_text("bash_exec", DanglingProvenance::EarlierRun, false, None);
        let denied = boundary_repair_text("bash_exec", DanglingProvenance::ThisRestart, true, None);

        for text in [&restart, &earlier, &denied] {
            assert_shared_points(text, "bash_exec");
        }

        assert!(restart.contains("OUTCOME UNKNOWN"));
        assert!(restart.contains("NOT a report that the call failed"));
        assert!(restart.contains("side effects"));
        assert!(restart.contains("the server restarted"));

        assert!(earlier.contains("an earlier run in this session"));
        assert!(
            !earlier.contains("the server restarted"),
            "an older dangle must not be blamed on this restart: {earlier}"
        );

        assert!(
            denied.contains("did not run"),
            "a denied call must be told it did not run: {denied}"
        );
        assert!(
            !denied.contains("may have completed"),
            "a denied call never ran; claiming it may have is the fabrication this \
             arm exists to prevent: {denied}"
        );
        assert!(
            denied.contains("no file writes"),
            "a denied call must deny the side effects, not merely omit them: {denied}"
        );

        assert_ne!(restart, earlier);
        assert_ne!(restart, denied);
        assert_ne!(earlier, denied);
    }

    #[test]
    fn a_denied_dangling_call_is_answered_with_the_denied_arm() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(3, tool_denied("c1"), 30),
        ];
        let reduction = reduce_run(&events).expect("legal log");
        let repairs = repairs_for(&reduction, None);
        assert_eq!(repairs.len(), 1, "a denied call is still unanswered");
        let SessionEvent::ToolError { error, .. } = &repairs[0] else {
            panic!("expected ToolError, got {:?}", repairs[0]);
        };
        assert!(error.contains("did not run"), "got: {error}");
        assert!(!error.contains("OUTCOME UNKNOWN"), "got: {error}");
    }

    #[test]
    fn repairs_speak_a_different_sentence_per_provenance() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(3, run_started(30), 30),
            rec(4, tool_requested("c2"), 40),
        ];
        let repairs = repairs_for(&reduce_run(&events).expect("legal log"), None);
        assert_eq!(repairs.len(), 2, "BOTH provenances get a repair event");
        let mut texts = Vec::new();
        for ev in &repairs {
            let SessionEvent::ToolError { call_id, error, .. } = ev else {
                panic!("expected ToolError, got {ev:?}");
            };
            assert_shared_points(error, "bash_exec");
            texts.push((call_id.clone(), error.clone()));
        }
        assert_eq!(texts[0].0, "c1");
        assert!(texts[0].1.contains("an earlier run in this session"));
        assert_eq!(texts[1].0, "c2");
        assert!(texts[1].1.contains("the server restarted"));
    }

    #[test]
    fn repairs_are_empty_when_every_call_is_answered() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(3, tool_result("c1"), 30),
        ];
        assert!(repairs_for(&reduce_run(&events).expect("legal log"), None).is_empty());
    }

    #[test]
    fn the_degrade_note_rides_on_the_first_repair_only() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(3, tool_requested("c2"), 30),
        ];
        let note = DegradeNote::new("This session resumes on m-new; m-old was retired.");
        let repairs = repairs_for(&reduce_run(&events).expect("legal log"), Some(&note));
        assert_eq!(repairs.len(), 2);
        let text = |ev: &SessionEvent| match ev {
            SessionEvent::ToolError { error, .. } => error.clone(),
            other => panic!("expected ToolError, got {other:?}"),
        };
        assert!(
            text(&repairs[0]).contains("resumes on m-new"),
            "the first repair carries the degrade sentence"
        );
        assert!(
            !text(&repairs[1]).contains("resumes on m-new"),
            "one degradation is one fact, not one per dangling call"
        );
    }

    #[tokio::test]
    async fn repairing_twice_over_a_re_read_log_appends_nothing_the_second_time() {
        let store: std::sync::Arc<dyn SessionEventStore> = std::sync::Arc::new(
            crate::session::store::SqliteEventStore::new({
                let conn = rusqlite::Connection::open_in_memory().expect("sqlite");
                crate::session::store::migrate_add_session_events(&conn).expect("migrate");
                conn
            }),
        );
        let sid: SessionId = crate::routing::session_key::SessionKey::ephemeral(
            "boundary-repair-idempotence",
        );
        for (seq, ev) in [(1, run_started(10)), (2, tool_requested("c1"))] {
            store.append(&sid, seq, &ev, 10).await.expect("append");
        }

        let first = {
            let events = store.load_all_events(&sid).await.expect("load");
            let reduction = reduce_run(&events).expect("legal log");
            repair_boundary(store.as_ref(), &sid, &reduction, None)
                .await
                .expect("repair")
        };
        assert_eq!(first.appended, 1);

        let second = {
            let events = store.load_all_events(&sid).await.expect("load");
            let reduction = reduce_run(&events).expect("legal log");
            repair_boundary(store.as_ref(), &sid, &reduction, None)
                .await
                .expect("repair")
        };
        assert_eq!(
            second.appended, 0,
            "the re-read log has no dangling call left to answer"
        );
    }
}
