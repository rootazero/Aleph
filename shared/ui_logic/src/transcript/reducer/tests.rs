//! The live leg, run by run and frame by frame. The replay leg and G2 are
//! in `g2`.

use super::{Change, Transcript};
use crate::transcript::{
    Note, NoteKind, RowBody, RowStatus, StepEntry, StepStatus, ToolRow, TranscriptEntry,
};
use aleph_protocol::events::ToolSummaryItem;
use aleph_protocol::{
    AgentTraceEvent, AgentTraceTextKind, AgentTraceToolCallEnd, AgentTraceToolCallStart,
    AgentTraceToolResult, RunSummary, StreamEvent, ToolResult,
};
use serde_json::json;

const RUN: &str = "run-1";

fn accepted() -> StreamEvent {
    StreamEvent::RunAccepted {
        run_id: RUN.into(),
        session_key: "s".into(),
        accepted_at: "t".into(),
    }
}
fn reasoning(s: &str) -> StreamEvent {
    StreamEvent::Reasoning {
        run_id: RUN.into(),
        seq: 0,
        content: s.into(),
        is_complete: false,
    }
}
fn chunk(s: &str) -> StreamEvent {
    StreamEvent::ResponseChunk {
        run_id: RUN.into(),
        seq: 0,
        content: s.into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    }
}
fn trace(ev: AgentTraceEvent) -> StreamEvent {
    StreamEvent::AgentTrace {
        run_id: RUN.into(),
        seq: 0,
        event: ev,
    }
}
fn turn(i: usize) -> StreamEvent {
    trace(AgentTraceEvent::TurnStarted { iteration: i })
}
fn thought(i: usize, text: &str) -> StreamEvent {
    trace(AgentTraceEvent::ReasoningEmitted {
        iteration: i,
        text: text.into(),
    })
}
fn tool_start(id: &str, name: &str) -> StreamEvent {
    StreamEvent::ToolStart {
        run_id: RUN.into(),
        seq: 0,
        tool_name: name.into(),
        tool_id: id.into(),
        params: json!({"command": "ls"}),
    }
}
fn tool_end(id: &str, ms: u64) -> StreamEvent {
    StreamEvent::ToolEnd {
        run_id: RUN.into(),
        seq: 0,
        tool_id: id.into(),
        result: ToolResult::success("ok"),
        duration_ms: ms,
    }
}
fn complete(loops: u32, summaries: Vec<ToolSummaryItem>) -> StreamEvent {
    StreamEvent::RunComplete {
        run_id: RUN.into(),
        seq: 0,
        summary: RunSummary {
            loops,
            tool_summaries: summaries,
            ..Default::default()
        },
        total_duration_ms: 32_000,
    }
}
fn item(id: &str, name: &str, ms: u64, ok: bool) -> ToolSummaryItem {
    ToolSummaryItem {
        tool_id: id.into(),
        tool_name: name.into(),
        emoji: String::new(),
        duration_ms: ms,
        success: ok,
    }
}
fn steps(t: &Transcript) -> Vec<&StepEntry> {
    t.entries()
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::Step(s) => Some(s),
            _ => None,
        })
        .collect()
}
fn finals(t: &Transcript) -> Vec<String> {
    t.entries()
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::AssistantText { markdown, .. } => Some(markdown.clone()),
            _ => None,
        })
        .collect()
}
fn drive(t: &mut Transcript, frames: &[StreamEvent]) -> Vec<Change> {
    let mut out = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        out.extend(t.apply_live(f, 1_000 + i as u64));
    }
    out
}

#[test]
fn a_frame_before_any_turn_started_opens_an_unnumbered_step_that_is_never_renumbered() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[accepted(), reasoning("early"), turn(1), chunk("hi")],
    );
    let s = steps(&t);
    assert_eq!(s.len(), 2, "{:?}", t.entries());
    assert_eq!(s[0].iteration, None);
    assert_eq!(
        s[0].thinking.as_ref().map(|b| b.text.as_str()),
        Some("early")
    );
    assert_eq!(s[1].iteration, Some(1));
    assert_eq!(s[1].text.as_deref(), Some("hi"));
}

#[test]
fn turn_started_closes_the_previous_step_and_opens_the_next() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[accepted(), turn(1), chunk("a"), turn(2), chunk("b")],
    );
    let s = steps(&t);
    assert_eq!(s[0].status, StepStatus::Settled);
    assert!(s[0].ended_ms.is_some());
    assert_eq!(s[1].status, StepStatus::Live);
    assert_eq!(
        (s[0].text.as_deref(), s[1].text.as_deref()),
        (Some("a"), Some("b"))
    );
}

#[test]
fn streamed_thinking_is_replaced_by_the_authoritative_record() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            reasoning("Weigh"),
            reasoning("ing…"),
            trace(AgentTraceEvent::ReasoningEmitted {
                iteration: 1,
                text: "Weighing the two readings.".into(),
            }),
        ],
    );
    let b = steps(&t)[0].thinking.clone().unwrap();
    assert_eq!(b.text, "Weighing the two readings.");
    assert!(!b.streaming);
}

/// The verifier-halt salvage path re-runs Think on the SAME iteration and
/// emits a second record. Records accumulate in arrival order; only the
/// streamed deltas each record covers are replaced.
#[test]
fn two_reasoning_records_on_one_iteration_are_kept_in_arrival_order() {
    let mut t = Transcript::new();
    let changes = drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            reasoning("Fir"),
            thought(1, "First pass."),
            reasoning("Sec"),
            thought(1, "Second pass."),
        ],
    );
    assert!(!changes.contains(&Change::NeedsResync), "{changes:?}");
    let b = steps(&t)[0].thinking.clone().unwrap();
    assert_eq!(b.text, "First pass.\n\nSecond pass.");
    assert!(!b.streaming);
}

/// Thinking + tool calls and no text: the record still lands on the step,
/// and the step survives the run's end (it holds thinking and a tool).
#[test]
fn a_tool_only_step_gets_its_thinking() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            tool_start("a", "bash"),
            tool_end("a", 4),
            thought(1, "List the directory first."),
            complete(1, vec![item("a", "bash", 4, true)]),
        ],
    );
    let s = steps(&t);
    assert_eq!(s.len(), 1, "{:?}", t.entries());
    assert_eq!(
        s[0].thinking.as_ref().map(|b| b.text.as_str()),
        Some("List the directory first.")
    );
    assert_eq!(s[0].text, None);
    assert_eq!(s[0].tools.len(), 1);
    assert!(finals(&t).is_empty(), "{:?}", t.entries());
}

/// A record naming an iteration other than the open step's cannot be
/// placed: "I don't know", never a guess (判据 §8).
#[test]
fn a_reasoning_record_for_another_iteration_asks_for_a_resync_and_changes_nothing() {
    let mut t = Transcript::new();
    drive(&mut t, &[accepted(), turn(2)]);
    let before = t.entries().to_vec();
    let changes = t.apply_live(&thought(1, "stray"), 2_000);
    assert_eq!(changes, vec![Change::NeedsResync]);
    assert_eq!(t.entries(), before.as_slice());
}

#[test]
fn final_text_from_the_trace_is_deduplicated_against_the_streamed_chunks() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            chunk("Hel"),
            chunk("lo"),
            trace(AgentTraceEvent::TextEmitted {
                iteration: 1,
                stream: AgentTraceTextKind::Final,
                text: "Hello world".into(),
            }),
        ],
    );
    assert_eq!(steps(&t)[0].text.as_deref(), Some("Hello world"));
}

#[test]
fn parallel_tool_calls_in_one_iteration_share_the_step_and_settle_independently() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            tool_start("a", "bash"),
            tool_start("b", "bash"),
            // the trace face repeats the start: must not reset the row
            trace(AgentTraceEvent::ToolCallStarted {
                iteration: 1,
                call: AgentTraceToolCallStart {
                    tool_id: "a".into(),
                    tool_name: "bash".into(),
                    input: json!({"command": "ls"}),
                },
            }),
            tool_end("b", 7),
        ],
    );
    let s = steps(&t);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].tools.len(), 2);
    assert!(matches!(s[0].tools[0].status, RowStatus::Running { .. }));
    assert_eq!(s[0].tools[1].status, RowStatus::Ok { duration_ms: 7 });
}

#[test]
fn a_tool_end_for_an_unknown_id_is_a_no_op_until_the_summary_names_it() {
    let mut t = Transcript::new();
    drive(&mut t, &[accepted(), turn(1), tool_end("ghost", 3)]);
    assert!(steps(&t)[0].tools.is_empty());
    drive(&mut t, &[complete(1, vec![item("ghost", "bash", 3, true)])]);
    let s = steps(&t);
    assert_eq!(
        s[0].tools.len(),
        1,
        "the authoritative record reconstructs it"
    );
    assert_eq!(s[0].tools[0].status, RowStatus::Ok { duration_ms: 3 });
    assert_eq!(
        s[0].tools[0].body,
        RowBody::None,
        "header-only: it knows the call, not its output"
    );
}

#[test]
fn run_complete_hoists_the_final_text_and_keeps_a_thinking_only_step() {
    // Step 1: thinking only (a veto forced a continue). Step 2: the answer.
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            reasoning("first try"),
            trace(AgentTraceEvent::VerifierVeto {
                iteration: 1,
                reason: "- [ ] tests".into(),
            }),
            turn(2),
            reasoning("second"),
            chunk("Done."),
            complete(2, vec![]),
        ],
    );
    let s = steps(&t);
    assert_eq!(s.len(), 2, "{:?}", t.entries());
    assert_eq!(s[0].notes.len(), 1);
    assert_eq!(s[0].notes[0].kind, NoteKind::VerifierVeto);
    assert_eq!(s[1].text, None, "hoisted out of the step");
    assert_eq!(
        s[1].thinking.as_ref().map(|b| b.text.as_str()),
        Some("second")
    );
    assert_eq!(finals(&t), vec!["Done.".to_string()]);
    let hoisted_after_step = t
        .entries()
        .iter()
        .position(|e| matches!(e, TranscriptEntry::AssistantText { .. }));
    let last_step = t
        .entries()
        .iter()
        .rposition(|e| matches!(e, TranscriptEntry::Step(_)));
    assert!(
        hoisted_after_step > last_step,
        "the answer follows the steps"
    );
}

#[test]
fn run_complete_removes_a_step_that_held_only_the_hoisted_text() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            chunk("Just an answer."),
            complete(1, vec![]),
        ],
    );
    assert!(steps(&t).is_empty(), "{:?}", t.entries());
    assert_eq!(finals(&t), vec!["Just an answer.".to_string()]);
}

#[test]
fn run_complete_drops_a_step_that_held_nothing() {
    // The fourth hoist cell: an iteration that produced no thinking, no
    // text, no tool and no note (e.g. an empty retried response). It is
    // not rendered as "Step N: (nothing)".
    // Step 1 is closed empty by `TurnStarted 2` and dropped there; step 2
    // holds only the answer, which the hoist takes, so it is dropped too.
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            turn(2),
            chunk("Answer."),
            complete(2, vec![]),
        ],
    );
    assert!(steps(&t).is_empty(), "{:?}", t.entries());
    assert_eq!(finals(&t), vec!["Answer.".to_string()]);
    assert!(
        t.entries().iter().any(
            |e| matches!(e, TranscriptEntry::SystemNotice { text, .. } if text.ends_with("· 2 steps"))
        ),
        "the trailer still counts the iterations the loop ran: {:?}",
        t.entries()
    );
}

/// PF-D9 hoist cell: the OPEN step at `RunComplete` (not one a
/// `TurnStarted` closed) holds only thinking and the record carries an
/// empty answer. The step stays; no empty answer is invented.
#[test]
fn run_complete_keeps_a_thinking_only_open_step_when_final_response_is_empty() {
    let mut t = Transcript::new();
    drive(&mut t, &[accepted(), turn(1), reasoning("only a thought")]);
    let ev = StreamEvent::RunComplete {
        run_id: RUN.into(),
        seq: 0,
        summary: RunSummary {
            loops: 1,
            final_response: Some(String::new()),
            ..Default::default()
        },
        total_duration_ms: 10,
    };
    t.apply_live(&ev, 5_000);
    let s = steps(&t);
    assert_eq!(s.len(), 1, "{:?}", t.entries());
    assert_eq!(s[0].status, StepStatus::Settled);
    assert_eq!(
        s[0].thinking.as_ref().map(|b| b.text.as_str()),
        Some("only a thought")
    );
    assert_eq!(s[0].text, None);
    assert!(
        finals(&t).is_empty(),
        "no answer was recorded, none is invented"
    );
}

/// PF-D9 hoist cell: the OPEN step at `RunComplete` holds nothing at
/// all. It is removed by the hoist, not left as "Step 1: (nothing)".
#[test]
fn run_complete_drops_an_open_step_that_held_nothing() {
    let mut t = Transcript::new();
    drive(&mut t, &[accepted(), turn(1)]);
    let changes = t.apply_live(&complete(1, vec![]), 5_000);
    assert!(steps(&t).is_empty(), "{:?}", t.entries());
    assert!(finals(&t).is_empty());
    assert!(
        changes.iter().any(|c| matches!(c, Change::Removed(_))),
        "the removal is reported so a keyed surface drops the row: {changes:?}"
    );
}

#[test]
fn run_complete_with_thinking_and_text_keeps_the_thinking_row_beside_the_answer() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            reasoning("why"),
            chunk("Answer."),
            complete(1, vec![]),
        ],
    );
    let s = steps(&t);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].thinking.as_ref().map(|b| b.text.as_str()), Some("why"));
    assert_eq!(s[0].text, None);
    assert_eq!(finals(&t), vec!["Answer.".to_string()]);
}

#[test]
fn run_complete_with_nothing_rendered_falls_back_to_final_response() {
    let mut t = Transcript::new();
    drive(&mut t, &[accepted(), turn(1)]);
    let ev = StreamEvent::RunComplete {
        run_id: RUN.into(),
        seq: 0,
        summary: RunSummary {
            loops: 1,
            final_response: Some("From the summary.  ".into()),
            ..Default::default()
        },
        total_duration_ms: 10,
    };
    t.apply_live(&ev, 5_000);
    assert_eq!(finals(&t), vec!["From the summary.".to_string()]);
}

#[test]
fn run_complete_produces_the_turn_summary_and_the_worked_for_notice() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            tool_start("a", "bash"),
            tool_end("a", 100),
            turn(2),
            tool_start("b", "file_read"),
            tool_end("b", 50),
            chunk("Done."),
            complete(
                2,
                vec![
                    item("a", "bash", 100, true),
                    item("b", "file_read", 50, true),
                ],
            ),
        ],
    );
    let summary = t.entries().iter().find_map(|e| match e {
        TranscriptEntry::TurnSummary(s) => Some(s.clone()),
        _ => None,
    });
    let summary = summary.expect("a turn summary");
    assert_eq!(
        (summary.commands, summary.reads, summary.duration_ms),
        (1, 1, 150)
    );
    let notice = t.entries().iter().rev().find_map(|e| match e {
        TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(notice.as_deref(), Some("✻ Worked for 32s · 2 steps"));
}

type Skeleton = (String, Option<u32>, Option<String>, Vec<ToolRow>, Vec<Note>);

/// What a step IS, minus what the run's end legitimately does to it:
/// its status/clock (the run still completes) and its text (hoisted —
/// asserted separately through `finals`).
fn step_skeleton(t: &Transcript) -> Vec<Skeleton> {
    steps(t)
        .into_iter()
        .map(|s| {
            (
                s.id.clone(),
                s.iteration,
                s.thinking.as_ref().map(|b| b.text.clone()),
                s.tools.clone(),
                s.notes.clone(),
            )
        })
        .collect()
}

#[test]
fn fewer_turn_starts_than_summary_loops_reports_needs_resync_without_touching_entries() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[accepted(), turn(1), reasoning("weigh"), chunk("a")],
    );
    let before = step_skeleton(&t);
    assert_eq!(before.len(), 1, "fixture: one step that survives the hoist");
    let changes = t.apply_live(&complete(3, vec![]), 9_000);
    assert!(changes.contains(&Change::NeedsResync), "{changes:?}");
    // "Never patches" = no step is fabricated for the missing loops and
    // none is renumbered: same ids, iterations and contents.
    assert_eq!(step_skeleton(&t), before);
    // The run still completes normally (hoist etc.) — resync is the
    // client's next move, not a reason to leave the run half-open.
    assert_eq!(finals(&t), vec!["a".to_string()]);
}

#[test]
fn a_run_with_zero_loops_never_asks_for_a_resync() {
    // simple.rs / slash paths: no trace, no iterations counted.
    let mut t = Transcript::new();
    drive(&mut t, &[accepted(), reasoning("x"), chunk("y")]);
    let changes = t.apply_live(&complete(0, vec![]), 9_000);
    assert!(!changes.contains(&Change::NeedsResync));
}

#[test]
fn run_error_settles_rows_hoists_the_partial_answer_and_leaves_a_notice() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            tool_start("a", "bash"),
            chunk("half an ans"),
        ],
    );
    let ev = StreamEvent::RunError {
        run_id: RUN.into(),
        seq: 0,
        error: "provider down".into(),
        error_code: None,
    };
    t.apply_live(&ev, 5_000);
    assert_eq!(
        steps(&t)[0].tools[0].status,
        RowStatus::Pending,
        "never a spinner after the run ended"
    );
    assert_eq!(finals(&t), vec!["half an ans".to_string()]);
    let notice = t.entries().iter().rev().find_map(|e| match e {
        TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(notice.as_deref(), Some("Error: provider down"));
}

#[test]
fn a_tool_summary_becomes_a_note_on_the_open_step() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            trace(AgentTraceEvent::ToolSummary {
                iteration: 1,
                summary: "Read the failing test.".into(),
            }),
        ],
    );
    let n = &steps(&t)[0].notes[0];
    assert_eq!(n.kind, NoteKind::ToolSummary);
    assert!(n.text.contains("Read the failing test."), "{}", n.text);
}

#[test]
fn cache_health_is_a_run_level_notice_not_a_step_note() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            trace(AgentTraceEvent::CacheHealthDegraded {
                scope: "main".into(),
                streak: 3,
                reads: 0,
                writes: 900,
                prefix_changed: Some(true),
            }),
        ],
    );
    assert!(steps(&t)[0].notes.is_empty());
    assert!(t
        .entries()
        .iter()
        .any(|e| matches!(e, TranscriptEntry::SystemNotice { .. })));
}

#[test]
fn a_frame_for_another_run_is_ignored() {
    let mut t = Transcript::new();
    drive(&mut t, &[accepted(), turn(1)]);
    let foreign = StreamEvent::ResponseChunk {
        run_id: "run-other".into(),
        seq: 0,
        content: "nope".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    };
    assert!(t.apply_live(&foreign, 1).is_empty());
    assert_eq!(steps(&t)[0].text, None);
}

#[test]
fn push_user_appends_a_user_row_with_its_time() {
    let mut t = Transcript::new();
    let c = t.push_user("hi", Some(42), vec![]);
    assert!(matches!(c, Change::Inserted(_)));
    assert!(matches!(
        &t.entries()[0],
        TranscriptEntry::UserText { text, at_ms: Some(42), .. } if text == "hi"
    ));
}

#[test]
fn a_tool_call_completed_with_a_presentation_lands_it_on_the_row() {
    use aleph_protocol::file_change::{FileChange, FileChangeKind, Presentation, Unavailable};
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            trace(AgentTraceEvent::ToolCallStarted {
                iteration: 1,
                call: AgentTraceToolCallStart {
                    tool_id: "e".into(),
                    tool_name: "file_edit".into(),
                    input: json!({"file_path": "a.rs"}),
                },
            }),
            trace(AgentTraceEvent::ToolCallCompleted {
                iteration: 1,
                call: AgentTraceToolCallEnd {
                    tool_id: "e".into(),
                    tool_name: "file_edit".into(),
                    input: json!({"file_path": "a.rs"}),
                    duration_ms: 9,
                    presentation: Some(Presentation::FileChanges {
                        changes: vec![FileChange::unavailable(
                            "a.rs",
                            FileChangeKind::Modified,
                            Unavailable::TooLarge,
                        )],
                    }),
                },
                result: AgentTraceToolResult::Success {
                    output: json!("ok"),
                },
            }),
        ],
    );
    let row = &steps(&t)[0].tools[0];
    assert!(matches!(row.body, RowBody::FileChanges(ref c) if c.len() == 1));
    assert_eq!(row.status, RowStatus::Ok { duration_ms: 9 });
}

// ---- fix round 1 ----------------------------------------------------

fn text_record(i: usize, text: &str) -> StreamEvent {
    trace(AgentTraceEvent::TextEmitted {
        iteration: i,
        stream: AgentTraceTextKind::Final,
        text: text.into(),
    })
}

/// The same frame, addressed to another run.
fn in_run(run: &str, ev: StreamEvent) -> StreamEvent {
    let run_id = run.to_string();
    match ev {
        StreamEvent::RunAccepted {
            session_key,
            accepted_at,
            ..
        } => StreamEvent::RunAccepted {
            run_id,
            session_key,
            accepted_at,
        },
        StreamEvent::AgentTrace { seq, event, .. } => {
            StreamEvent::AgentTrace { run_id, seq, event }
        }
        StreamEvent::ToolStart {
            seq,
            tool_name,
            tool_id,
            params,
            ..
        } => StreamEvent::ToolStart {
            run_id,
            seq,
            tool_name,
            tool_id,
            params,
        },
        StreamEvent::ToolEnd {
            seq,
            tool_id,
            result,
            duration_ms,
            ..
        } => StreamEvent::ToolEnd {
            run_id,
            seq,
            tool_id,
            result,
            duration_ms,
        },
        StreamEvent::RunComplete {
            seq,
            summary,
            total_duration_ms,
            ..
        } => StreamEvent::RunComplete {
            run_id,
            seq,
            summary,
            total_duration_ms,
        },
        other => panic!("in_run: no arm for {other:?}"),
    }
}

/// F1(a): the boundary grace turn records a different text on the halted
/// iteration after that Think streamed a partial one. The record is the
/// text — it replaces the streamed segment instead of being dropped.
#[test]
fn a_text_record_replaces_a_halted_turns_partial_text() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            chunk("I think the bug is in the par"),
            chunk("The timezone handling was wrong."),
            text_record(1, "The timezone handling was wrong."),
        ],
    );
    assert_eq!(
        steps(&t)[0].text.as_deref(),
        Some("The timezone handling was wrong.")
    );
    t.apply_live(&complete(1, vec![]), 9_000);
    assert_eq!(
        finals(&t),
        vec!["The timezone handling was wrong.".to_string()]
    );
}

/// F1(b): deltas that ARE the record converge to it with no duplication.
#[test]
fn streamed_text_that_spells_the_record_converges_without_duplication() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            chunk("Hello "),
            chunk("world"),
            text_record(1, "Hello world"),
        ],
    );
    assert_eq!(steps(&t)[0].text.as_deref(), Some("Hello world"));
}

/// F1(c): a streaming call failed mid-stream and was rescued without
/// streaming; the rescue's full text is delta'd again from the start.
#[test]
fn a_rescued_call_restreamed_from_the_start_converges_to_the_record() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            chunk("Fixed the ti"),
            chunk("Fixed the timezone."),
            text_record(1, "Fixed the timezone."),
        ],
    );
    assert_eq!(steps(&t)[0].text.as_deref(), Some("Fixed the timezone."));
}

/// Ruling T910-SUM: the trailer counts every row of the run on the live
/// leg too — the record's `tool_summaries` only reconciles rows the
/// client missed.
#[test]
fn the_turn_summary_counts_every_row_of_the_run_not_only_the_records_list() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            tool_start("a", "bash"),
            tool_end("a", 10),
            tool_start("b", "bash"),
            tool_end("b", 20),
            chunk("Done."),
            complete(1, vec![item("a", "bash", 10, true)]),
        ],
    );
    let summary = t.entries().iter().find_map(|e| match e {
        TranscriptEntry::TurnSummary(s) => Some(s.clone()),
        _ => None,
    });
    let summary = summary.expect("two rows cross the gate");
    assert_eq!((summary.commands, summary.duration_ms), (2, 30));
}

/// F2: thinking that was only ever streamed stops streaming when its step
/// closes — at the next `TurnStarted` and at the run's end.
#[test]
fn a_delta_only_thinking_step_stops_streaming_when_it_closes() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            reasoning("first"),
            turn(2),
            reasoning("second"),
            complete(2, vec![]),
        ],
    );
    let s = steps(&t);
    assert_eq!(s.len(), 2, "{:?}", t.entries());
    assert!(s
        .iter()
        .all(|s| s.thinking.as_ref().is_some_and(|b| !b.streaming)));
}

/// F11: mid-stream, a delta segment after a record starts a new
/// paragraph — the join the next record will use.
#[test]
fn a_delta_after_a_record_starts_a_new_paragraph_mid_stream() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[accepted(), turn(1), thought(1, "First."), reasoning("Sec")],
    );
    let b = steps(&t)[0].thinking.clone().unwrap();
    assert_eq!(b.text, "First.\n\nSec");
    assert!(b.streaming);
}

/// F4: a provider may repeat a tool id across runs. The next run gets its
/// own row; the earlier run's finished row is not touched.
#[test]
fn a_tool_id_repeated_by_the_next_run_gets_its_own_row() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            tool_start("t1", "bash"),
            tool_end("t1", 5),
            complete(1, vec![item("t1", "bash", 5, true)]),
        ],
    );
    let second: Vec<StreamEvent> = [
        accepted(),
        turn(1),
        tool_start("t1", "bash"),
        tool_end("t1", 9),
        complete(1, vec![item("t1", "bash", 9, true)]),
    ]
    .into_iter()
    .map(|f| in_run("run-2", f))
    .collect();
    drive(&mut t, &second);
    let s = steps(&t);
    assert_eq!(s.len(), 2, "{:?}", t.entries());
    assert_eq!(s[0].tools.len(), 1);
    assert_eq!(s[0].tools[0].status, RowStatus::Ok { duration_ms: 5 });
    assert_eq!(s[1].tools.len(), 1);
    assert_eq!(s[1].tools[0].status, RowStatus::Ok { duration_ms: 9 });
}

/// F5: a run accepted while the previous one never reported its end.
/// How that run ended is unknown: its step closes `Pending` and its rows
/// stop spinning; the new run folds into a step of its own.
#[test]
fn a_run_accepted_over_an_open_run_closes_it_pending() {
    let mut t = Transcript::new();
    drive(
        &mut t,
        &[
            accepted(),
            turn(1),
            reasoning("thinking"),
            tool_start("a", "bash"),
        ],
    );
    let changes = t.apply_live(&in_run("run-2", accepted()), 5_000);
    assert!(!changes.is_empty(), "the old step's change is reported");
    t.apply_live(&in_run("run-2", turn(1)), 5_001);
    let s = steps(&t);
    assert_eq!(s.len(), 2, "{:?}", t.entries());
    assert_eq!(s[0].status, StepStatus::Pending);
    assert_eq!(s[0].tools[0].status, RowStatus::Pending);
    assert!(s[0].thinking.as_ref().is_some_and(|b| !b.streaming));
    assert_eq!(s[1].status, StepStatus::Live);
}

mod g2;
