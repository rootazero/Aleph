//! Integration test: simulate actor/Harness crash, verify wake() recovers state.
//!
//! Phase 1 Task 8 of the managed-agents refactor. Exercises the full public
//! `SessionService` surface end-to-end against the real SQLite-backed store.

use std::sync::Arc;

use alephcore::routing::session_key::SessionKey;
use alephcore::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
use alephcore::session::store::{migrate_add_session_events, SqliteEventStore};
use alephcore::session::{InProcessActorSessionService, SessionEventStore, SessionService};

async fn fresh_service() -> InProcessActorSessionService {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    InProcessActorSessionService::new(store)
}

#[tokio::test]
async fn crash_then_wake_then_continue() {
    let svc = fresh_service().await;
    let id = SessionKey::ephemeral("crash-recover");
    let tid = uuid::Uuid::new_v4();

    // 1. Run a few events normally.
    svc.emit_event(
        &id,
        SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at: now_ms(),
        },
    )
    .await
    .unwrap();
    svc.emit_event(
        &id,
        SessionEvent::UserMessage {
            turn_id: tid,
            content: MessageContent {
                text: "hi".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: false,
            author_user_id: None,
        },
    )
    .await
    .unwrap();

    // 2. Simulate crash: detach forcefully (mimics Harness dropping its sender).
    svc.detach(&id).await.unwrap();

    // 3. Wake should bring the session back — new actor, replay from SQLite.
    let handle = svc.wake(&id).await.unwrap();
    // Two emitted events + SessionWoken marker.
    assert_eq!(handle.head_seq, 3);

    // 4. Post-wake emission continues with correct seq.
    let seq = svc
        .emit_event(
            &id,
            SessionEvent::AssistantMessage {
                turn_id: tid,
                content: MessageContent {
                    text: "hello back".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                usage: None,
                at: now_ms(),
            },
        )
        .await
        .unwrap();
    assert_eq!(seq, 4);

    // 5. get_events returns all 4 in order.
    let events = svc.get_events(&id, None, None).await.unwrap();
    assert_eq!(events.len(), 4);
    let types: Vec<_> = events
        .iter()
        .map(|r| match &r.event {
            SessionEvent::TurnStarted { .. } => "turn_started",
            SessionEvent::UserMessage { .. } => "user_message",
            SessionEvent::SessionWoken { .. } => "session_woken",
            SessionEvent::AssistantMessage { .. } => "assistant_message",
            _ => "other",
        })
        .collect();
    assert_eq!(
        types,
        vec![
            "turn_started",
            "user_message",
            "session_woken",
            "assistant_message"
        ]
    );
}

#[tokio::test]
async fn two_sessions_are_independent() {
    let svc = fresh_service().await;
    let s1 = SessionKey::ephemeral("s1");
    let s2 = SessionKey::ephemeral("s2");
    let tid = uuid::Uuid::new_v4();

    let e = |at: i64| SessionEvent::TurnStarted {
        turn_id: tid,
        trigger: TurnTrigger::UserMessage,
        at,
    };

    svc.emit_event(&s1, e(now_ms())).await.unwrap();
    svc.emit_event(&s1, e(now_ms())).await.unwrap();
    svc.emit_event(&s2, e(now_ms())).await.unwrap();

    let h1 = svc.attach(s1.clone()).await.unwrap();
    let h2 = svc.attach(s2.clone()).await.unwrap();
    assert_eq!(h1.head_seq, 2);
    assert_eq!(h2.head_seq, 1);
}
