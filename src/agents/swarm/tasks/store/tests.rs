use super::helpers::now_epoch;
use super::*;
use crate::agents::swarm::tasks::{CoordTaskStatus, Priority};
use rusqlite::params;
use serde_json::json;

async fn setup_store() -> SqliteCoordTaskStore {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    let store = SqliteCoordTaskStore::new(conn);
    store.migrate().await.expect("migrate");
    store
}

#[tokio::test]
async fn test_create_and_get_task() {
    let store = setup_store().await;

    let task = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "Write tests".into(),
            description: "Unit tests for store".into(),
            owner: Some("agent-1".into()),
            priority: Priority::High,
            blocked_by: vec![],
            metadata: json!({"tag": "test"}),
        })
        .await
        .unwrap();

    assert_eq!(task.subject, "Write tests");
    assert_eq!(task.description, "Unit tests for store");
    assert_eq!(task.owner.as_deref(), Some("agent-1"));
    assert_eq!(task.priority, Priority::High);
    assert_eq!(task.status, CoordTaskStatus::Pending);
    assert_eq!(task.metadata["tag"], "test");
    assert!(task.dependencies.is_empty());
    assert!(task.started_at.is_none());
    assert!(task.completed_at.is_none());

    // Get by id
    let fetched = store.get_task(&task.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, task.id);
    assert_eq!(fetched.subject, "Write tests");
}

#[tokio::test]
async fn test_list_tasks_with_filter() {
    let store = setup_store().await;

    let _t1 = store
        .create_task(NewCoordTask {
            team_id: Some("team-1".into()),
            subject: "Task A".into(),
            description: "".into(),
            owner: Some("agent-1".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    let _t2 = store
        .create_task(NewCoordTask {
            team_id: Some("team-1".into()),
            subject: "Task B".into(),
            description: "".into(),
            owner: Some("agent-2".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    let _t3 = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "Task C".into(),
            description: "".into(),
            owner: Some("agent-1".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    // Filter by team
    let by_team = store
        .list_tasks(CoordTaskFilter {
            team_id: Some("team-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(by_team.len(), 2);

    // (Owner filtering was removed from `CoordTaskFilter` in 503bd9993 — the
    // field had no production consumer. `CoordTask.owner` itself still exists
    // and is asserted elsewhere; only the filter arm is gone.)

    // No filter
    let all = store.list_tasks(CoordTaskFilter::default()).await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn test_derived_blocked_status() {
    let store = setup_store().await;

    // Create A (no deps) — should be Pending
    let a = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "Task A".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();
    assert_eq!(a.status, CoordTaskStatus::Pending);

    // Create B (blocked by A) — should be Blocked
    let b = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "Task B".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![a.id.clone()],
            metadata: json!({}),
        })
        .await
        .unwrap();
    assert_eq!(b.status, CoordTaskStatus::Blocked);
    assert_eq!(b.dependencies, vec![a.id.clone()]);

    // Complete A
    store
        .update_task(
            &a.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Now B should be Pending (all deps completed)
    let b2 = store.get_task(&b.id).await.unwrap().unwrap();
    assert_eq!(b2.status, CoordTaskStatus::Pending);
}

#[tokio::test]
async fn test_get_newly_unblocked() {
    let store = setup_store().await;

    // Chain: A → B → C
    let a = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "A".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    let b = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "B".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![a.id.clone()],
            metadata: json!({}),
        })
        .await
        .unwrap();

    let c = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "C".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![b.id.clone()],
            metadata: json!({}),
        })
        .await
        .unwrap();

    // Complete A → B should become unblocked
    store
        .update_task(
            &a.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let unblocked = store.get_newly_unblocked(&a.id).await.unwrap();
    assert_eq!(unblocked.len(), 1);
    assert_eq!(unblocked[0].id, b.id);

    // C should NOT be unblocked yet (B is still pending)
    let unblocked_c = store.get_newly_unblocked(&b.id).await.unwrap();
    assert!(unblocked_c.is_empty());

    // Complete B → C should become unblocked
    store
        .update_task(
            &b.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let unblocked2 = store.get_newly_unblocked(&b.id).await.unwrap();
    assert_eq!(unblocked2.len(), 1);
    assert_eq!(unblocked2[0].id, c.id);
}

#[tokio::test]
async fn test_acquire_and_release_lock() {
    let store = setup_store().await;

    let task = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "Lockable".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    // Acquire lock
    store.acquire_lock(&task.id, "agent-1").await.unwrap();
    let t = store.get_task(&task.id).await.unwrap().unwrap();
    assert_eq!(t.locked_by.as_deref(), Some("agent-1"));
    assert!(t.locked_at.is_some());

    // Re-acquire by same agent (idempotent)
    store.acquire_lock(&task.id, "agent-1").await.unwrap();

    // Fail by different agent
    let err = store.acquire_lock(&task.id, "agent-2").await;
    assert!(err.is_err());

    // Release
    store.release_lock(&task.id, "agent-1").await.unwrap();
    let t2 = store.get_task(&task.id).await.unwrap().unwrap();
    assert!(t2.locked_by.is_none());
    assert!(t2.locked_at.is_none());

    // Now different agent can acquire
    store.acquire_lock(&task.id, "agent-2").await.unwrap();
    let t3 = store.get_task(&task.id).await.unwrap().unwrap();
    assert_eq!(t3.locked_by.as_deref(), Some("agent-2"));
}

#[tokio::test]
async fn test_release_stale_locks() {
    let store = setup_store().await;

    let task = store
        .create_task(NewCoordTask {
            team_id: None,
            subject: "Stale lock".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    // Acquire lock
    store.acquire_lock(&task.id, "agent-1").await.unwrap();

    // Manually backdate locked_at to 1 hour ago
    {
        let conn = store.conn.lock().await;
        let old_time = now_epoch() - 3600;
        conn.execute(
            "UPDATE coord_tasks SET locked_at = ?1 WHERE id = ?2",
            params![old_time, task.id],
        )
        .unwrap();
    }

    // Release stale locks with 30 min max age
    let released = store.release_stale_locks(1800).await.unwrap();
    assert_eq!(released, 1);

    // Verify lock is released
    let t = store.get_task(&task.id).await.unwrap().unwrap();
    assert!(t.locked_by.is_none());
    assert!(t.locked_at.is_none());
}

#[tokio::test]
async fn with_event_bus_attaches_bus_without_breaking_existing_methods() {
    use crate::gateway::event_bus::GatewayEventBus;
    let conn = Connection::open_in_memory().expect("open in-memory db");
    let bus = std::sync::Arc::new(GatewayEventBus::new());
    let store = SqliteCoordTaskStore::new(conn).with_event_bus(bus);
    store.migrate().await.expect("migrate");

    // Existing CRUD still works after bus injection
    let task = store
        .create_task(NewCoordTask {
            team_id: Some("team-X".into()),
            subject: "ping".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();
    assert_eq!(task.team_id.as_deref(), Some("team-X"));
}

#[tokio::test]
async fn create_and_update_emit_team_task_topics() {
    use crate::gateway::event_bus::GatewayEventBus;
    let conn = Connection::open_in_memory().expect("open in-memory db");
    let bus = std::sync::Arc::new(GatewayEventBus::new());
    let mut rx = bus.subscribe();
    let store = SqliteCoordTaskStore::new(conn).with_event_bus(bus);
    store.migrate().await.expect("migrate");

    let task = store
        .create_task(NewCoordTask {
            team_id: Some("team-T".into()),
            subject: "do".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
        .await
        .expect("event received in time")
        .expect("event payload");
    assert!(
        evt.contains(r#""topic":"team.team-T.task.created""#),
        "got: {evt}"
    );
    assert!(
        evt.contains(&task.id),
        "topic payload missing task id: {evt}"
    );

    let _ = store
        .update_task(
            &task.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::InProgress),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let evt2 = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
        .await
        .expect("update event received in time")
        .expect("update event payload");
    assert!(
        evt2.contains(r#""topic":"team.team-T.task.updated""#),
        "got: {evt2}"
    );
}

#[tokio::test]
async fn list_tasks_derives_blocked_in_a_single_pass() {
    let store = setup_store().await;

    let a = store
        .create_task(NewCoordTask {
            team_id: Some("T".into()),
            subject: "A".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();
    let b = store
        .create_task(NewCoordTask {
            team_id: Some("T".into()),
            subject: "B".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![a.id.clone()],
            metadata: json!({}),
        })
        .await
        .unwrap();
    let c = store
        .create_task(NewCoordTask {
            team_id: Some("T".into()),
            subject: "C".into(),
            description: "".into(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: vec![b.id.clone()],
            metadata: json!({}),
        })
        .await
        .unwrap();

    let all = store
        .list_tasks(CoordTaskFilter {
            team_id: Some("T".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let by_id: std::collections::HashMap<_, _> = all.iter().map(|t| (t.id.clone(), t)).collect();
    assert_eq!(by_id[&a.id].status, CoordTaskStatus::Pending);
    assert_eq!(by_id[&b.id].status, CoordTaskStatus::Blocked);
    assert_eq!(by_id[&c.id].status, CoordTaskStatus::Blocked);
}

#[tokio::test]
async fn create_task_broadcasts_team_task_assigned_and_updated() {
    // GlobalBus is a singleton; filter by the unique team_id this test owns
    // so we don't collide with sibling tests in the same binary.
    let team_id = format!("team-broadcast-{}", uuid::Uuid::new_v4());
    let store = setup_store().await;
    let mut rx = crate::event::GlobalBus::global().subscribe_broadcast();

    let task = store
        .create_task(NewCoordTask {
            team_id: Some(team_id.clone()),
            subject: "wired".into(),
            description: "".into(),
            owner: Some("agent-x".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    // Drain receiver looking for our 2 expected events. Cap by elapsed
    // time so the test exits even if GlobalBus is heavily contested.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let mut saw_assigned = false;
    let mut saw_updated = false;
    while std::time::Instant::now() < deadline && !(saw_assigned && saw_updated) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(global_evt)) => match &global_evt.event {
                crate::event::AlephEvent::TeamTaskAssigned {
                    team_id: tid,
                    task_id,
                    assignee_id,
                } if tid == &team_id && task_id == &task.id => {
                    assert_eq!(assignee_id, "agent-x");
                    saw_assigned = true;
                }
                crate::event::AlephEvent::TeamTaskUpdated {
                    team_id: tid,
                    task_id,
                    status,
                    ..
                } if tid == &team_id && task_id == &task.id => {
                    assert_eq!(status, "pending");
                    saw_updated = true;
                }
                _ => {}
            },
            _ => break,
        }
    }
    assert!(saw_assigned, "TeamTaskAssigned not broadcast");
    assert!(saw_updated, "TeamTaskUpdated not broadcast");
}

#[tokio::test]
async fn update_task_completed_broadcasts_team_task_completed_with_summary() {
    let team_id = format!("team-complete-{}", uuid::Uuid::new_v4());
    let store = setup_store().await;
    let task = store
        .create_task(NewCoordTask {
            team_id: Some(team_id.clone()),
            subject: "wire-complete".into(),
            description: "".into(),
            owner: Some("agent-y".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: json!({}),
        })
        .await
        .unwrap();

    let mut rx = crate::event::GlobalBus::global().subscribe_broadcast();
    let summary_text = "the agent's last reply";
    store
        .update_task(
            &task.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                result: Some(summary_text.to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let mut got = false;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(global_evt)) => {
                if let crate::event::AlephEvent::TeamTaskCompleted {
                    team_id: tid,
                    task_id,
                    result_summary,
                } = &global_evt.event
                {
                    if tid == &team_id && task_id == &task.id {
                        assert_eq!(result_summary.as_deref(), Some(summary_text));
                        got = true;
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    assert!(got, "TeamTaskCompleted not broadcast");
}

#[tokio::test]
async fn delete_team_tasks_removes_tasks_and_children() {
    let store = setup_store().await;
    let t = store
        .create_task(NewCoordTask {
            team_id: Some("team-A".into()),
            subject: "s".into(),
            description: String::new(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: Vec::new(),
            metadata: json!({}),
        })
        .await
        .unwrap();
    // Add an observable child row (comment)
    store
        .add_task_comment(&t.id, "author", "body")
        .await
        .unwrap();

    let n = store.delete_team_tasks("team-A").await.unwrap();
    assert_eq!(n, 1);
    assert!(
        store.get_task(&t.id).await.unwrap().is_none(),
        "task should be gone"
    );
    assert!(
        store.list_task_comments(&t.id).await.unwrap().is_empty(),
        "child comments should be deleted"
    );
}

#[tokio::test]
async fn list_tasks_orders_by_priority_then_created() {
    let store = setup_store().await;

    // Insert in a deliberately scrambled order so created_at cannot
    // accidentally produce the expected sequence on its own.
    let mk = |subject: &'static str, priority: Priority| NewCoordTask {
        team_id: Some("T".into()),
        subject: subject.into(),
        description: "".into(),
        owner: None,
        priority,
        blocked_by: vec![],
        metadata: json!({}),
    };
    let low = store.create_task(mk("low", Priority::Low)).await.unwrap();
    let critical = store
        .create_task(mk("critical", Priority::Critical))
        .await
        .unwrap();
    let normal_old = store
        .create_task(mk("normal-old", Priority::Normal))
        .await
        .unwrap();
    let high = store.create_task(mk("high", Priority::High)).await.unwrap();
    let normal_new = store
        .create_task(mk("normal-new", Priority::Normal))
        .await
        .unwrap();

    // `created_at` is second-granularity, so all five inserts land in the
    // same second and the same-priority tiebreak would be undefined.
    // Backdate normal_old so the `created_at ASC` tiebreak is deterministic.
    {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE coord_tasks SET created_at = created_at - 100 WHERE id = ?1",
            params![normal_old.id],
        )
        .unwrap();
    }

    let ordered = store
        .list_tasks(CoordTaskFilter {
            team_id: Some("T".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let ids: Vec<&str> = ordered.iter().map(|t| t.id.as_str()).collect();
    // critical → high → normal (oldest-first within tie) → low
    assert_eq!(
        ids,
        vec![
            critical.id.as_str(),
            high.id.as_str(),
            normal_old.id.as_str(),
            normal_new.id.as_str(),
            low.id.as_str(),
        ]
    );
}

#[tokio::test]
async fn delete_team_tasks_cascades_all_child_tables() {
    let store = setup_store().await;

    // Team A: two tasks with full child row coverage on one of them.
    let t_a1 = store
        .create_task(NewCoordTask {
            team_id: Some("team-A".into()),
            subject: "A1".into(),
            description: String::new(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: Vec::new(),
            metadata: json!({}),
        })
        .await
        .unwrap();
    let t_a2 = store
        .create_task(NewCoordTask {
            team_id: Some("team-A".into()),
            subject: "A2".into(),
            description: String::new(),
            owner: None,
            priority: Priority::Normal,
            // t_a2 depends on t_a1, so both sides of the dep live in team A.
            blocked_by: vec![t_a1.id.clone()],
            metadata: json!({}),
        })
        .await
        .unwrap();

    // Per-attempt run on t_a1.
    let run_id = store.start_task_run(&t_a1.id, "worker-A").await.unwrap();
    store
        .finish_task_run(
            &run_id,
            crate::agents::swarm::tasks::TaskRunStatus::Completed,
            Some("done".into()),
            None,
        )
        .await
        .unwrap();

    // Comment on t_a1.
    store
        .add_task_comment(&t_a1.id, "author", "body")
        .await
        .unwrap();

    // Exit journal on t_a1.
    store
        .upsert_task_journal(crate::agents::swarm::tasks::NewTaskExitJournal {
            task_id: t_a1.id.clone(),
            agent_id: "worker-A".into(),
            summary: "exit".into(),
            decisions: vec!["d1".into()],
            artifacts_ref: vec!["a1".into()],
            next_steps: vec!["n1".into()],
            confidence: Some(80),
        })
        .await
        .unwrap();

    // Team B: a sibling task that must survive team A's delete.
    let t_b = store
        .create_task(NewCoordTask {
            team_id: Some("team-B".into()),
            subject: "B".into(),
            description: String::new(),
            owner: None,
            priority: Priority::Normal,
            blocked_by: Vec::new(),
            metadata: json!({}),
        })
        .await
        .unwrap();
    store
        .add_task_comment(&t_b.id, "author", "untouched")
        .await
        .unwrap();

    // Pre-conditions: every child table has rows for t_a1 and team B is intact.
    assert_eq!(store.list_task_comments(&t_a1.id).await.unwrap().len(), 1);
    assert_eq!(store.list_task_runs(&t_a1.id).await.unwrap().len(), 1);
    assert!(store.get_task_journal(&t_a1.id).await.unwrap().is_some());
    assert!(store
        .get_dependencies(&t_a2.id)
        .await
        .unwrap()
        .contains(&t_a1.id));

    // Delete team A — single statement should cascade every child table.
    let n = store.delete_team_tasks("team-A").await.unwrap();
    assert_eq!(n, 2, "return count = parent rows deleted, not children");

    assert!(store.get_task(&t_a1.id).await.unwrap().is_none());
    assert!(store.get_task(&t_a2.id).await.unwrap().is_none());
    assert!(
        store.list_task_comments(&t_a1.id).await.unwrap().is_empty(),
        "comments must cascade"
    );
    assert!(
        store.list_task_runs(&t_a1.id).await.unwrap().is_empty(),
        "runs must cascade"
    );
    assert!(
        store.get_task_journal(&t_a1.id).await.unwrap().is_none(),
        "journal must cascade"
    );
    assert!(
        store.get_dependencies(&t_a2.id).await.unwrap().is_empty(),
        "dependencies must cascade on both task_id and depends_on"
    );

    // Team B untouched.
    assert!(store.get_task(&t_b.id).await.unwrap().is_some());
    assert_eq!(store.list_task_comments(&t_b.id).await.unwrap().len(), 1);
}

/// Tolerant fan-in: a task stamped `tolerate_failed_deps` stops waiting on a
/// dependency that ended `failed` — it is Pending (ready), never
/// `Unsatisfiable` — while an unstamped sibling on the same edge stays
/// `Unsatisfiable`. Both read paths must agree: `get_task` derives per row
/// (`row_decode::derive_status`) and `list_tasks` derives from a counting join,
/// two hand-copied expressions of one rule.
#[tokio::test]
async fn tolerant_dependent_of_a_failed_dep_is_ready_on_both_read_paths() {
    let store = setup_store().await;
    let new = |subject: &str, deps: Vec<String>, meta: serde_json::Value| NewCoordTask {
        team_id: Some("team-tol".into()),
        subject: subject.into(),
        description: String::new(),
        owner: Some("agent-1".into()),
        priority: Priority::Normal,
        blocked_by: deps,
        metadata: meta,
    };

    let upstream = store
        .create_task(new("upstream", vec![], json!({})))
        .await
        .unwrap();
    let tolerant = store
        .create_task(new(
            "synthesis",
            vec![upstream.id.clone()],
            json!({ "tolerate_failed_deps": true }),
        ))
        .await
        .unwrap();
    let strict = store
        .create_task(new("strict", vec![upstream.id.clone()], json!({})))
        .await
        .unwrap();

    // While the upstream is merely pending, BOTH dependents are Blocked —
    // tolerance is about dead deps, not about skipping the wait.
    assert_eq!(
        store.get_task(&tolerant.id).await.unwrap().unwrap().status,
        CoordTaskStatus::Blocked,
        "a live upstream still blocks a tolerant task"
    );

    store
        .update_task(
            &upstream.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Failed),
                result: Some("boom".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        store.get_task(&tolerant.id).await.unwrap().unwrap().status,
        CoordTaskStatus::Pending,
        "get_task: a tolerant dependent runs after its dep failed"
    );
    assert_eq!(
        store.get_task(&strict.id).await.unwrap().unwrap().status,
        CoordTaskStatus::Unsatisfiable,
        "get_task: an unstamped dependent is still structurally dead"
    );

    let listed = store
        .list_tasks(CoordTaskFilter {
            team_id: Some("team-tol".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let status_of = |id: &str| listed.iter().find(|t| t.id == id).unwrap().status;
    assert_eq!(
        status_of(&tolerant.id),
        CoordTaskStatus::Pending,
        "list_tasks agrees with get_task — the dispatcher only ever sees this path"
    );
    assert_eq!(status_of(&strict.id), CoordTaskStatus::Unsatisfiable);
}

/// Tolerance is per-dependency-state, not per-task: one failed dep and one
/// still-running dep leaves a tolerant task `Blocked`, because the live one may
/// still deliver. (The bug this pins would be reading the flag as "ignore all
/// deps".)
#[tokio::test]
async fn tolerant_task_with_a_live_dep_left_is_still_blocked() {
    let store = setup_store().await;
    let new = |subject: &str, deps: Vec<String>, meta: serde_json::Value| NewCoordTask {
        team_id: Some("team-tol2".into()),
        subject: subject.into(),
        description: String::new(),
        owner: Some("agent-1".into()),
        priority: Priority::Normal,
        blocked_by: deps,
        metadata: meta,
    };
    let dead = store
        .create_task(new("dead", vec![], json!({})))
        .await
        .unwrap();
    let live = store
        .create_task(new("live", vec![], json!({})))
        .await
        .unwrap();
    let tolerant = store
        .create_task(new(
            "synthesis",
            vec![dead.id.clone(), live.id.clone()],
            json!({ "tolerate_failed_deps": true }),
        ))
        .await
        .unwrap();

    store
        .update_task(
            &dead.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Failed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        store.get_task(&tolerant.id).await.unwrap().unwrap().status,
        CoordTaskStatus::Blocked,
        "one dep is dead, but the other can still arrive"
    );
    let listed = store
        .list_tasks(CoordTaskFilter {
            team_id: Some("team-tol2".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        listed.iter().find(|t| t.id == tolerant.id).unwrap().status,
        CoordTaskStatus::Blocked
    );

    // The live dep completing is what releases it.
    store
        .update_task(
            &live.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_task(&tolerant.id).await.unwrap().unwrap().status,
        CoordTaskStatus::Pending
    );
}

/// `get_newly_unblocked` answers for the FAILURE transition too: a tolerant
/// dependent whose last blocking dep just failed is now runnable, and a caller
/// asking "what did this transition release?" must be told. The strict
/// dependent on the same edge is not released.
#[tokio::test]
async fn get_newly_unblocked_reports_a_tolerant_dependent_when_its_dep_fails() {
    let store = setup_store().await;
    let new = |subject: &str, deps: Vec<String>, meta: serde_json::Value| NewCoordTask {
        team_id: Some("team-tol3".into()),
        subject: subject.into(),
        description: String::new(),
        owner: Some("agent-1".into()),
        priority: Priority::Normal,
        blocked_by: deps,
        metadata: meta,
    };
    let upstream = store
        .create_task(new("upstream", vec![], json!({})))
        .await
        .unwrap();
    let tolerant = store
        .create_task(new(
            "synthesis",
            vec![upstream.id.clone()],
            json!({ "tolerate_failed_deps": true }),
        ))
        .await
        .unwrap();
    let _strict = store
        .create_task(new("strict", vec![upstream.id.clone()], json!({})))
        .await
        .unwrap();

    assert!(
        store
            .get_newly_unblocked(&upstream.id)
            .await
            .unwrap()
            .is_empty(),
        "nothing is released while the upstream is still pending"
    );

    store
        .update_task(
            &upstream.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Failed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let released = store.get_newly_unblocked(&upstream.id).await.unwrap();
    let ids: Vec<&str> = released.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![tolerant.id.as_str()],
        "only the tolerant dependent is released by a failure"
    );
    assert_eq!(released[0].status, CoordTaskStatus::Pending);
}
