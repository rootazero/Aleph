//! Dual-write consistency: SessionManager writes produce matching session_events.
//!
//! Phase 1 Task 9 acceptance: when a SessionManager is wired with a
//! SessionService via `with_session_service`, every `add_message` into the
//! legacy `messages` table must also land in the new `session_events` log.
//! Read paths on SessionManager are unchanged.

use std::sync::Arc;

use alephcore::gateway::router::SessionKey;
use alephcore::gateway::{SessionManager, SessionManagerConfig};
use alephcore::session::events::SessionEvent;
use alephcore::session::in_process::InProcessActorSessionService;
use alephcore::session::service::SessionService;
use alephcore::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

/// Build a `SessionManager` on a temp-file SQLite DB and a `SessionService`
/// on a **dedicated** connection to the same DB. Mirrors the production
/// wiring in `src/bin/aleph-server/commands/start/mod.rs` (Phase 1 Task 9).
fn make_pair() -> (SessionManager, Arc<dyn SessionService>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("sessions.db");

    // SessionManager (legacy messages table).
    let sm = SessionManager::new(SessionManagerConfig {
        db_path: db_path.clone(),
        ..Default::default()
    })
    .expect("SessionManager");

    // Dedicated SessionService connection to the same file, migrated for
    // session_events.
    let conn = rusqlite::Connection::open(&db_path).expect("open conn");
    migrate_add_session_events(&conn).expect("migrate");
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let svc: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));

    let sm = sm.with_session_service(svc.clone());
    (sm, svc, tmp)
}

#[tokio::test]
async fn five_sessionmanager_messages_produce_five_events() {
    let (sm, svc, _tmp) = make_pair();
    let key = SessionKey::ephemeral("dual-write-five");

    sm.get_or_create(&key).await.unwrap();
    for i in 0..5 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        sm.add_message(&key, role, &format!("msg-{i}"))
            .await
            .unwrap();
    }

    let events = svc.get_events(&key, None, None).await.unwrap();
    assert_eq!(
        events.len(),
        5,
        "session_events must receive one record per add_message"
    );

    // Ordering and role mapping match the call sequence.
    for (i, record) in events.iter().enumerate() {
        if i % 2 == 0 {
            assert!(
                matches!(record.event, SessionEvent::UserMessage { .. }),
                "event {i} should be UserMessage, got {:?}",
                record.event
            );
        } else {
            assert!(
                matches!(record.event, SessionEvent::AssistantMessage { .. }),
                "event {i} should be AssistantMessage, got {:?}",
                record.event
            );
        }
    }
}

#[tokio::test]
async fn unknown_role_is_skipped_not_errored() {
    let (sm, svc, _tmp) = make_pair();
    let key = SessionKey::ephemeral("dual-write-unknown");
    sm.get_or_create(&key).await.unwrap();

    // "observer" is not one of {user,assistant,system,tool} — the legacy
    // messages row still lands, but the mirror skips it.
    sm.add_message(&key, "observer", "side-channel")
        .await
        .unwrap();
    sm.add_message(&key, "user", "regular").await.unwrap();

    let events = svc.get_events(&key, None, None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].event, SessionEvent::UserMessage { .. }));
}

#[tokio::test]
async fn no_service_wired_is_a_no_op() {
    // Sanity: the shim stays dormant without a service — no panic, no err.
    let tmp = tempfile::tempdir().unwrap();
    let sm = SessionManager::new(SessionManagerConfig {
        db_path: tmp.path().join("sessions.db"),
        ..Default::default()
    })
    .unwrap();

    let key = SessionKey::ephemeral("dual-write-no-svc");
    sm.get_or_create(&key).await.unwrap();
    sm.add_message(&key, "user", "hi").await.unwrap();
    // If we reach this line without panicking, the shim correctly stayed dormant.
}
