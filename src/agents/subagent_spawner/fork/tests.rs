//! Tests for [`super`] — fork selection policy and its drift guards.

use super::*;
use crate::session::events::{MessageContent, RunOutcome, ToolOutput};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn rec(event: SessionEvent) -> SessionEventRecord {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    SessionEventRecord {
        seq: SEQ.fetch_add(1, Ordering::Relaxed),
        event,
        created_at_ms: 0,
    }
}

fn content(text: &str) -> MessageContent {
    MessageContent {
        text: text.to_string(),
        blocks: Vec::new(),
        thinking: None,
        thinking_signature: None,
    }
}

fn user(turn: TurnId, text: &str) -> SessionEventRecord {
    rec(SessionEvent::UserMessage {
        turn_id: turn,
        content: content(text),
        at: 0,
        synthetic: false,
        author_user_id: None,
    })
}

fn assistant(turn: TurnId, text: &str) -> SessionEventRecord {
    rec(SessionEvent::AssistantMessage {
        turn_id: turn,
        content: content(text),
        usage: None,
        at: 0,
    })
}

fn call(turn: TurnId, id: &str) -> SessionEventRecord {
    rec(SessionEvent::ToolCallRequested {
        turn_id: turn,
        call_id: id.to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({}),
        at: 0,
    })
}

fn result(turn: TurnId, id: &str, body: &str) -> SessionEventRecord {
    rec(SessionEvent::ToolResult {
        turn_id: turn,
        call_id: id.to_string(),
        output: ToolOutput {
            value: serde_json::Value::String(body.to_string()),
            metadata: Default::default(),
        },
        at: 0,
    })
}

/// Three complete turns followed by the in-flight turn that is calling us.
fn parent_log() -> (Vec<SessionEventRecord>, Vec<TurnId>) {
    let turns: Vec<TurnId> = (0..4).map(|_| uuid::Uuid::new_v4()).collect();
    let mut log = Vec::new();
    for (i, t) in turns.iter().take(3).enumerate() {
        log.push(user(*t, &format!("ask {i}")));
        log.push(call(*t, &format!("c{i}")));
        log.push(result(*t, &format!("c{i}"), &format!("out {i}")));
        log.push(assistant(*t, &format!("answer {i}")));
    }
    // The live turn: the model asked for a sub-agent and no result exists yet.
    let live = turns[3];
    log.push(user(live, "now delegate this"));
    log.push(call(live, "spawn-call"));
    (log, turns)
}

fn unbounded() -> ForkBudget {
    ForkBudget {
        max_turns: None,
        max_chars: usize::MAX,
    }
}

// ---------------------------------------------------------------------------
// Drift guards — the two directions the kind list can rot
// ---------------------------------------------------------------------------

/// Every `SessionEvent` variant name `prompt.rs` production code mentions, and
/// the set [`is_prompt_bearing`] answers `true` for, must be the same set.
///
/// This is the guard for the failure a fork has and nothing else does: someone
/// teaches `build_prompt` to render a new event kind, forks silently stop
/// carrying it, and the only symptom is a child that is missing something the
/// parent could see — no error, no red test, nothing to grep for.
///
/// **CRLF**: this checkout is CRLF (git autocrlf), and a source-level guard
/// whose separator has any character before its `\n` matches nothing here while
/// passing on CI's LF — the exact shape CLAUDE.md §10 records. `\r` is stripped
/// before anything is split, and the non-empty assertions below fail loudly
/// rather than silently scanning zero bytes.
#[test]
fn fork_kinds_track_the_prompt_builder() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/harness/agent/prompt.rs"
    ))
    .expect("prompt.rs is readable")
    .replace('\r', "");

    // Production prefix only: the test module below constructs events of every
    // kind as fixtures, so scanning it would report the whole enum as
    // prompt-bearing and the guard would pass on anything.
    let production = crate::utils::source_scan::production_prefix(&src);
    assert!(
        production.len() < src.len(),
        "no `#[cfg(test)]` found in prompt.rs — the split did not remove the \
         fixtures, so this guard is scanning them and cannot fail"
    );

    let mut mentioned: Vec<String> = Vec::new();
    for chunk in production.split("SessionEvent::").skip(1) {
        let name: String = chunk
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && !mentioned.contains(&name) {
            mentioned.push(name);
        }
    }
    assert!(
        !mentioned.is_empty(),
        "found no `SessionEvent::` references in prompt.rs production code — \
         the builder was restructured and this guard is now blind"
    );
    mentioned.sort();

    let mut bearing: Vec<String> = sample_of_every_kind()
        .into_iter()
        .filter(|(_, e)| is_prompt_bearing(e))
        .map(|(name, _)| name.to_string())
        .collect();
    bearing.sort();

    assert_eq!(
        mentioned, bearing,
        "the event kinds `prompt.rs` renders and the kinds a fork carries have \
         diverged. Left = mentioned by the prompt builder, right = carried by a \
         fork. A kind on the left only is silently dropped from every forked \
         child; a kind on the right only is dead weight copied for nothing."
    );
}

/// One constructed event per `SessionEvent` variant.
///
/// The exhaustive `match` in [`is_prompt_bearing`] makes the compiler force a
/// decision on every new variant; this list makes the *guard above* see it too.
/// Kept as `(name, event)` pairs so the guard compares names, not shapes.
fn sample_of_every_kind() -> Vec<(&'static str, SessionEvent)> {
    let t = uuid::Uuid::new_v4();
    vec![
        (
            "SessionWoken",
            SessionEvent::SessionWoken {
                at: 0,
                prior_head: 0,
            },
        ),
        (
            "RunStarted",
            SessionEvent::RunStarted {
                run_id: "r".into(),
                at: 0,
                project_root: None,
            },
        ),
        (
            "RunFinished",
            SessionEvent::RunFinished {
                run_id: "r".into(),
                outcome: RunOutcome::Cancelled,
                at: 0,
            },
        ),
        (
            "TurnStarted",
            SessionEvent::TurnStarted {
                turn_id: t,
                trigger: crate::session::events::TurnTrigger::SubagentRequest,
                at: 0,
            },
        ),
        ("UserMessage", user(t, "u").event),
        ("AssistantMessage", assistant(t, "a").event),
        (
            "SystemMessage",
            SessionEvent::SystemMessage {
                turn_id: t,
                content: "s".into(),
                at: 0,
            },
        ),
        ("ToolCallRequested", call(t, "c").event),
        (
            "ToolCallApproved",
            SessionEvent::ToolCallApproved {
                turn_id: t,
                call_id: "c".into(),
                by: crate::session::events::ApprovalSource::Trusted,
                at: 0,
            },
        ),
        (
            "ToolCallDenied",
            SessionEvent::ToolCallDenied {
                turn_id: t,
                call_id: "c".into(),
                reason: "no".into(),
                at: 0,
            },
        ),
        ("ToolResult", result(t, "c", "o").event),
        (
            "ToolError",
            SessionEvent::ToolError {
                turn_id: t,
                call_id: "c".into(),
                error: "e".into(),
                at: 0,
            },
        ),
        (
            "AssistantRunMeta",
            SessionEvent::AssistantRunMeta {
                turn_id: t,
                run_id: "r".into(),
                context_tokens: 0,
                context_window: 0,
                total_tokens: 0,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: None,
                model: None,
                model_provider: None,
                at: 0,
            },
        ),
        (
            "SubagentSpawned",
            SessionEvent::SubagentSpawned {
                turn_id: t,
                child_id: crate::routing::session_key::SessionKey::parse("agent:main:peer:x")
                    .expect("fixture key parses"),
                flow: "f".into(),
                at: 0,
            },
        ),
        (
            "SubagentReturned",
            SessionEvent::SubagentReturned {
                turn_id: t,
                child_id: crate::routing::session_key::SessionKey::parse("agent:main:peer:x")
                    .expect("fixture key parses"),
                summary: "s".into(),
                at: 0,
            },
        ),
        (
            "CompactionPerformed",
            SessionEvent::CompactionPerformed {
                from_seq: 0,
                to_seq: 1,
                summary_ref: "s".into(),
                at: 0,
            },
        ),
        (
            "SessionForked",
            SessionEvent::SessionForked {
                parent_session_id: "p".into(),
                at: 0,
            },
        ),
        (
            "Error",
            SessionEvent::Error {
                turn_id: None,
                kind: crate::session::events::ErrorKind::Guardrail,
                message: "m".into(),
                recoverable: false,
                at: 0,
            },
        ),
    ]
}

/// `RunFinished` is prompt-bearing only when it marks a cancellation.
///
/// Carrying `Completed` would inject an interruption note into a child whose
/// parent was never interrupted — a fabricated event, which is worse than a
/// missing one because the model cannot tell it is fabricated.
#[test]
fn only_a_cancelled_run_marker_is_carried() {
    for (outcome, expected) in [
        (RunOutcome::Cancelled, true),
        (RunOutcome::Completed, false),
        (RunOutcome::Errored, false),
    ] {
        let e = SessionEvent::RunFinished {
            run_id: "r".into(),
            outcome,
            at: 0,
        };
        assert_eq!(is_prompt_bearing(&e), expected);
    }
}

// ---------------------------------------------------------------------------
// Selection policy
// ---------------------------------------------------------------------------

/// The parent is *inside* the `subagent` call while the fork is planned, so its
/// own pending `ToolCallRequested` is the newest thing in the log. Carrying it
/// would show the child "I asked someone to do this" as the last thing that
/// happened, and `build_prompt` would drop the orphan half-turn anyway.
#[test]
fn the_in_flight_turn_is_not_carried() {
    let (log, turns) = parent_log();
    let plan = plan(&log, &unbounded());

    assert_eq!(plan.turns_available, 3, "three turns are closed");
    assert_eq!(plan.turns_copied, 3);
    assert!(
        !plan.events.iter().any(|e| turn_of(e) == Some(turns[3])),
        "the live turn leaked into the fork"
    );
    assert!(
        plan.receipt().is_some_and(|n| n.contains("3 of 3")),
        "a full fork still reports itself"
    );
}

/// The stability property the whole feature rests on: two forks taken at
/// different moments of the same parent share a byte-identical *prefix*, so the
/// second one's provider cache read covers everything the first one wrote.
///
/// This is what would break if the cut moved with the in-flight turn, or if the
/// slice were taken on an event count instead of on turn boundaries.
#[test]
fn a_later_fork_extends_the_earlier_forks_prefix() {
    let (mut log, _) = parent_log();
    let early = plan(&log, &unbounded());

    // The parent's in-flight turn lands, and a new one opens.
    let live = turn_of(&log.last().expect("non-empty").event).expect("has a turn");
    log.push(result(live, "spawn-call", "child said ok"));
    log.push(assistant(live, "and so"));
    let next = uuid::Uuid::new_v4();
    log.push(user(next, "keep going"));
    log.push(call(next, "spawn-2"));

    let later = plan(&log, &unbounded());

    assert!(
        later.events.len() > early.events.len(),
        "the later fork should have gained the turn that closed"
    );
    // Compared as serialized bytes, not as values: "byte-identical prefix" is
    // literally the property the provider's cache keys on, so asserting it in
    // that currency is the assertion, not a proxy for it.
    let bytes = |events: &[SessionEvent]| -> Vec<String> {
        events
            .iter()
            .map(|e| serde_json::to_string(e).expect("events serialize"))
            .collect()
    };
    assert_eq!(
        bytes(&later.events[..early.events.len()]),
        bytes(&early.events),
        "the later fork must EXTEND the earlier one, not re-render it — a \
         changed prefix is a full cache re-write, which is the cost this \
         feature exists to avoid"
    );
}

#[test]
fn max_turns_takes_the_newest_whole_turns() {
    let (log, turns) = parent_log();
    let plan = plan(
        &log,
        &ForkBudget {
            max_turns: Some(2),
            max_chars: usize::MAX,
        },
    );

    assert_eq!(plan.turns_copied, 2);
    assert_eq!(plan.turns_available, 3);
    assert!(
        !plan.events.iter().any(|e| turn_of(e) == Some(turns[0])),
        "the oldest turn should have been dropped, not the newest"
    );
    // Whole turns: the surviving turns keep their call/result pairs intact.
    let calls: usize = plan
        .events
        .iter()
        .filter(|e| matches!(e, SessionEvent::ToolCallRequested { .. }))
        .count();
    let results: usize = plan
        .events
        .iter()
        .filter(|e| matches!(e, SessionEvent::ToolResult { .. }))
        .count();
    assert_eq!(calls, results, "a turn was sliced through a tool pair");
}

/// Dropping history is allowed; dropping it quietly is not. A child that thinks
/// it can see the whole conversation will answer the wrong question with
/// complete confidence.
#[test]
fn a_truncated_fork_says_so() {
    let (log, _) = parent_log();
    let plan = plan(
        &log,
        &ForkBudget {
            max_turns: Some(1),
            max_chars: usize::MAX,
        },
    );
    let note = plan
        .receipt()
        .expect("a fork that carried something reports it");
    assert!(note.contains('1') && note.contains('3'), "note: {note}");
}

#[test]
fn the_char_budget_cuts_on_turn_boundaries() {
    let (log, _) = parent_log();
    let one_turn = plan(
        &log,
        &ForkBudget {
            max_turns: Some(1),
            max_chars: usize::MAX,
        },
    )
    .chars;

    // Room for two turns but not three.
    let plan = plan(
        &log,
        &ForkBudget {
            max_turns: None,
            max_chars: one_turn * 2 + one_turn / 2,
        },
    );
    assert_eq!(plan.turns_copied, 2);
    assert!(plan.chars <= one_turn * 2 + one_turn / 2);
}

/// A budget too small for even the newest turn yields nothing — and says so via
/// `is_empty`, so the caller refuses the fork rather than seeding a child with a
/// half turn that starts mid-tool-pair.
#[test]
fn a_budget_smaller_than_one_turn_carries_nothing() {
    let (log, _) = parent_log();
    let plan = plan(
        &log,
        &ForkBudget {
            max_turns: None,
            max_chars: 1,
        },
    );
    assert!(plan.is_empty());
    assert_eq!(plan.turns_copied, 0);
    assert_eq!(plan.turns_available, 3);
}

/// A copied window must never *begin* with a tool result: an Anthropic-
/// compatible backend rejects a `tool_result` with no preceding `tool_use`
/// with HTTP 400, on the child's very first call. Same snap `session_split`
/// applies at its own boundary.
#[test]
fn a_window_never_opens_on_an_orphan_tool_result() {
    // A turn whose `tool_use` was written under a *different* turn id — the
    // shape whole-turn slicing cannot produce but a synthetic closure can.
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let log = vec![
        result(a, "stray", "orphaned output"),
        user(b, "hello"),
        assistant(b, "hi"),
    ];
    let plan = plan(&log, &unbounded());
    assert!(
        !matches!(
            plan.events.first(),
            Some(SessionEvent::ToolResult { .. } | SessionEvent::ToolError { .. })
        ),
        "fork opened on an orphan tool result: {:?}",
        plan.events.first()
    );
}

#[test]
fn an_empty_parent_log_plans_an_empty_fork() {
    let plan = plan(&[], &unbounded());
    assert!(plan.is_empty());
    assert_eq!(plan.turns_available, 0);
    assert!(plan.receipt().is_none());
}

// ---------------------------------------------------------------------------
// Budget derivation
// ---------------------------------------------------------------------------

fn cfg(token_budget: u64) -> crate::context::budget::ContextBudgetConfig {
    crate::context::budget::ContextBudgetConfig {
        token_budget,
        warning_threshold: 0.70,
        critical_threshold: 0.85,
        token_estimate_ratio: 4.0,
        fresh_tail_count: 4,
        summarizer_input_budget: 48_000,
        circuit_breaker_max: 3,
        max_splits: 3,
    }
}

/// The ceiling is the child's own compaction warning line, minus the system
/// block that shares the window — not a constant somebody picked. Seeding a
/// child above that line makes its first Think pay an LLM to summarise the
/// history we just paid to copy.
#[test]
fn the_budget_is_derived_from_the_childs_own_warning_line() {
    let budget = ForkBudget::for_child(Some(&cfg(200_000)), 20_000, None)
        .expect("a 200k window leaves room");
    // 200_000 * 0.70 * 4.0 = 560_000 chars, minus the 20k system block.
    assert_eq!(budget.max_chars, 540_000);
}

/// No `[context_budget]` means no window to reason about. Inventing a ceiling
/// here would be the guessed constant `for_child` exists to avoid, and copying
/// unbounded would hand an unmanaged child a transcript it cannot compact.
#[test]
fn without_a_context_budget_there_is_no_fork_budget() {
    assert!(ForkBudget::for_child(None, 0, None).is_none());
}

/// A window the system prompt has already eaten leaves no room to fork into.
#[test]
fn a_system_prompt_that_fills_the_window_refuses_the_fork() {
    assert!(ForkBudget::for_child(Some(&cfg(8_000)), 1_000_000, None).is_none());
    // ...and the boundary is the floor, not zero.
    let tight = cfg(1_000); // 1_000 * 0.7 * 4 = 2_800 chars
    assert!(ForkBudget::for_child(Some(&tight), 0, None).is_some());
    assert!(ForkBudget::for_child(Some(&tight), 1_000, None).is_none());
}
