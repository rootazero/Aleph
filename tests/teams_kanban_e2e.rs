//! End-to-end coverage for the Teams kanban backend wiring:
//!   - SqliteCoordTaskStore::with_event_bus emits team.<id>.task.<verb>
//!   - list_tasks returns the derived Blocked status in a single pass
//!   - update_task transitions trigger Blocked → Pending downstream
//!
//! Uses an in-memory SqliteCoordTaskStore wired to a real GatewayEventBus.
//! This stays at the store layer because the JSON-RPC handlers are thin
//! wrappers (verified by cargo check) — exercising them through the gateway
//! requires standing up the full server, which is out of scope here.

use alephcore::agents::swarm::tasks::{
    CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
};
use alephcore::agents::swarm::tasks::store::SqliteCoordTaskStore;
use alephcore::gateway::event_bus::GatewayEventBus;
use rusqlite::Connection;
use std::sync::Arc;

#[tokio::test]
async fn list_tasks_and_update_task_round_trip() {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    let bus = Arc::new(GatewayEventBus::new());
    let mut rx = bus.subscribe();
    let store = Arc::new(SqliteCoordTaskStore::new(conn).with_event_bus(bus.clone()));
    store.migrate().await.expect("migrate");

    // Seed: t1 (no deps) and t2 (depends on t1)
    let t1 = store
        .create_task(NewCoordTask {
            team_id: Some("alpha".into()),
            subject: "first".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    // Drain the 'created' event for t1
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;

    let _t2 = store
        .create_task(NewCoordTask {
            team_id: Some("alpha".into()),
            subject: "second".into(),
            description: "".into(),
            owner: None,
            priority: Priority::High,
            blocked_by: vec![t1.id.clone()],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    // Drain the 'created' event for t2
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;

    // list_tasks should derive Blocked for t2 because t1 is still pending
    let all = store
        .list_tasks(CoordTaskFilter {
            team_id: Some("alpha".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    let second = all.iter().find(|t| t.subject == "second").unwrap();
    assert_eq!(second.status, CoordTaskStatus::Blocked);

    // Update t1 → Completed
    store
        .update_task(
            &t1.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Should observe a 'completed' topic event on the bus
    let payload = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("event in time")
        .expect("payload");
    assert!(
        payload.contains(r#""topic":"team.alpha.task.completed""#),
        "expected completed topic, got: {payload}"
    );

    // Re-list — t2 now derives Pending (no unresolved parents)
    let after = store
        .list_tasks(CoordTaskFilter {
            team_id: Some("alpha".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let second2 = after.iter().find(|t| t.subject == "second").unwrap();
    assert_eq!(second2.status, CoordTaskStatus::Pending);
}
