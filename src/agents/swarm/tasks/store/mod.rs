//! SQLite-backed implementation of [`CoordTaskStore`].
//!
//! Uses `Arc<tokio::sync::Mutex<rusqlite::Connection>>` for thread-safe
//! async access. The `Blocked` status is never stored — it is derived at
//! query time from unresolved dependency edges.
//!
//! ## Layout
//! - [`helpers`] — `now_epoch`, `db_err`, `summarize`
//! - [`row_decode`] — `read_task_row`, `load_dependencies`, `derive_status`, `load_task`
//! - [`schema`] — DDL migration (sync, runs under the mutex lock)
//! - [`crud`] — create / get / update / list / delete task rows
//! - [`deps`] — dependency edges and newly-unblocked queries
//! - [`locks`] — task lock acquire / release / stale-release
//! - [`runs`] — task-run lifecycle and run reviews
//! - [`comments`] — task comment append / list
//! - [`journal`] — per-task and per-team journals
//! - this module — `SqliteCoordTaskStore` + the thin `CoordTaskStore` trait
//!   impl that delegates to the topic submodules above

mod comments;
mod crud;
mod deps;
mod helpers;
mod journal;
mod locks;
mod row_decode;
mod runs;
mod schema;

#[cfg(test)]
mod tests;

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use rusqlite::Connection;
use tokio::sync::Mutex;

use super::{
    CoordTask, CoordTaskComment, CoordTaskFilter, CoordTaskId, CoordTaskRun, CoordTaskStore,
    CoordTaskUpdate, NewCoordTask, ReviewVerdict, ReviewerKind, TaskRunStatus,
};

use helpers::{now_epoch, summarize};

// ---------------------------------------------------------------------------
// SqliteCoordTaskStore
// ---------------------------------------------------------------------------

pub struct SqliteCoordTaskStore {
    conn: Arc<Mutex<Connection>>,
    bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

impl SqliteCoordTaskStore {
    /// Create a new store wrapping the given connection.
    /// Call [`migrate`] before using the store.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            bus: None,
        }
    }

    /// Attach an event bus so the store emits topic events on mutations.
    /// Builder is no-op safe: stores constructed without a bus simply skip emission.
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<crate::gateway::event_bus::GatewayEventBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Hand out a clone of the inner connection handle so a sibling store
    /// living in the same database file (currently
    /// [`crate::teams::snapshots::SqliteSnapshotStore`]) can share the lock
    /// and avoid the `SQLite` "database is locked" hazard that would arise
    /// from two independent connections to the same file.
    #[must_use]
    pub fn connection_handle(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// Publish a `team.<team_id>.task.<verb>` topic AND broadcast the matching
    /// [`AlephEvent`] on [`GlobalBus`] so [`TeamEventLogger`] persists it in
    /// `team_events`. Centralising both emissions here means the panel/RPC
    /// paths get audit-logged the same way the dispatcher path does — no
    /// caller-side responsibility, no drift.
    ///
    /// No-op when the task has no `team_id` (`CoordTasks` can be orphan-scoped).
    async fn emit_task_topic(&self, task: &CoordTask, verb: &str) {
        // --- 1. Gateway WS topic (existing path, fire-and-forget) ----------
        if let Some(bus) = &self.bus {
            if let Some(team_id) = task.team_id.as_deref() {
                let topic = format!("team.{team_id}.task.{verb}");
                let payload = serde_json::json!({
                    "topic": topic,
                    "data": {
                        "task_id": task.id,
                        "team_id": team_id,
                        "status": task.status.as_str(),
                        "owner": task.owner,
                        "priority": task.priority.as_str(),
                        "timestamp": now_epoch(),
                    },
                });
                let _ = bus.publish_json(&payload);
            }
        }

        // --- 2. AlephEvent broadcast for downstream listeners --------------
        // TeamEventLogger persists these into `team_events` so the kanban
        // drawer can render a full timeline. GlobalBus is a singleton; no
        // injection required, and broadcast is safe with zero subscribers.
        let Some(team_id) = task.team_id.clone() else {
            return;
        };
        let task_id = task.id.clone();
        let bus = crate::event::GlobalBus::global();

        match verb {
            "created" => {
                if let Some(owner) = &task.owner {
                    bus.broadcast(
                        "coord_task_store",
                        &task_id,
                        crate::event::AlephEvent::TeamTaskAssigned {
                            team_id: team_id.clone(),
                            task_id: task_id.clone(),
                            assignee_id: owner.clone(),
                        },
                    )
                    .await;
                }
                bus.broadcast(
                    "coord_task_store",
                    &task_id,
                    crate::event::AlephEvent::TeamTaskUpdated {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        status: task.status.as_str().to_string(),
                        progress: None,
                    },
                )
                .await;
            }
            "completed" => {
                bus.broadcast(
                    "coord_task_store",
                    &task_id,
                    crate::event::AlephEvent::TeamTaskCompleted {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        result_summary: task.result.as_ref().map(|r| summarize(r, 500)),
                    },
                )
                .await;
            }
            "failed" => {
                bus.broadcast(
                    "coord_task_store",
                    &task_id,
                    crate::event::AlephEvent::TeamTaskFailed {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        error: task.result.clone().unwrap_or_default(),
                    },
                )
                .await;
            }
            // "updated" (incl. InProgress) and "cancelled" — emit a generic
            // TeamTaskUpdated carrying the new status string. There is no
            // dedicated TeamTaskCancelled variant; the status field disambiguates.
            _ => {
                bus.broadcast(
                    "coord_task_store",
                    &task_id,
                    crate::event::AlephEvent::TeamTaskUpdated {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        status: task.status.as_str().to_string(),
                        progress: None,
                    },
                )
                .await;
            }
        }
    }

    /// Run schema migration (creates tables + indexes). See [`schema::migrate`]
    /// for the full sync body; this is a thin async wrapper that owns the
    /// `tokio::sync::Mutex` lock.
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        schema::migrate(&conn)
    }
}

// ---------------------------------------------------------------------------
// CoordTaskStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl CoordTaskStore for SqliteCoordTaskStore {
    // --- Task CRUD ---

    async fn create_task(&self, input: NewCoordTask) -> crate::error::Result<CoordTask> {
        crud::create_task(self, input).await
    }

    async fn get_task(&self, id: &str) -> crate::error::Result<Option<CoordTask>> {
        crud::get_task(self, id).await
    }

    async fn update_task(
        &self,
        id: &str,
        update: CoordTaskUpdate,
    ) -> crate::error::Result<CoordTask> {
        crud::update_task(self, id, update).await
    }

    async fn list_tasks(&self, filter: CoordTaskFilter) -> crate::error::Result<Vec<CoordTask>> {
        crud::list_tasks(self, filter).await
    }

    // --- DAG queries ---

    async fn get_dependencies(&self, id: &str) -> crate::error::Result<Vec<CoordTaskId>> {
        deps::get_dependencies(self, id).await
    }

    async fn get_dependents(&self, id: &str) -> crate::error::Result<Vec<CoordTaskId>> {
        deps::get_dependents(self, id).await
    }

    async fn get_newly_unblocked(
        &self,
        completed_id: &str,
    ) -> crate::error::Result<Vec<CoordTask>> {
        deps::get_newly_unblocked(self, completed_id).await
    }

    // --- Task locking ---

    async fn acquire_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()> {
        locks::acquire_lock(self, task_id, agent_id).await
    }

    async fn release_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()> {
        locks::release_lock(self, task_id, agent_id).await
    }

    async fn release_stale_locks(&self, max_age_secs: u64) -> crate::error::Result<usize> {
        locks::release_stale_locks(self, max_age_secs).await
    }

    // --- Run history -------------------------------------------------------

    async fn start_task_run(&self, task_id: &str, agent_id: &str) -> crate::error::Result<String> {
        runs::start_task_run(self, task_id, agent_id).await
    }

    async fn finish_task_run(
        &self,
        run_id: &str,
        status: TaskRunStatus,
        summary: Option<String>,
        error: Option<String>,
    ) -> crate::error::Result<()> {
        runs::finish_task_run(self, run_id, status, summary, error).await
    }

    async fn list_task_runs(&self, task_id: &str) -> crate::error::Result<Vec<CoordTaskRun>> {
        runs::list_task_runs(self, task_id).await
    }

    async fn abandon_orphaned_runs(&self, live_task_ids: &[String]) -> crate::error::Result<usize> {
        runs::abandon_orphaned_runs(self, live_task_ids).await
    }

    async fn stamp_abandoned_run_summary(
        &self,
        task_id: &str,
        summary: &str,
    ) -> crate::error::Result<bool> {
        runs::stamp_abandoned_run_summary(self, task_id, summary).await
    }

    async fn record_run_review(
        &self,
        task_id: &str,
        verdict: ReviewVerdict,
        reviewer_kind: ReviewerKind,
        reviewer_id: Option<&str>,
    ) -> crate::error::Result<()> {
        runs::record_run_review(self, task_id, verdict, reviewer_kind, reviewer_id).await
    }

    // --- Comments ----------------------------------------------------------

    async fn add_task_comment(
        &self,
        task_id: &str,
        author: &str,
        body: &str,
    ) -> crate::error::Result<CoordTaskComment> {
        comments::add_task_comment(self, task_id, author, body).await
    }

    async fn list_task_comments(
        &self,
        task_id: &str,
    ) -> crate::error::Result<Vec<CoordTaskComment>> {
        comments::list_task_comments(self, task_id).await
    }

    // --- Exit Journal (R3) --------------------------------------------------

    async fn upsert_task_journal(
        &self,
        input: crate::agents::swarm::tasks::NewTaskExitJournal,
    ) -> crate::error::Result<crate::agents::swarm::tasks::TaskExitJournal> {
        journal::upsert_task_journal(self, input).await
    }

    async fn get_task_journal(
        &self,
        task_id: &str,
    ) -> crate::error::Result<Option<crate::agents::swarm::tasks::TaskExitJournal>> {
        journal::get_task_journal(self, task_id).await
    }

    async fn delete_team_tasks(&self, team_id: &str) -> crate::error::Result<usize> {
        crud::delete_team_tasks(self, team_id).await
    }

    async fn list_team_journals(
        &self,
        team_id: &str,
    ) -> crate::error::Result<Vec<crate::agents::swarm::tasks::TaskExitJournal>> {
        journal::list_team_journals(self, team_id).await
    }
}

#[cfg(test)]
mod review_tests {
    use super::SqliteCoordTaskStore;
    use crate::agents::swarm::tasks::{
        CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
        ReviewVerdict, ReviewerKind, TaskRunStatus,
    };

    async fn make_store() -> SqliteCoordTaskStore {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.unwrap();
        store
    }

    #[tokio::test]
    async fn record_run_review_stamps_latest_finished_run() {
        let store = make_store().await;
        let task = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "review-me".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        let run_id = store.start_task_run(&task.id, "worker").await.unwrap();
        store
            .finish_task_run(&run_id, TaskRunStatus::Completed, Some("ok".into()), None)
            .await
            .unwrap();

        store
            .record_run_review(
                &task.id,
                ReviewVerdict::Approved,
                ReviewerKind::User,
                Some("alice"),
            )
            .await
            .unwrap();

        let runs = store.list_task_runs(&task.id).await.unwrap();
        let last = runs.last().expect("at least one run");
        assert_eq!(last.review_verdict, Some(ReviewVerdict::Approved));
        assert_eq!(last.reviewer_kind, Some(ReviewerKind::User));
        assert_eq!(last.reviewer_id.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn abandon_orphaned_runs_closes_only_dead_rows() {
        let store = make_store().await;
        let mk = |subject: &str| NewCoordTask {
            team_id: Some("t1".into()),
            subject: subject.into(),
            description: String::new(),
            owner: Some("worker".into()),
            priority: Priority::Normal,
            blocked_by: Vec::new(),
            metadata: serde_json::json!({}),
        };
        let live = store.create_task(mk("live")).await.unwrap();
        let dead = store.create_task(mk("dead")).await.unwrap();
        let live_run = store.start_task_run(&live.id, "worker").await.unwrap();
        let _dead_run = store.start_task_run(&dead.id, "worker").await.unwrap();

        // Sweep with only `live` in flight → dead's row closes as Abandoned.
        let closed = store
            .abandon_orphaned_runs(std::slice::from_ref(&live.id))
            .await
            .unwrap();
        assert_eq!(closed, 1);

        let dead_runs = store.list_task_runs(&dead.id).await.unwrap();
        assert_eq!(dead_runs.len(), 1);
        assert_eq!(dead_runs[0].status, TaskRunStatus::Abandoned);
        assert!(dead_runs[0].ended_at.is_some());
        assert!(dead_runs[0]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("interrupted"));

        // Live row untouched — and still finishable afterwards.
        let live_runs = store.list_task_runs(&live.id).await.unwrap();
        assert_eq!(live_runs[0].status, TaskRunStatus::Running);
        store
            .finish_task_run(&live_run, TaskRunStatus::Completed, None, None)
            .await
            .unwrap();

        // Empty live set (boot sweep) closes everything still running.
        let _r2 = store.start_task_run(&dead.id, "worker").await.unwrap();
        let closed = store.abandon_orphaned_runs(&[]).await.unwrap();
        assert_eq!(closed, 1);
    }

    /// The crash-recovery ceiling is only real if the rows the janitor writes
    /// are the rows the counter reads. Asserted end to end (write → read back →
    /// count), not against the sentinel literal: a drift between the two sides
    /// is silent — the budget simply reads 0 forever and the unbounded
    /// re-dispatch loop comes back.
    #[tokio::test]
    async fn janitor_closed_rows_are_the_rows_the_recovery_budget_counts() {
        use crate::agents::swarm::tasks::retry::recovery_abandons_since;

        let store = make_store().await;
        let task = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "crashy".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        // Two crashes: a run row left open, closed by the boot sweep.
        for _ in 0..2 {
            let _ = store.start_task_run(&task.id, "worker").await.unwrap();
            store.abandon_orphaned_runs(&[]).await.unwrap();
        }
        // ...and one attempt the dispatcher itself deferred (agent busy). It
        // lands under the same status on purpose, and must NOT read as a crash.
        let busy = store.start_task_run(&task.id, "worker").await.unwrap();
        store
            .finish_task_run(
                &busy,
                TaskRunStatus::Abandoned,
                None,
                Some("Agent busy, attempt deferred: Agent is busy: worker".into()),
            )
            .await
            .unwrap();

        let runs = store.list_task_runs(&task.id).await.unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!(
            recovery_abandons_since(&runs, None),
            2,
            "two crashes counted, the busy deferral not"
        );
    }

    #[tokio::test]
    async fn skipped_satisfies_dependency_at_query_time() {
        let store = make_store().await;
        let parent = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "parent".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let child = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "child".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![parent.id.clone()],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        // Child sees Blocked while parent is Pending.
        let pending_child = store.get_task(&child.id).await.unwrap().unwrap();
        assert_eq!(pending_child.status, CoordTaskStatus::Blocked);

        // Mark parent as Skipped — child should become Pending (unblocked).
        store
            .update_task(
                &parent.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Skipped),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let unblocked_child = store.get_task(&child.id).await.unwrap().unwrap();
        assert_eq!(unblocked_child.status, CoordTaskStatus::Pending);
    }

    #[tokio::test]
    async fn failed_dependency_makes_child_unsatisfiable() {
        let store = make_store().await;
        let parent = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "parent".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let child = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "child".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![parent.id.clone()],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        // Parent still Pending → child is merely Blocked (deps may yet complete).
        assert_eq!(
            store.get_task(&child.id).await.unwrap().unwrap().status,
            CoordTaskStatus::Blocked
        );

        // Fail the parent → child can never run → Unsatisfiable. Verified via
        // both the get_task (derive_status) path and the list_tasks inline path.
        store
            .update_task(
                &parent.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Failed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(
            store.get_task(&child.id).await.unwrap().unwrap().status,
            CoordTaskStatus::Unsatisfiable
        );

        let listed = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("t1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let listed_child = listed.iter().find(|t| t.id == child.id).unwrap();
        assert_eq!(listed_child.status, CoordTaskStatus::Unsatisfiable);

        // The dedicated filter returns the child; the Blocked filter does not.
        let unsat = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("t1".into()),
                status: Some(CoordTaskStatus::Unsatisfiable),
            })
            .await
            .unwrap();
        assert_eq!(unsat.len(), 1);
        assert_eq!(unsat[0].id, child.id);

        let blocked = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("t1".into()),
                status: Some(CoordTaskStatus::Blocked),
            })
            .await
            .unwrap();
        assert!(blocked.iter().all(|t| t.id != child.id));
    }
}

#[cfg(test)]
mod abandoned_summary_tests {
    use super::SqliteCoordTaskStore;
    use crate::agents::swarm::tasks::{CoordTaskStore, NewCoordTask, Priority, TaskRunStatus};

    async fn make_store() -> SqliteCoordTaskStore {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.unwrap();
        store
    }

    fn new_task(subject: &str) -> NewCoordTask {
        NewCoordTask {
            team_id: Some("t1".into()),
            subject: subject.into(),
            description: String::new(),
            owner: Some("worker".into()),
            priority: Priority::Normal,
            blocked_by: Vec::new(),
            metadata: serde_json::json!({}),
        }
    }

    /// The crashed attempt's partial output lands on its own row, and
    /// `build_recovery_section` reads it out of `summary` — the "partial output
    /// (incomplete)" slot that was empty for exactly the population it exists
    /// for.
    #[tokio::test]
    async fn a_crashed_attempt_gets_its_partial_output_stamped() {
        let store = make_store().await;
        let task = store.create_task(new_task("step")).await.unwrap();
        store.start_task_run(&task.id, "worker").await.unwrap();

        let stamped = store
            .stamp_abandoned_run_summary(&task.id, "got as far as reading the file")
            .await
            .unwrap();
        assert!(stamped, "the still-running row is the crashed attempt");

        let runs = store.list_task_runs(&task.id).await.unwrap();
        assert_eq!(
            runs[0].summary.as_deref(),
            Some("got as far as reading the file")
        );
    }

    /// A finished attempt already wrote its own summary; a later stamp must not
    /// replace a deliverable with a fragment.
    #[tokio::test]
    async fn a_completed_attempts_summary_is_never_overwritten() {
        let store = make_store().await;
        let task = store.create_task(new_task("step")).await.unwrap();
        let run = store.start_task_run(&task.id, "worker").await.unwrap();
        store
            .finish_task_run(
                &run,
                TaskRunStatus::Completed,
                Some("the real answer".into()),
                None,
            )
            .await
            .unwrap();

        let stamped = store
            .stamp_abandoned_run_summary(&task.id, "a fragment")
            .await
            .unwrap();
        assert!(!stamped, "there is no crashed attempt waiting for a summary");
        let runs = store.list_task_runs(&task.id).await.unwrap();
        assert_eq!(runs[0].summary.as_deref(), Some("the real answer"));
    }

    /// Re-stamping on a later tick keeps the first, fuller reading: a second
    /// pass reduces over a log that has already been repaired, so its counters
    /// are strictly poorer.
    #[tokio::test]
    async fn a_second_stamp_does_not_overwrite_the_first() {
        let store = make_store().await;
        let task = store.create_task(new_task("step")).await.unwrap();
        store.start_task_run(&task.id, "worker").await.unwrap();

        assert!(store
            .stamp_abandoned_run_summary(&task.id, "first reading")
            .await
            .unwrap());
        assert!(!store
            .stamp_abandoned_run_summary(&task.id, "poorer second reading")
            .await
            .unwrap());
        let runs = store.list_task_runs(&task.id).await.unwrap();
        assert_eq!(runs[0].summary.as_deref(), Some("first reading"));
    }
}
