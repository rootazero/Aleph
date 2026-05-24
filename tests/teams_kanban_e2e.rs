//! End-to-end coverage for the Teams kanban backend wiring:
//!   - SqliteCoordTaskStore::with_event_bus emits team.<id>.task.<verb>
//!   - list_tasks returns the derived Blocked status in a single pass
//!   - update_task transitions trigger Blocked → Pending downstream
//!
//! Uses an in-memory SqliteCoordTaskStore wired to a real GatewayEventBus.
//! This stays at the store layer because the JSON-RPC handlers are thin
//! wrappers (verified by cargo check) — exercising them through the gateway
//! requires standing up the full server, which is out of scope here.

use alephcore::agents::swarm::tasks::store::SqliteCoordTaskStore;
use alephcore::agents::swarm::tasks::{
    CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
    TaskRunStatus,
};
use alephcore::gateway::event_bus::GatewayEventBus;
use rusqlite::Connection;
use std::sync::Arc;

async fn fresh_store() -> Arc<SqliteCoordTaskStore> {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    let store = Arc::new(SqliteCoordTaskStore::new(conn));
    store.migrate().await.expect("migrate");
    store
}

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

// ---------------------------------------------------------------------------
// Phase 1 — terminal-state topic emission
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_and_fail_transitions_emit_distinct_topics() {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    let bus = Arc::new(GatewayEventBus::new());
    let mut rx = bus.subscribe();
    let store = Arc::new(SqliteCoordTaskStore::new(conn).with_event_bus(bus.clone()));
    store.migrate().await.expect("migrate");

    let make = |subject: &str| NewCoordTask {
        team_id: Some("ops".into()),
        subject: subject.into(),
        description: "".into(),
        owner: None,
        priority: Priority::Normal,
        blocked_by: vec![],
        metadata: serde_json::json!({}),
    };

    let cancelled = store.create_task(make("to-cancel")).await.unwrap();
    let failed = store.create_task(make("to-fail")).await.unwrap();

    // Drain the two "created" emissions.
    for _ in 0..2 {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
    }

    // Cancel via update_task
    store
        .update_task(
            &cancelled.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Cancelled),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let p1 = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("cancel event in time")
        .expect("payload");
    assert!(
        p1.contains(r#""topic":"team.ops.task.cancelled""#),
        "expected cancelled topic, got: {p1}"
    );

    // Fail via update_task
    store
        .update_task(
            &failed.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Failed),
                result: Some("explosion".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let p2 = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("fail event in time")
        .expect("payload");
    assert!(
        p2.contains(r#""topic":"team.ops.task.failed""#),
        "expected failed topic, got: {p2}"
    );
}

// ---------------------------------------------------------------------------
// Phase 2 — per-attempt run history
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_history_records_each_attempt_with_outcome_and_summary() {
    let store = fresh_store().await;

    let task = store
        .create_task(NewCoordTask {
            team_id: Some("hist".into()),
            subject: "trace runs".into(),
            description: "".into(),
            owner: Some("agent-a".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    // Attempt 1: failed
    let r1 = store
        .start_task_run(&task.id, "agent-a")
        .await
        .expect("start run 1");
    assert!(!r1.is_empty(), "run id must be non-empty for real store");
    store
        .finish_task_run(
            &r1,
            TaskRunStatus::Failed,
            None,
            Some("backend timeout".into()),
        )
        .await
        .unwrap();

    // Attempt 2: timeout (different agent reassigned)
    let r2 = store.start_task_run(&task.id, "agent-b").await.unwrap();
    store
        .finish_task_run(&r2, TaskRunStatus::Timeout, None, Some("slow tool".into()))
        .await
        .unwrap();

    // Attempt 3: success
    let r3 = store.start_task_run(&task.id, "agent-b").await.unwrap();
    store
        .finish_task_run(
            &r3,
            TaskRunStatus::Completed,
            Some("delivered the report".into()),
            None,
        )
        .await
        .unwrap();

    let runs = store.list_task_runs(&task.id).await.unwrap();
    assert_eq!(runs.len(), 3, "expected 3 attempts");
    assert_eq!(runs[0].status, TaskRunStatus::Failed);
    assert_eq!(runs[0].error.as_deref(), Some("backend timeout"));
    assert!(runs[0].ended_at.is_some());
    assert_eq!(runs[1].status, TaskRunStatus::Timeout);
    assert_eq!(runs[1].agent_id, "agent-b");
    assert_eq!(runs[2].status, TaskRunStatus::Completed);
    assert_eq!(runs[2].summary.as_deref(), Some("delivered the report"));

    // Sanity: a fresh task returns empty without panicking.
    let other = store
        .create_task(NewCoordTask {
            team_id: Some("hist".into()),
            subject: "no runs".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();
    let empty = store.list_task_runs(&other.id).await.unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn finish_task_run_with_empty_id_is_noop_for_optional_callers() {
    let store = fresh_store().await;
    // start_task_run can fail (no FK target), but the default-trait path uses
    // an empty id. finish_task_run must accept it gracefully so dispatcher
    // callers can write `let id = ...unwrap_or_default(); ...finish(&id, ...)`
    // without branching.
    store
        .finish_task_run("", TaskRunStatus::Failed, None, Some("oops".into()))
        .await
        .expect("empty run id should be a no-op");
}

// ---------------------------------------------------------------------------
// Phase 4 — per-task comments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn comments_roundtrip_preserves_order_and_metadata() {
    let store = fresh_store().await;
    let task = store
        .create_task(NewCoordTask {
            team_id: Some("notes".into()),
            subject: "annotate me".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    let c1 = store
        .add_task_comment(&task.id, "worker-1", "Started, checking deps")
        .await
        .unwrap();
    // Force monotonic ordering even at second-granularity timestamps.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let c2 = store
        .add_task_comment(&task.id, "worker-2", "Picked it up, will retry")
        .await
        .unwrap();

    let listed = store.list_task_comments(&task.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, c1.id);
    assert_eq!(listed[0].author, "worker-1");
    assert_eq!(listed[1].id, c2.id);
    assert_eq!(listed[1].body, "Picked it up, will retry");
    assert!(
        listed[0].created_at <= listed[1].created_at,
        "comments must be ordered by created_at ASC"
    );
}

#[tokio::test]
async fn comments_are_scoped_per_task() {
    let store = fresh_store().await;
    let t1 = store
        .create_task(NewCoordTask {
            team_id: Some("scope".into()),
            subject: "task A".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();
    let t2 = store
        .create_task(NewCoordTask {
            team_id: Some("scope".into()),
            subject: "task B".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    store
        .add_task_comment(&t1.id, "panel", "for A only")
        .await
        .unwrap();
    store
        .add_task_comment(&t2.id, "panel", "for B only")
        .await
        .unwrap();

    let a = store.list_task_comments(&t1.id).await.unwrap();
    let b = store.list_task_comments(&t2.id).await.unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].body, "for A only");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].body, "for B only");
}
