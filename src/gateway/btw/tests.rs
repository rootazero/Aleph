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
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::agents::subagent_spawner::fork::ForkBudget;
    use crate::gateway::btw::seed::{ensure_seeded, interpret_cursor, SeedOutcome};
    use crate::gateway::btw::side_key_for;
    use crate::gateway::session_store::SessionStore;
    use crate::session::events::{
        MessageContent, RunOutcome, SessionEvent, ToolOutput, TurnTrigger,
    };
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

    /// No run attribution — equivalent to a single-user install, where
    /// `scope_from_metadata` yields `None` and the row is owner-adopted.
    fn no_attribution() -> HashMap<String, String> {
        HashMap::new()
    }

    fn content(text: &str) -> MessageContent {
        MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }
    }

    async fn emit(session: &dyn SessionService, id: &SessionId, event: SessionEvent) {
        session.emit_event(id, event).await.expect("emit");
    }

    async fn turn_started(session: &dyn SessionService, id: &SessionId) -> uuid::Uuid {
        let turn_id = uuid::Uuid::new_v4();
        emit(
            session,
            id,
            SessionEvent::TurnStarted {
                turn_id,
                trigger: TurnTrigger::UserMessage,
                at: 0,
            },
        )
        .await;
        turn_id
    }

    async fn user(session: &dyn SessionService, id: &SessionId, turn_id: uuid::Uuid, text: &str) {
        emit(
            session,
            id,
            SessionEvent::UserMessage {
                turn_id,
                content: content(text),
                at: 0,
                synthetic: false,
                author_user_id: None,
            },
        )
        .await;
    }

    async fn assistant(
        session: &dyn SessionService,
        id: &SessionId,
        turn_id: uuid::Uuid,
        text: &str,
    ) {
        emit(
            session,
            id,
            SessionEvent::AssistantMessage {
                turn_id,
                content: content(text),
                usage: None,
                at: 0,
            },
        )
        .await;
    }

    async fn tool_call(
        session: &dyn SessionService,
        id: &SessionId,
        turn_id: uuid::Uuid,
        call_id: &str,
    ) {
        emit(
            session,
            id,
            SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call_id.to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
                at: 0,
            },
        )
        .await;
    }

    async fn tool_result(
        session: &dyn SessionService,
        id: &SessionId,
        turn_id: uuid::Uuid,
        call_id: &str,
        body: &str,
    ) {
        emit(
            session,
            id,
            SessionEvent::ToolResult {
                turn_id,
                call_id: call_id.to_string(),
                output: ToolOutput {
                    value: serde_json::Value::String(body.to_string()),
                    metadata: Default::default(),
                },
                at: 0,
            },
        )
        .await;
    }

    /// The marker that ends a run — and therefore, per `TurnStarted`'s own
    /// doc, the marker that proves its last turn is over.
    async fn run_finished(session: &dyn SessionService, id: &SessionId) {
        emit(
            session,
            id,
            SessionEvent::RunFinished {
                run_id: uuid::Uuid::new_v4().to_string(),
                outcome: RunOutcome::Completed,
                at: 0,
            },
        )
        .await;
    }

    /// Append one *settled* turn, framed the way production frames it:
    /// `harness_bridge::session_seed` opens every user turn with
    /// `TurnStarted`, and `harness_bridge::runner_impl` closes every run with
    /// `RunFinished` on the completed, cancelled and errored paths alike.
    /// Seeding cuts on those markers, so a fixture without them would be
    /// asserting against a log shape production never produces.
    async fn append_closed_turn(
        session: &dyn SessionService,
        id: &SessionId,
        ask: &str,
        answer: &str,
    ) {
        let turn_id = turn_started(session, id).await;
        user(session, id, turn_id, ask).await;
        assistant(session, id, turn_id, answer).await;
        run_finished(session, id).await;
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

        let a = ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
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

        // Nothing new settled on the main session in between.
        let b = ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
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
        let a = ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
        .await
        .expect("first seed");

        append_closed_turn(session.as_ref(), &main, "turn two", "answer two").await;
        let b = ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
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
    ///
    /// This pins the **top-up** path. It deliberately does not cover the
    /// window described in the module doc, where a cold seed whose cursor
    /// write is lost re-enters the cold arm and emits a second marker — that
    /// is a real gap, stated there rather than pinned here.
    #[tokio::test]
    async fn the_fork_marker_is_emitted_once_however_many_top_ups_follow() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        for i in 0..3 {
            append_closed_turn(session.as_ref(), &main, &format!("ask {i}"), "answer").await;
            ensure_seeded(
                session.as_ref(),
                store.as_ref(),
                &main,
                &side,
                &no_attribution(),
                &budget(),
            )
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

    /// A turn holding an unresolved tool call is left behind — it is not over,
    /// and neither of the markers that would prove it over has landed.
    #[tokio::test]
    async fn an_open_trailing_turn_is_carried_whole_once_it_closes() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        append_closed_turn(session.as_ref(), &main, "closed ask", "closed answer").await;

        // The model asked for a tool and no result exists yet.
        let open_turn = turn_started(session.as_ref(), &main).await;
        user(session.as_ref(), &main, open_turn, "still in flight").await;
        tool_call(session.as_ref(), &main, open_turn, "c-open").await;

        let cold = ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
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

        tool_result(session.as_ref(), &main, open_turn, "c-open", "tool output").await;
        assistant(session.as_ref(), &main, open_turn, "landed at last").await;
        run_finished(session.as_ref(), &main).await;

        let b = ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
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

    /// The case `fork::plan`'s snapshot predicate gets wrong on its own.
    ///
    /// Between an `AssistantMessage` and the next `ToolCallRequested` a live
    /// multi-step turn has **no outstanding tool call**, so `open_calls
    /// .is_empty()` reads it as closed. Cutting there carries half the turn,
    /// lets the side thread append its own exchange, and carries the other
    /// half next time — one main turn in two places with foreign content
    /// wedged between them. Asking a side question while the main run works is
    /// the premise of the feature, so this is the ordinary path, not an edge.
    #[tokio::test]
    async fn a_multi_step_main_turn_is_never_split_across_two_deltas() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        // A settled turn first, so the side session has a cursor and the
        // second seed exercises the warm arm.
        append_closed_turn(session.as_ref(), &main, "earlier ask", "earlier answer").await;
        ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
        .await
        .expect("cold seed");

        // Main is mid-run, momentarily between steps: it has spoken but not
        // yet reached for its next tool.
        let live = turn_started(session.as_ref(), &main).await;
        user(session.as_ref(), &main, live, "multi step ask").await;
        assistant(session.as_ref(), &main, live, "thinking out loud").await;

        let a = ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
        .await
        .expect("mid-run seed");
        assert_eq!(
            a.events_added, 0,
            "an unfinished main turn must not be carried in halves"
        );

        // The side thread now has its own exchange. Anything carried later
        // lands after this, so a split turn would straddle it.
        let side_turn = turn_started(session.as_ref(), &side).await;
        user(session.as_ref(), &side, side_turn, "SIDE QUESTION").await;
        assistant(session.as_ref(), &side, side_turn, "SIDE ANSWER").await;

        // Main finishes the turn it was in the middle of.
        tool_call(session.as_ref(), &main, live, "c-live").await;
        tool_result(session.as_ref(), &main, live, "c-live", "step output").await;
        assistant(session.as_ref(), &main, live, "final word").await;
        run_finished(session.as_ref(), &main).await;

        let b = ensure_seeded(
            session.as_ref(),
            store.as_ref(),
            &main,
            &side,
            &no_attribution(),
            &budget(),
        )
        .await
        .expect("settled seed");
        assert!(b.events_added > 0, "the settled turn must be carried");

        let text = transcript_text(session.as_ref(), &side).await;
        let side_q = text.find("SIDE QUESTION").expect("side question present");
        for fragment in [
            "multi step ask",
            "thinking out loud",
            "step output",
            "final word",
        ] {
            assert_eq!(
                text.matches(fragment).count(),
                1,
                "`{fragment}` must appear exactly once"
            );
            assert!(
                text.find(fragment).expect("present") > side_q,
                "`{fragment}` landed before the side thread's own exchange — \
                 the main turn was split around it"
            );
        }
    }

    /// Concurrent side questions on one side session must queue, not
    /// interleave. Without the seeding lock every racer reads the same cursor,
    /// copies the same delta, and writes the same cursor back: the delta lands
    /// N times, the resulting state looks healthy, and nothing ever repeats or
    /// self-heals.
    ///
    /// `patch_session`'s own critical section does not cover this — it makes
    /// the *metadata document*'s read-modify-write atomic, not
    /// `ensure_seeded`'s read → copy → write.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_side_questions_do_not_double_the_delta() {
        let session = in_memory_session_service();
        let (store, _tmp) = in_memory_session_store();
        let (main, side) = keys();

        append_closed_turn(
            session.as_ref(),
            &main,
            "contested turn",
            "contested answer",
        )
        .await;

        let racers: Vec<_> = (0..8)
            .map(|_| {
                let session = Arc::clone(&session);
                let store = Arc::clone(&store);
                let main = main.clone();
                let side = side.clone();
                tokio::spawn(async move {
                    ensure_seeded(
                        session.as_ref(),
                        store.as_ref(),
                        &main,
                        &side,
                        &no_attribution(),
                        &budget(),
                    )
                    .await
                    .expect("concurrent seed")
                })
            })
            .collect();

        let mut carried = 0usize;
        for racer in racers {
            let outcome = racer.await.expect("task joined");
            if outcome.events_added > 0 {
                carried += 1;
            }
        }

        let text = transcript_text(session.as_ref(), &side).await;
        assert_eq!(
            text.matches("contested turn").count(),
            1,
            "the delta was copied more than once — concurrent seeds interleaved"
        );
        assert_eq!(carried, 1, "exactly one racer should report having carried");
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
