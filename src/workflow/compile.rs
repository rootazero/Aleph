//! Compile a [`WorkflowDef`] into runnable `coord_tasks`.
//!
//! This is the whole point of the workflow layer: a declarative template is
//! *materialised* into the existing coordination-task DAG, then executed by
//! the existing [`TeamDispatcher`](crate::teams::dispatcher::TeamDispatcher).
//! No new scheduler, no reasoning — a pure mapping (R10 / R7 safe):
//!
//! - each [`WorkflowStepDef`] → one `coord_task` owned by `step.agent`
//! - `step.depends_on` → `coord_task.blocked_by` (cycle-checked by the store)
//! - tasks are tagged `{"managed_by": "dispatcher"}` so the autonomous loop
//!   picks them up; upstream step outputs flow into each step automatically
//!   via the dispatcher's `build_handoff_context`.
//!
//! Tasks are created in topological order so each `blocked_by` references an
//! already-minted task id.

use serde_json::json;

use crate::agents::swarm::tasks::{
    CoordTaskId, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
};
use crate::error::Result;
use crate::teams::dispatcher::{MANAGED_BY_DISPATCHER, MANAGED_BY_KEY};
use crate::workflow::def::{render_prompt, WorkflowDef};

/// The set of `coord_task` ids minted for one workflow run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedWorkflow {
    /// Task ids in creation (topological) order.
    pub task_ids: Vec<CoordTaskId>,
}

/// Materialise `def` into `coord_tasks` under `team_id`, substituting `input`
/// into each step's prompt. Returns the created task ids in topological order.
///
/// The caller is responsible for ensuring `team_id` refers to a team whose
/// members cover every `step.agent` (create one with `team_create` first).
/// After this returns, signal the dispatcher (or let its fallback tick fire)
/// to begin execution.
pub async fn materialize(
    def: &WorkflowDef,
    input: &str,
    team_id: &str,
    store: &dyn CoordTaskStore,
) -> Result<MaterializedWorkflow> {
    def.validate()?;
    let order = def.topo_order()?;

    // step-local id → freshly-minted coord_task id.
    let mut id_map: std::collections::HashMap<&str, CoordTaskId> =
        std::collections::HashMap::with_capacity(def.steps.len());
    let mut task_ids = Vec::with_capacity(def.steps.len());

    for &idx in &order {
        let step = &def.steps[idx];

        // depends_on resolves to already-created task ids because we iterate
        // in topological order — a dependency is always materialised first.
        // De-duplicate: a step listing the same dependency twice would emit a
        // duplicate `(task_id, depends_on)` edge, which violates the dependency
        // table's PRIMARY KEY and aborts `create_task`. `validate()` permits
        // duplicate `depends_on` (semantically a no-op), so collapse them here.
        let mut blocked_by: Vec<CoordTaskId> = Vec::with_capacity(step.depends_on.len());
        for dep in &step.depends_on {
            let Some(dep_id) = id_map.get(dep.as_str()).cloned() else {
                cancel_partial(store, &task_ids).await;
                return Err(crate::error::AlephError::invalid_input(format!(
                    "internal: dependency '{dep}' of step '{}' not yet materialised",
                    step.id
                )));
            };
            if !blocked_by.contains(&dep_id) {
                blocked_by.push(dep_id);
            }
        }

        let metadata = json!({
            MANAGED_BY_KEY: MANAGED_BY_DISPATCHER,
            "workflow": def.name,
            "workflow_step": step.id,
        });

        let created = match store
            .create_task(NewCoordTask {
                team_id: Some(team_id.to_string()),
                subject: format!("{}:{}", def.name, step.id),
                description: render_prompt(&step.prompt, input),
                owner: Some(step.agent.clone()),
                priority: Priority::Normal,
                blocked_by,
                metadata,
            })
            .await
        {
            Ok(created) => created,
            Err(e) => {
                // A mid-loop failure leaves the steps created so far as live,
                // dispatcher-managed tasks. Cancel them best-effort so a failed
                // run does not execute a half-materialised workflow.
                cancel_partial(store, &task_ids).await;
                return Err(e);
            }
        };

        id_map.insert(step.id.as_str(), created.id.clone());
        task_ids.push(created.id);
    }

    Ok(MaterializedWorkflow { task_ids })
}

/// Best-effort rollback: mark already-created tasks `Cancelled` so a failed
/// partial materialisation leaves no live dispatcher-managed orphans. Errors
/// are swallowed — we are already on an error path and a terminal status is the
/// most we can guarantee without a batch/transaction API on the store.
async fn cancel_partial(store: &dyn CoordTaskStore, ids: &[CoordTaskId]) {
    for id in ids {
        let _ = store
            .update_task(
                id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{store::SqliteCoordTaskStore, CoordTaskStatus};
    use crate::workflow::def::WorkflowStepDef;
    use rusqlite::Connection;

    async fn setup_store() -> SqliteCoordTaskStore {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    fn step(id: &str, agent: &str, deps: &[&str]) -> WorkflowStepDef {
        WorkflowStepDef {
            id: id.into(),
            agent: agent.into(),
            prompt: format!("handle {{input}} for {id}"),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn linear_def() -> WorkflowDef {
        WorkflowDef {
            name: "pipeline".into(),
            description: String::new(),
            steps: vec![
                step("gather", "researcher", &[]),
                step("write", "writer", &["gather"]),
            ],
        }
    }

    #[tokio::test]
    async fn materialize_creates_one_task_per_step() {
        let store = setup_store().await;
        let mat = materialize(&linear_def(), "the topic", "team-1", &store)
            .await
            .expect("materialise");
        assert_eq!(mat.task_ids.len(), 2);
    }

    #[tokio::test]
    async fn materialize_substitutes_input_and_tags_dispatcher() {
        let store = setup_store().await;
        let mat = materialize(&linear_def(), "quantum computing", "team-1", &store)
            .await
            .unwrap();

        let first = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        assert_eq!(first.subject, "pipeline:gather");
        assert_eq!(first.description, "handle quantum computing for gather");
        assert_eq!(first.owner.as_deref(), Some("researcher"));
        assert_eq!(
            first.metadata.get(MANAGED_BY_KEY).and_then(|v| v.as_str()),
            Some(MANAGED_BY_DISPATCHER)
        );
        assert_eq!(
            first.metadata.get("workflow_step").and_then(|v| v.as_str()),
            Some("gather")
        );
    }

    #[tokio::test]
    async fn materialize_wires_dependency_so_dependent_is_blocked() {
        let store = setup_store().await;
        let mat = materialize(&linear_def(), "x", "team-1", &store)
            .await
            .unwrap();

        // task_ids[0] is "gather" (root), [1] is "write" (depends on gather).
        let root = store.get_task(&mat.task_ids[0]).await.unwrap().unwrap();
        let dependent = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert_eq!(root.status, CoordTaskStatus::Pending, "root has no deps");
        assert_eq!(
            dependent.status,
            CoordTaskStatus::Blocked,
            "dependent waits on gather"
        );

        // Completing the root unblocks the dependent.
        store
            .update_task(
                &mat.task_ids[0],
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let after = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Pending);
    }

    #[tokio::test]
    async fn materialize_collapses_duplicate_dependency() {
        // A step listing the same dependency twice must NOT emit a duplicate
        // (task_id, depends_on) edge — that would hit the dependency table's
        // PRIMARY KEY and abort materialisation. `validate()` allows the
        // duplicate (it is semantically a no-op), so the compiler collapses it.
        let store = setup_store().await;
        let def = WorkflowDef {
            name: "dup".into(),
            description: String::new(),
            steps: vec![
                step("a", "w", &[]),
                step("b", "w", &["a", "a"]),
            ],
        };
        let mat = materialize(&def, "x", "t", &store)
            .await
            .expect("duplicate dep collapses instead of aborting");
        assert_eq!(mat.task_ids.len(), 2);
        let dependent = store.get_task(&mat.task_ids[1]).await.unwrap().unwrap();
        assert_eq!(dependent.subject, "dup:b");
        assert_eq!(dependent.status, CoordTaskStatus::Blocked);
    }

    #[tokio::test]
    async fn materialize_rejects_invalid_def() {
        let store = setup_store().await;
        let mut def = linear_def();
        def.steps[1].depends_on = vec!["ghost".into()];
        assert!(materialize(&def, "x", "team-1", &store).await.is_err());
    }

    #[tokio::test]
    async fn materialize_diamond_orders_dependencies_first() {
        let store = setup_store().await;
        let def = WorkflowDef {
            name: "diamond".into(),
            description: String::new(),
            steps: vec![
                step("a", "w", &[]),
                step("b", "w", &["a"]),
                step("c", "w", &["a"]),
                step("d", "w", &["b", "c"]),
            ],
        };
        let mat = materialize(&def, "x", "t", &store).await.unwrap();
        assert_eq!(mat.task_ids.len(), 4);
        // The final task "d" must be blocked until both b and c complete.
        let last = store
            .get_task(mat.task_ids.last().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(last.subject, "diamond:d");
        assert_eq!(last.status, CoordTaskStatus::Blocked);
    }
}
