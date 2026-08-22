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

/// The crossing itself, against a real event log.
///
/// The tests above decide *what* to carry; these assert *where it lands* and
/// *what it is once it gets there* — the two halves a shape test cannot see.
mod crossing {
    use std::sync::Arc;

    use super::super::promote_latest_exchange;
    use crate::gateway::btw::side_key_for;
    use crate::session::events::{MessageContent, RunOutcome, SessionEvent, TurnTrigger};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::service::{SessionId, SessionService};
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

    /// A real event log on an in-memory SQLite connection — the same fixture the
    /// seeding tests build, so what these assert about appending is what
    /// production does rather than what a stub agreed to.
    fn service() -> Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
        migrate_add_session_events(&conn).expect("migrate session_events");
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    fn text(s: &str) -> MessageContent {
        MessageContent {
            text: s.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }
    }

    /// One answered side question, written the way a side run writes one.
    async fn answered_side_turn(session: &dyn SessionService, side: &SessionId) {
        let turn_id = uuid::Uuid::new_v4();
        for event in [
            SessionEvent::TurnStarted {
                turn_id,
                trigger: TurnTrigger::UserMessage,
                at: 0,
            },
            SessionEvent::UserMessage {
                turn_id,
                content: text("/btw what is X?"),
                at: 0,
                synthetic: false,
                author_user_id: None,
            },
            SessionEvent::AssistantMessage {
                turn_id,
                content: text("X is the config loader."),
                usage: None,
                at: 0,
            },
            SessionEvent::RunFinished {
                run_id: "run-side".into(),
                outcome: RunOutcome::Completed,
                at: 0,
            },
        ] {
            session.emit_event(side, event).await.expect("emit");
        }
    }

    #[tokio::test]
    async fn what_crosses_is_a_carrier_the_prompt_layer_can_tell_from_user_speech() {
        let session = service();
        let main = SessionId::main("promote-crossing");
        let side = side_key_for(&main);
        answered_side_turn(session.as_ref(), &side).await;

        let carried = promote_latest_exchange(session.as_ref(), &side, &main)
            .await
            .expect("the read and the append both succeed")
            .expect("there is a completed exchange to carry");
        assert_eq!(carried.question, "what is X?");

        let main_log = session
            .get_events(&main, None, None)
            .await
            .expect("read the main log");
        assert_eq!(
            main_log.len(),
            1,
            "the crossing is ONE event: the model reads `session_events` and the \
             projector materialises that same append into `messages`, so a second \
             append would be a second answer to what crossed"
        );
        let SessionEvent::UserMessage {
            content,
            synthetic,
            author_user_id,
            ..
        } = &main_log[0].event
        else {
            panic!("the carrier rides the user role: {:?}", main_log[0].event);
        };
        assert!(
            *synthetic,
            "an unflagged user event is re-wrapped by the prompt builder in the \
             interjection fence, which re-classifies the carrier as words the user \
             typed — the one failure this carrier exists to prevent"
        );
        assert_eq!(
            author_user_id.as_deref(),
            None,
            "nobody said this; in a room it must not be attributed to whoever typed \
             `/btw promote`"
        );
        assert!(
            crate::thinker::nudges::is_synthetic_reminder(&content.text),
            "the text that crossed must classify as scaffolding: {:?}",
            content.text
        );
        assert!(content.text.contains("X is the config loader."));
        assert!(
            content.text.contains("what is X?"),
            "the question gives the answer its referent"
        );

        assert_eq!(
            session
                .get_events(&side, None, None)
                .await
                .expect("read the side log")
                .len(),
            4,
            "promoting reads the side thread; it must not write to it, or the next \
             promote would carry its own output"
        );
    }

    /// Nothing to promote is an answer, and it leaves no trace.
    #[tokio::test]
    async fn an_absent_side_thread_carries_nothing_and_writes_nothing() {
        let session = service();
        let main = SessionId::main("promote-empty");
        let side = side_key_for(&main);

        let carried = promote_latest_exchange(session.as_ref(), &side, &main)
            .await
            .expect("an absent side thread is not a fault");
        assert!(carried.is_none());
        assert!(
            session
                .get_events(&main, None, None)
                .await
                .expect("read the main log")
                .is_empty(),
            "an empty promote must not put an empty carrier in the conversation"
        );
    }
}
