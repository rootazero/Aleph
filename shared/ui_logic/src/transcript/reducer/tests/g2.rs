//! The replay leg, and G2: live and replay fold one run to the same
//! entries modulo clocks.

use super::*;
use crate::transcript::{step_headline, step_tally, trace_result_to_wire};

// ---- replay leg + G2 -------------------------------------------------

/// A replay row is the same `AgentTraceEvent` a live `agent_trace`
/// frame carries. The two legs share `apply_trace`; this fixture checks
/// the parts that DIFFER around it: no `RunAccepted`, no deltas, no
/// `RunComplete`, and `finish_replay` doing what `complete_run` does.
///
/// `text_first` puts iteration 2's `TextEmitted{Final}` before its
/// `ReasoningEmitted` — the order production emits them in; the fold
/// must not depend on either. `salvage` adds the verifier-halt salvage
/// path's second `ReasoningEmitted` on iteration 1. Iteration 1 reads
/// two files, so the run crosses the turn-summary gate and both legs'
/// trailer inputs are compared.
fn full_run_trace_with(text_first: bool, salvage: bool) -> Vec<AgentTraceEvent> {
    let read = |id: &str, path: &str, ms: u64| {
        [
            AgentTraceEvent::ToolCallStarted {
                iteration: 1,
                call: AgentTraceToolCallStart {
                    tool_id: id.into(),
                    tool_name: "file_read".into(),
                    input: json!({ "path": path }),
                },
            },
            AgentTraceEvent::ToolCallCompleted {
                iteration: 1,
                call: AgentTraceToolCallEnd {
                    tool_id: id.into(),
                    tool_name: "file_read".into(),
                    input: json!({ "path": path }),
                    duration_ms: ms,
                    presentation: None,
                },
                result: AgentTraceToolResult::Success {
                    output: json!("fn a() {}"),
                },
            },
        ]
    };
    let mut rows = vec![
        AgentTraceEvent::TurnStarted { iteration: 1 },
        AgentTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: "Look at the failing test first.".into(),
        },
    ];
    if salvage {
        rows.push(AgentTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: "The veto says a box is unchecked.".into(),
        });
    }
    rows.extend(read("r1", "tests/a.rs", 12));
    rows.extend(read("r2", "src/b.rs", 8));
    rows.push(AgentTraceEvent::TurnStarted { iteration: 2 });
    let thinking = AgentTraceEvent::ReasoningEmitted {
        iteration: 2,
        text: "The timezone is the bug.".into(),
    };
    let text = AgentTraceEvent::TextEmitted {
        iteration: 2,
        stream: AgentTraceTextKind::Final,
        text: "Fixed the timezone handling.".into(),
    };
    if text_first {
        rows.extend([text, thinking]);
    } else {
        rows.extend([thinking, text]);
    }
    rows.push(AgentTraceEvent::SessionCompleted {
        outcome: aleph_protocol::AgentTraceSessionOutcome::Completed,
        iterations: 2,
        tool_calls_made: 1,
        total_tokens: 100,
        hit_limit: false,
        final_text: Some("Fixed the timezone handling.".into()),
        terminate_reason: None,
        duration_ms: Some(32_000),
        token_breakdown: None,
        tool_timeline: Vec::new(),
    });
    rows
}

fn full_run_trace() -> Vec<AgentTraceEvent> {
    full_run_trace_with(true, false)
}

/// How the last iteration's records reached the live leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grace {
    /// A normal Think: each record's deltas, split in two, then it.
    No,
    /// The boundary grace turn (`fire_boundary_grace_turn`): no
    /// `TurnStarted` of its own, its text and thinking streamed as ONE
    /// delta each, then `TextEmitted{Final}` + `ReasoningEmitted` on the
    /// halted iteration. The halted Think streamed nothing.
    Clean,
    /// The same, after the halted Think had streamed partial thinking and
    /// partial text that no record covers and that are NOT prefixes of
    /// the grace turn's records.
    AfterPartial,
}

/// The live leg for the same run: the same trace frames interleaved
/// with the deltas and the lifecycle frames a client actually receives.
fn full_run_live(rows: Vec<AgentTraceEvent>, grace: Grace) -> Vec<StreamEvent> {
    let deltas = |text: &str| -> Vec<String> {
        if grace == Grace::No {
            let (a, b) = text.split_at(text.len() / 2);
            vec![a.to_string(), b.to_string()]
        } else {
            vec![text.to_string()]
        }
    };
    let mut out = vec![accepted()];
    for ev in rows {
        match &ev {
            AgentTraceEvent::TurnStarted { iteration: 2 } if grace == Grace::AfterPartial => {
                out.push(trace(ev));
                out.push(reasoning("Maybe the parser drops"));
                out.push(chunk("I think the bug is in the par"));
            }
            AgentTraceEvent::ReasoningEmitted { text, .. } => {
                // deltas first, then the authoritative record
                out.extend(deltas(text).iter().map(|d| reasoning(d)));
                out.push(trace(ev));
            }
            AgentTraceEvent::TextEmitted { text, .. } => {
                out.extend(deltas(text).iter().map(|d| chunk(d)));
                out.push(trace(ev));
            }
            AgentTraceEvent::ToolCallStarted { call, .. } => {
                out.push(StreamEvent::ToolStart {
                    run_id: RUN.into(),
                    seq: 0,
                    tool_name: call.tool_name.clone(),
                    tool_id: call.tool_id.clone(),
                    params: call.input.clone(),
                });
                out.push(trace(ev));
            }
            AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                out.push(StreamEvent::ToolEnd {
                    run_id: RUN.into(),
                    seq: 0,
                    tool_id: call.tool_id.clone(),
                    result: trace_result_to_wire(result, call.presentation.as_ref()),
                    duration_ms: call.duration_ms,
                });
                out.push(trace(ev));
            }
            _ => out.push(trace(ev)),
        }
    }
    out.push(StreamEvent::RunComplete {
        run_id: RUN.into(),
        seq: 0,
        summary: RunSummary {
            loops: 2,
            tool_summaries: vec![
                item("r1", "file_read", 12, true),
                item("r2", "file_read", 8, true),
            ],
            final_response: Some("Fixed the timezone handling.".into()),
            ..Default::default()
        },
        total_duration_ms: 32_000,
    });
    out
}

/// Clocks are the one thing the replay leg cannot know.
fn strip_clocks(entries: &[TranscriptEntry]) -> Vec<TranscriptEntry> {
    entries
        .iter()
        .cloned()
        .map(|e| match e {
            TranscriptEntry::Step(mut s) => {
                s.started_ms = None;
                s.ended_ms = None;
                for r in &mut s.tools {
                    r.started_ms = None;
                    r.ended_ms = None;
                    if let RowStatus::Running { since_ms } = &mut r.status {
                        *since_ms = 0;
                    }
                }
                TranscriptEntry::Step(s)
            }
            other => other,
        })
        .collect()
}

fn replay(rows: &[AgentTraceEvent]) -> Transcript {
    let mut t = Transcript::new();
    for ev in rows {
        t.apply_replay(ev);
    }
    t.finish_replay();
    t
}

/// G2. Two legs, one fold — in either within-turn order, across the
/// salvage path's two records on one iteration, and across the boundary
/// grace turn with and without a halted Think's partial stream.
#[test]
fn the_live_leg_and_the_replay_leg_fold_to_the_same_entries() {
    let shapes = [
        (true, false, Grace::No),
        (false, false, Grace::No),
        (true, true, Grace::No),
        (true, false, Grace::Clean),
        (true, false, Grace::AfterPartial),
    ];
    for (text_first, salvage, grace) in shapes {
        let rows = full_run_trace_with(text_first, salvage);
        let mut live = Transcript::new();
        let live_changes = drive(&mut live, &full_run_live(rows.clone(), grace));
        assert!(
            !live_changes.contains(&Change::NeedsResync),
            "{live_changes:?}"
        );
        let replay = replay(&rows);

        let (l, r) = (strip_clocks(live.entries()), strip_clocks(replay.entries()));
        assert_eq!(
            l, r,
            "text_first={text_first} salvage={salvage} grace={grace:?}\nLIVE:   {l:#?}\nREPLAY: {r:#?}"
        );
        assert!(
            l.iter()
                .any(|e| matches!(e, TranscriptEntry::TurnSummary(_))),
            "the fixture must cross the turn-summary gate: {l:#?}"
        );
    }

    // And the shape is the one the surfaces will paint:
    let replay = replay(&full_run_trace());
    let s = steps(&replay);
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].iteration, Some(1));
    assert_eq!(s[0].tools[0].status, RowStatus::Ok { duration_ms: 12 });
    assert_eq!(s[0].tools.len(), 2);
    assert_eq!(
        s[1].thinking.as_ref().map(|b| b.text.as_str()),
        Some("The timezone is the bug.")
    );
    assert_eq!(
        finals(&replay),
        vec!["Fixed the timezone handling.".to_string()]
    );
    assert_eq!(
        step_headline(s[0], 80).map(|h| h.text),
        Some("Look at the failing test first.".to_string())
    );
}

#[test]
fn a_replayed_run_without_a_session_completed_row_is_pending_not_settled() {
    let mut t = Transcript::new();
    let rows = full_run_trace();
    // The first four rows: TurnStarted 1, ReasoningEmitted 1,
    // ToolCallStarted r1, ToolCallCompleted r1 — the log stops there.
    for ev in &rows[..4] {
        t.apply_replay(ev);
    }
    t.apply_replay(&AgentTraceEvent::TurnStarted { iteration: 2 });
    t.apply_replay(&AgentTraceEvent::ToolCallStarted {
        iteration: 2,
        call: AgentTraceToolCallStart {
            tool_id: "b".into(),
            tool_name: "bash".into(),
            input: json!({"command": "cargo test"}),
        },
    });
    t.finish_replay();
    let s = steps(&t);
    assert_eq!(
        s[0].status,
        StepStatus::Settled,
        "a step the next turn closed is settled"
    );
    assert_eq!(
        s[1].status,
        StepStatus::Pending,
        "the last step never ended: unknown"
    );
    assert_eq!(
        s[1].tools[0].status,
        RowStatus::Pending,
        "never a spinner from a log"
    );
    assert!(
        finals(&t).is_empty(),
        "no answer was recorded, none is invented"
    );
}

#[test]
fn replay_rows_carry_no_clocks_but_the_recorded_durations() {
    let t = replay(&full_run_trace());
    let row = &steps(&t)[0].tools[0];
    assert_eq!((row.started_ms, row.ended_ms), (None, None));
    let summary = t.entries().iter().find_map(|e| match e {
        TranscriptEntry::TurnSummary(s) => Some(s.duration_ms),
        _ => None,
    });
    assert_eq!(summary, Some(20), "the recorded durations, 12 + 8");
    assert_eq!(step_tally(steps(&t)[0]).map(|s| s.duration_ms), Some(20));
}

/// One transcript holds a session's runs. The replay leg has no
/// `tool_summaries` list to filter by, so "all rows" must mean THIS
/// run's rows: a one-tool run after a two-tool run is still below the
/// turn-summary gate.
#[test]
fn a_replayed_run_counts_only_its_own_rows_in_the_turn_summary() {
    let tool = |id: &str| {
        [
            AgentTraceEvent::ToolCallStarted {
                iteration: 1,
                call: AgentTraceToolCallStart {
                    tool_id: id.into(),
                    tool_name: "bash".into(),
                    input: json!({"command": "ls"}),
                },
            },
            AgentTraceEvent::ToolCallCompleted {
                iteration: 1,
                call: AgentTraceToolCallEnd {
                    tool_id: id.into(),
                    tool_name: "bash".into(),
                    input: json!({"command": "ls"}),
                    duration_ms: 5,
                    presentation: None,
                },
                result: AgentTraceToolResult::Success {
                    output: json!("ok"),
                },
            },
        ]
    };
    let done = AgentTraceEvent::SessionCompleted {
        outcome: aleph_protocol::AgentTraceSessionOutcome::Completed,
        iterations: 1,
        tool_calls_made: 1,
        total_tokens: 1,
        hit_limit: false,
        final_text: None,
        terminate_reason: None,
        duration_ms: Some(1_000),
        token_breakdown: None,
        tool_timeline: Vec::new(),
    };
    let mut t = Transcript::new();
    t.apply_replay(&AgentTraceEvent::TurnStarted { iteration: 1 });
    for ev in tool("a1").iter().chain(tool("a2").iter()) {
        t.apply_replay(ev);
    }
    t.apply_replay(&done);
    t.finish_replay();
    t.apply_replay(&AgentTraceEvent::TurnStarted { iteration: 1 });
    for ev in &tool("b1") {
        t.apply_replay(ev);
    }
    t.apply_replay(&done);
    t.finish_replay();
    let summaries = t
        .entries()
        .iter()
        .filter(|e| matches!(e, TranscriptEntry::TurnSummary(_)))
        .count();
    assert_eq!(
        summaries,
        1,
        "only the two-tool run earns one: {:?}",
        t.entries()
    );
}
