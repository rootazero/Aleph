use super::*;
use crate::routing::session_key::SessionKey;

#[test]
fn resolve_accepts_the_documented_spellings() {
    assert_eq!(
        BtwTurn::resolve("/btw what was that config file called?"),
        Some(BtwTurn {
            question: "what was that config file called?".into(),
            promote: false
        })
    );
    // Case-insensitive command, body case preserved verbatim for the model.
    assert_eq!(
        BtwTurn::resolve("/BTW Explain Async/Await").map(|b| b.question),
        Some("Explain Async/Await".into())
    );
    // Telegram's @botname suffix is tolerated.
    assert_eq!(
        BtwTurn::resolve("/btw@MyBot why?").map(|b| b.question),
        Some("why?".into())
    );
    // Newline separator.
    assert_eq!(
        BtwTurn::resolve("/btw\nnext line").map(|b| b.question),
        Some("next line".into())
    );
}

#[test]
fn resolve_rejects_non_btw_and_empty_bodies() {
    assert_eq!(BtwTurn::resolve("hello"), None);
    assert_eq!(BtwTurn::resolve("/help"), None);
    assert_eq!(BtwTurn::resolve("/btwlike this"), None);
    // An empty side question has nowhere to go.
    assert_eq!(BtwTurn::resolve("/btw"), None);
    assert_eq!(BtwTurn::resolve("/btw    "), None);
}

#[test]
fn resolve_recognises_the_promote_verb() {
    let b = BtwTurn::resolve("/btw promote").expect("promote parses");
    assert!(b.promote);
    assert!(b.question.is_empty());
    // "promote" as the first word of a real question is still promote —
    // documented and deliberate; ask "/btw please promote ..." to disambiguate.
    assert!(
        !BtwTurn::resolve("/btw what does promote mean?")
            .expect("q")
            .promote
    );
}

#[test]
fn the_side_key_is_derived_from_the_main_key_including_its_epoch() {
    let main = SessionKey::main("assistant");
    let bumped = main.with_epoch(1);

    let a = side_key_for(&main);
    let b = side_key_for(&main);
    let c = side_key_for(&bumped);

    // Deterministic: same main key, same side key. This is what gives the
    // side thread its memory.
    assert_eq!(a.to_key_string(), b.to_key_string());
    // Epoch-inclusive: /new bumps the epoch, so the side thread starts empty
    // by construction rather than by anyone remembering to clear it.
    assert_ne!(a.to_key_string(), c.to_key_string());
    assert!(matches!(a, SessionKey::Ephemeral { .. }));
    // Agent identity is preserved so partition/visibility predicates still work.
    assert_eq!(a.agent_id(), main.agent_id());
}

// ---------------------------------------------------------------------------
// Incremental seeding
// ---------------------------------------------------------------------------

mod seeding {
    use std::sync::Arc;

    use crate::agents::subagent_spawner::fork::ForkBudget;
    use crate::gateway::btw::seed::{ensure_seeded, interpret_cursor, SeedOutcome};
    use crate::gateway::btw::side_key_for;
    use crate::gateway::session_store::SessionStore;
    use crate::session::events::{MessageContent, SessionEvent};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::service::{SessionId, SessionService};
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

    /// A real event log on an in-memory SQLite connection — the same fixture
    /// `subagent_spawner`'s own fork tests build, so what these assert about
    /// copying is what production does rather than what a stub agreed to.
    fn in_memory_session_service() -> Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
        migrate_add_session_events(&conn).expect("migrate session_events");
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    /// The cursor's home. Returned together with its `TempDir`, which must
    /// outlive the test body or the SQLite file goes away underneath it.
    fn in_memory_session_store() -> (Arc<dyn SessionStore>, tempfile::TempDir) {
        crate::builtin_tools::agent_manage::test_utils::session_store()
    }

    /// Both services are per-test, so every test may use the same pair of keys
    /// and still be fully isolated. The side key comes from the production
    /// derivation rather than a literal.
    fn keys() -> (SessionId, SessionId) {
        let main = SessionId::main("main");
        let side = side_key_for(&main);
        (main, side)
    }

    fn content(text: &str) -> MessageContent {
        MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }
    }

    /// Append one *complete* turn: a question and its answer, sharing a turn
    /// id and leaving no open tool call. Only closed turns are eligible to be
    /// forked, so seeding an incomplete one would make every assertion here
    /// vacuously true.
    async fn append_closed_turn(
        session: &dyn SessionService,
        id: &SessionId,
        ask: &str,
        answer: &str,
    ) {
        let turn_id = uuid::Uuid::new_v4();
        session
            .emit_event(
                id,
                SessionEvent::UserMessage {
                    turn_id,
                    content: content(ask),
                    at: 0,
                    synthetic: false,
                    author_user_id: None,
                },
            )
            .await
            .expect("emit user message");
        session
            .emit_event(
                id,
                SessionEvent::AssistantMessage {
                    turn_id,
                    content: content(answer),
                    usage: None,
                    at: 0,
                },
            )
            .await
            .expect("emit assistant message");
    }

    async fn transcript_text(session: &dyn SessionService, id: &SessionId) -> String {
        session
            .get_events(id, None, None)
            .await
            .expect("read side log")
            .iter()
            .map(|r| serde_json::to_string(&r.event).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn budget() -> ForkBudget {
        ForkBudget {
            max_turns: Some(10),
            max_chars: 100_000,
        }
    }

    #[tokio::test]
    async fn seeding_twice_does_not_duplicate_the_main_prefix() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        append_closed_turn(session.as_ref(), &main, "first user turn", "first answer").await;

        let a = ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
            .await
            .expect("first seed");
        assert!(
            a.events_added > 0,
            "the first seed must carry the transcript"
        );
        assert!(
            a.cursor.is_some(),
            "a seed that carried must leave a cursor"
        );

        // Nothing new closed on the main session in between.
        let b = ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
            .await
            .expect("second seed");

        // The transcript is asserted before the receipt on purpose: it is the
        // ground truth, and a receipt that agreed with a doubled transcript
        // would be the more expensive of the two failures.
        let text = transcript_text(session.as_ref(), &side).await;
        assert_eq!(
            text.matches("first user turn").count(),
            1,
            "the main prefix appears twice — the side transcript is doubling"
        );
        assert_eq!(
            b,
            SeedOutcome {
                events_added: 0,
                cursor: a.cursor,
            },
            "a second seed with no new turns must be a no-op that keeps the cursor"
        );
    }

    #[tokio::test]
    async fn seeding_carries_only_what_closed_since_the_cursor() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        append_closed_turn(session.as_ref(), &main, "turn one", "answer one").await;
        let a = ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
            .await
            .expect("first seed");

        append_closed_turn(session.as_ref(), &main, "turn two", "answer two").await;
        let b = ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
            .await
            .expect("delta seed");

        assert!(b.events_added > 0, "the new turn must be carried");
        assert!(
            b.cursor > a.cursor,
            "carrying a new turn must advance the cursor: {:?} -> {:?}",
            a.cursor,
            b.cursor
        );
        let text = transcript_text(session.as_ref(), &side).await;
        assert_eq!(text.matches("turn one").count(), 1);
        assert_eq!(text.matches("turn two").count(), 1);
    }

    /// Provenance is a claim about how many forks happened, and exactly one
    /// did. Re-marking on every top-up would tell the classification that
    /// reads this marker that the side session was forked N times.
    #[tokio::test]
    async fn the_fork_marker_is_emitted_once_however_many_top_ups_follow() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        for i in 0..3 {
            append_closed_turn(session.as_ref(), &main, &format!("ask {i}"), "answer").await;
            ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
                .await
                .expect("seed");
        }

        let marks = session
            .get_events(&side, None, None)
            .await
            .expect("read side log")
            .iter()
            .filter(|r| matches!(r.event, SessionEvent::SessionForked { .. }))
            .count();
        assert_eq!(marks, 1, "the side session claims {marks} forks");
    }

    /// A turn holding an unresolved tool call is deliberately left behind —
    /// that is `fork`'s definition of open, and it is what the main session
    /// looks like at the instant a side question is asked. The cursor must not
    /// step over it, or half of it would reach the side thread and the other
    /// half never would.
    ///
    /// Note what "open" does **not** mean here: a user message with no answer
    /// yet holds no open call, so it is a complete group and is carried at
    /// once. Its answer arrives in the next delta and appends behind it, which
    /// is why the second half of this test asserts order rather than absence.
    #[tokio::test]
    async fn an_open_trailing_turn_is_carried_whole_once_it_closes() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        append_closed_turn(session.as_ref(), &main, "closed ask", "closed answer").await;

        // The model asked for a tool and no result exists yet.
        let open_turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &main,
                SessionEvent::UserMessage {
                    turn_id: open_turn,
                    content: content("still in flight"),
                    at: 0,
                    synthetic: false,
                    author_user_id: None,
                },
            )
            .await
            .expect("emit");
        session
            .emit_event(
                &main,
                SessionEvent::ToolCallRequested {
                    turn_id: open_turn,
                    call_id: "c-open".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({}),
                    at: 0,
                },
            )
            .await
            .expect("emit");

        let cold = ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
            .await
            .expect("cold seed");
        assert_eq!(
            transcript_text(session.as_ref(), &side)
                .await
                .matches("still in flight")
                .count(),
            0,
            "a turn with an unresolved call must not be carried while it is open"
        );

        session
            .emit_event(
                &main,
                SessionEvent::ToolResult {
                    turn_id: open_turn,
                    call_id: "c-open".to_string(),
                    output: crate::session::events::ToolOutput {
                        value: serde_json::Value::String("tool output".to_string()),
                        metadata: Default::default(),
                    },
                    at: 0,
                },
            )
            .await
            .expect("emit");
        session
            .emit_event(
                &main,
                SessionEvent::AssistantMessage {
                    turn_id: open_turn,
                    content: content("landed at last"),
                    usage: None,
                    at: 0,
                },
            )
            .await
            .expect("emit");

        let b = ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
            .await
            .expect("delta seed");
        assert!(b.events_added > 0, "the turn that closed must be carried");
        assert!(b.cursor > cold.cursor, "the cursor must step past it now");

        let text = transcript_text(session.as_ref(), &side).await;
        for fragment in ["still in flight", "tool output", "landed at last"] {
            assert_eq!(
                text.matches(fragment).count(),
                1,
                "`{fragment}` must arrive exactly once"
            );
        }
        // Carried whole and in order: a tool result that outran its request
        // would be an HTTP 400 on the side agent's first call.
        assert!(
            text.find("still in flight") < text.find("tool output")
                && text.find("tool output") < text.find("landed at last"),
            "the turn arrived out of order: {text}"
        );
    }

    /// The other half of the note above: a user message whose answer has not
    /// landed is carried immediately, and the answer appends behind it rather
    /// than re-carrying the question.
    #[tokio::test]
    async fn an_unanswered_question_is_not_re_carried_when_its_answer_lands() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        let turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &main,
                SessionEvent::UserMessage {
                    turn_id: turn,
                    content: content("asked but unanswered"),
                    at: 0,
                    synthetic: false,
                    author_user_id: None,
                },
            )
            .await
            .expect("emit");

        ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
            .await
            .expect("cold seed");

        session
            .emit_event(
                &main,
                SessionEvent::AssistantMessage {
                    turn_id: turn,
                    content: content("answered later"),
                    usage: None,
                    at: 0,
                },
            )
            .await
            .expect("emit");

        ensure_seeded(session.as_ref(), store.as_ref(), &main, &side, &budget())
            .await
            .expect("delta seed");

        let text = transcript_text(session.as_ref(), &side).await;
        assert_eq!(text.matches("asked but unanswered").count(), 1);
        assert_eq!(text.matches("answered later").count(), 1);
        assert!(text.find("asked but unanswered") < text.find("answered later"));
    }

    /// A stored value this code cannot interpret has not said "no cursor".
    /// Reading it as an absence re-seeds the whole prefix, which is the
    /// doubling this module exists to prevent — so it must be an error.
    #[test]
    fn an_uninterpretable_cursor_refuses_rather_than_reading_as_absent() {
        assert_eq!(
            interpret_cursor(None).expect("absence is not an error"),
            None,
            "a side session with no cursor yet reads as absent"
        );
        assert_eq!(
            interpret_cursor(Some(&serde_json::Value::Null)).expect("null is not an error"),
            None,
            "both stores merge this bag key-by-key, nulls included"
        );
        assert_eq!(
            interpret_cursor(Some(&serde_json::json!(42u64))).expect("a seq reads back"),
            Some(42)
        );
        assert!(
            interpret_cursor(Some(&serde_json::json!("not-a-seq"))).is_err(),
            "a value of the wrong shape must be an error, never `None`"
        );
    }
}
