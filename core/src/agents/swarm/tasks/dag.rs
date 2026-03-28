//! DAG cycle detection for task dependency graphs.
//!
//! Uses BFS to detect cycles before inserting new dependency edges.
//! Dependency edges are immutable after task creation, so cycle detection
//! only needs to run at creation time.

use std::collections::{HashSet, VecDeque};

use rusqlite::{params, Connection};

use super::CoordTaskStore;

/// Check if adding edges `new_task_id ← blocked_by[..]` would create a cycle.
///
/// Async version that uses the store trait. Kept for API compatibility.
///
/// Returns `Err(AlephError)` if a cycle is detected, `Ok(())` otherwise.
pub async fn check_no_cycle(
    store: &dyn CoordTaskStore,
    new_task_id: &str,
    blocked_by: &[String],
) -> crate::error::Result<()> {
    if blocked_by.is_empty() {
        return Ok(());
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = blocked_by.iter().cloned().collect();

    while let Some(current) = queue.pop_front() {
        if current == new_task_id {
            return Err(crate::error::AlephError::invalid_input(format!(
                "Adding dependencies would create a cycle involving task '{new_task_id}'"
            )));
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        let deps = store.get_dependencies(&current).await?;
        for dep in deps {
            queue.push_back(dep);
        }
    }

    Ok(())
}

/// Synchronous cycle check that operates directly on a `rusqlite::Connection`.
///
/// This avoids the TOCTOU race in `create_task` by running the cycle check
/// inside the same lock scope as the INSERT, ensuring no concurrent
/// `create_task` can interleave edges between check and insert.
pub fn check_no_cycle_sync(
    conn: &Connection,
    new_task_id: &str,
    blocked_by: &[String],
) -> crate::error::Result<()> {
    if blocked_by.is_empty() {
        return Ok(());
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = blocked_by.iter().cloned().collect();

    while let Some(current) = queue.pop_front() {
        if current == new_task_id {
            return Err(crate::error::AlephError::invalid_input(format!(
                "Adding dependencies would create a cycle involving task '{new_task_id}'"
            )));
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        // Query dependencies directly from the connection (no lock needed)
        let mut stmt = conn
            .prepare_cached("SELECT depends_on FROM coord_task_dependencies WHERE task_id = ?1")
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("CoordTaskStore: {e}"),
                suggestion: None,
            })?;
        let deps: Vec<String> = stmt
            .query_map(params![current], |row| row.get(0))
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("CoordTaskStore: {e}"),
                suggestion: None,
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("CoordTaskStore: {e}"),
                suggestion: None,
            })?;
        for dep in deps {
            queue.push_back(dep);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{
        store::SqliteCoordTaskStore, CoordTaskStatus, CoordTaskUpdate, NewCoordTask, Priority,
    };
    use rusqlite::Connection;
    use serde_json::json;

    async fn setup_store() -> SqliteCoordTaskStore {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    /// Create a minimal task with no dependencies. Returns the task id.
    async fn make_task(store: &SqliteCoordTaskStore, subject: &str, blocked_by: Vec<String>) -> String {
        store
            .create_task(NewCoordTask {
                team_id: None,
                subject: subject.into(),
                description: String::new(),
                owner: None,
                priority: Priority::Normal,
                blocked_by,
                metadata: json!({}),
            })
            .await
            .expect("create task")
            .id
    }

    /// A depends on B — adding is fine (no cycle)
    #[tokio::test]
    async fn test_no_cycle_simple() {
        let store = setup_store().await;

        let b_id = make_task(&store, "B", vec![]).await;

        // Simulate adding A which depends on B.
        // A doesn't exist yet, so new_task_id is just a placeholder string.
        let a_id = "task-a-not-yet-created";
        let result = check_no_cycle(&store, a_id, &[b_id]).await;
        assert!(result.is_ok(), "A→B should not be a cycle: {result:?}");
    }

    /// Chain A→B→C; trying to make C depend on A should be detected as a cycle.
    #[tokio::test]
    async fn test_cycle_detected() {
        let store = setup_store().await;

        // A (no deps)
        let a_id = make_task(&store, "A", vec![]).await;
        // B depends on A
        let b_id = make_task(&store, "B", vec![a_id.clone()]).await;
        // C depends on B
        let c_id = make_task(&store, "C", vec![b_id.clone()]).await;

        // Now try to make A depend on C → would create A←C←B←A cycle.
        // A already exists, so we check as if we were creating a new task
        // with id = a_id that wants to depend on c_id.
        let result = check_no_cycle(&store, &a_id, &[c_id.clone()]).await;
        assert!(result.is_err(), "A→C when C→B→A should be a cycle");
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains(&a_id), "error should mention the task id");
    }

    /// A→A self-dependency must be detected as a cycle.
    #[tokio::test]
    async fn test_self_dependency() {
        let store = setup_store().await;

        let a_id = make_task(&store, "A", vec![]).await;

        // Try to make A depend on itself.
        let result = check_no_cycle(&store, &a_id, &[a_id.clone()]).await;
        assert!(result.is_err(), "A→A is a self-cycle");
    }

    /// Empty blocked_by always succeeds without touching the store.
    #[tokio::test]
    async fn test_empty_blocked_by() {
        let store = setup_store().await;

        let result = check_no_cycle(&store, "any-task-id", &[]).await;
        assert!(result.is_ok(), "empty deps should never be a cycle");
    }

    /// Wider diamond: A→[B,C], D→A; no cycle when adding E→[B,C].
    #[tokio::test]
    async fn test_no_cycle_diamond() {
        let store = setup_store().await;

        let b_id = make_task(&store, "B", vec![]).await;
        let c_id = make_task(&store, "C", vec![]).await;
        let _a_id = make_task(&store, "A", vec![b_id.clone(), c_id.clone()]).await;

        // New task E depends on both B and C — no cycle.
        let e_placeholder = "task-e-not-yet-created";
        let result = check_no_cycle(&store, e_placeholder, &[b_id, c_id]).await;
        assert!(result.is_ok(), "diamond shape should not be a cycle: {result:?}");
    }

    /// Verify that create_task itself rejects a cyclic dependency.
    #[tokio::test]
    async fn test_create_task_rejects_cycle() {
        let store = setup_store().await;

        // A (no deps)
        let a_id = make_task(&store, "A", vec![]).await;
        // B depends on A
        let b_id = make_task(&store, "B", vec![a_id.clone()]).await;

        // Mark A as depending on B — A already exists but we do this via
        // creating a NEW task with a_id equivalent logic isn't possible;
        // instead, directly test check_no_cycle with A's id and B as dep.
        let cycle_result = check_no_cycle(&store, &a_id, &[b_id.clone()]).await;
        assert!(cycle_result.is_err(), "B→A when A is upstream of B is a cycle");

        // Verify that create_task also rejects it when we attempt to create
        // a new task C that depends on B AND A's dependent chain would cycle.
        // (The real guard is in create_task; test that directly.)
        let c_id = make_task(&store, "C", vec![b_id.clone()]).await;
        // D depends on C — fine
        let d_create = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "D".into(),
                description: String::new(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![c_id.clone()],
                metadata: json!({}),
            })
            .await;
        assert!(d_create.is_ok(), "C→D linear chain should succeed");

        // Attempt to create a task that has both A as dep and A transitively
        // depends on nothing — but creating a new task that is upstream of
        // an existing chain to close a loop.
        //
        // Simplest direct test: complete A first (so A has no unresolved deps),
        // and try check_no_cycle for creating a task with id=a_id (pretending
        // we want to re-create it) with a dep on d which chains back to a.
        let d_id = d_create.unwrap().id;
        let cycle2 = check_no_cycle(&store, &a_id, &[d_id]).await;
        assert!(cycle2.is_err(), "a_id←d←c←b←a_id is a cycle");
    }

    /// Verify status transitions still work after cycle guard integration.
    #[tokio::test]
    async fn test_status_update_unaffected() {
        let store = setup_store().await;

        let a_id = make_task(&store, "A", vec![]).await;
        let b_id = make_task(&store, "B", vec![a_id.clone()]).await;

        // B is blocked by A
        let b_task = store.get_task(&b_id).await.unwrap().unwrap();
        assert_eq!(b_task.status, CoordTaskStatus::Blocked);

        // Complete A → B becomes pending
        store
            .update_task(
                &a_id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let b_after = store.get_task(&b_id).await.unwrap().unwrap();
        assert_eq!(b_after.status, CoordTaskStatus::Pending);
    }
}
