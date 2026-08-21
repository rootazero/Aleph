use super::*;
use crate::session::events::{MessageContent, RunOutcome, TurnId, TurnTrigger};

fn rec(seq: u64, event: SessionEvent) -> SessionEventRecord {
    SessionEventRecord {
        seq,
        event,
        created_at_ms: 1_700_000_000_000 + seq as i64,
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

fn turn_open(turn_id: TurnId) -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id,
        trigger: TurnTrigger::UserMessage,
        at: 1,
    }
}

fn user(turn_id: TurnId, text: &str) -> SessionEvent {
    SessionEvent::UserMessage {
        turn_id,
        content: content(text),
        at: 1,
        synthetic: false,
        author_user_id: None,
    }
}

fn assistant(turn_id: TurnId, text: &str) -> SessionEvent {
    SessionEvent::AssistantMessage {
        turn_id,
        content: content(text),
        usage: None,
        at: 1,
    }
}

fn finished(outcome: RunOutcome) -> SessionEvent {
    SessionEvent::RunFinished {
        run_id: "run-side".into(),
        outcome,
        at: 1,
    }
}

/// Build a log out of events, numbering the seqs in order.
fn log(events: Vec<SessionEvent>) -> Vec<SessionEventRecord> {
    events
        .into_iter()
        .enumerate()
        .map(|(i, e)| rec(i as u64 + 1, e))
        .collect()
}

/// One completed side turn, read back whole.
#[test]
fn the_latest_completed_turn_is_the_one_promoted() {
    let first = TurnId::new_v4();
    let second = TurnId::new_v4();
    let records = log(vec![
        turn_open(first),
        user(first, "/btw what is X?"),
        assistant(first, "X is the config loader."),
        finished(RunOutcome::Completed),
        turn_open(second),
        user(second, "/btw and Y?"),
        assistant(second, "Y is the router."),
        finished(RunOutcome::Completed),
    ]);

    let promoted = latest_complete_exchange(&records).expect("a completed turn exists");
    assert_eq!(promoted.question, "and Y?");
    assert_eq!(promoted.answer, "Y is the router.");
}

/// The failure this exists for: a superseded question whose run has not landed.
///
/// The TUI files a superseded side question with whatever text had arrived, so
/// the newest turn in the log can be an answer that is still being written. It
/// must not be the one promoted — a truncated answer in the main transcript
/// reads exactly like a complete one.
#[test]
fn a_turn_still_being_answered_is_never_promoted() {
    let done = TurnId::new_v4();
    let inflight = TurnId::new_v4();
    let records = log(vec![
        turn_open(done),
        user(done, "/btw what is X?"),
        assistant(done, "X is the config loader."),
        finished(RunOutcome::Completed),
        turn_open(inflight),
        user(inflight, "/btw walk me through the whole boot path"),
        assistant(inflight, "Sure — first the daemon"),
    ]);

    let promoted = latest_complete_exchange(&records).expect("the earlier turn is promotable");
    assert_eq!(
        promoted.answer, "X is the config loader.",
        "the half-written answer of the in-flight turn crossed into the main conversation"
    );
}

/// A cancelled or errored run left a partial answer, not an answer.
#[test]
fn an_interrupted_turn_is_not_a_completed_exchange() {
    for outcome in [
        RunOutcome::Cancelled,
        RunOutcome::Errored,
        RunOutcome::Abandoned,
    ] {
        let turn = TurnId::new_v4();
        let records = log(vec![
            turn_open(turn),
            user(turn, "/btw what is X?"),
            assistant(turn, "X is the con"),
            finished(outcome),
        ]);
        assert_eq!(
            latest_complete_exchange(&records),
            None,
            "{outcome:?} left a partial answer, and a partial answer is not what the user asked to promote"
        );
    }
}

/// The other half of "the latest side exchange": a side log is not only its own.
///
/// `seed` copies the main conversation's settled prefix in, and those events
/// carry no turn markers (`fork::is_prompt_bearing` drops `TurnStarted`, and
/// keeps `RunFinished` only when it is `Cancelled`). Promoting one would push a
/// slice of the main conversation back into the main conversation.
#[test]
fn a_turn_copied_in_from_the_main_conversation_is_not_promotable() {
    let copied = TurnId::new_v4();
    let records = log(vec![
        SessionEvent::SessionForked {
            parent_session_id: "agent:main:main".into(),
            at: 1,
        },
        user(copied, "deploy the thing"),
        assistant(copied, "Deployed."),
    ]);

    assert_eq!(
        latest_complete_exchange(&records),
        None,
        "the seeded main transcript has no turn markers of its own, and promoting \
         it would carry the main conversation back into itself"
    );
}

/// Scaffolding on the user role is not the question.
#[test]
fn a_harness_nudge_is_not_read_as_the_question() {
    let turn = TurnId::new_v4();
    let records = log(vec![
        turn_open(turn),
        user(turn, "/btw what is X?"),
        SessionEvent::synthetic_user(turn, crate::thinker::nudges::MAX_STEPS_HINT.to_string()),
        assistant(turn, "X is the config loader."),
        finished(RunOutcome::Completed),
    ]);

    let promoted = latest_complete_exchange(&records).expect("a completed turn exists");
    assert_eq!(promoted.question, "what is X?");
}

/// A tool-assisted turn emits one assistant message per Think step; the
/// intermediate ones are the tool calls, not the answer.
#[test]
fn the_final_assistant_step_is_the_answer() {
    let turn = TurnId::new_v4();
    let records = log(vec![
        turn_open(turn),
        user(turn, "/btw which file loads config?"),
        assistant(turn, "Let me grep for it."),
        SessionEvent::ToolCallRequested {
            turn_id: turn,
            call_id: "c1".into(),
            name: "file_read".into(),
            input: serde_json::json!({"path": "src/config/mod.rs"}),
            at: 1,
        },
        assistant(turn, ""),
        assistant(turn, "`src/config/mod.rs`."),
        finished(RunOutcome::Completed),
    ]);

    let promoted = latest_complete_exchange(&records).expect("a completed turn exists");
    assert_eq!(promoted.answer, "`src/config/mod.rs`.");
}

/// The `/btw` prefix comes off through the one resolver, so every spelling the
/// resolver knows keeps working here — including the ones a hand-rolled
/// four-character strip would lose.
#[test]
fn the_question_is_resolved_not_stripped() {
    let turn = TurnId::new_v4();
    for (raw, expected) in [
        ("/btw what is X?", "what is X?"),
        ("/BTW What Is X?", "What Is X?"),
        ("/btw@MyBot what is X?", "what is X?"),
        // Not a side question at all — a mid-turn steer lands on this role too,
        // and its own text is the honest rendering.
        ("actually, focus on Y", "actually, focus on Y"),
    ] {
        let records = log(vec![
            turn_open(turn),
            user(turn, raw),
            assistant(turn, "answer"),
            finished(RunOutcome::Completed),
        ]);
        assert_eq!(
            latest_complete_exchange(&records)
                .expect("a completed turn exists")
                .question,
            expected
        );
    }
}

/// An empty log is "nothing to promote", not a failure.
#[test]
fn an_empty_side_log_promotes_nothing() {
    assert_eq!(latest_complete_exchange(&[]), None);
}
