//! The deactivation freeze's team-task leg (round 11, N2): the one owner of
//! "pause / count the dispatcher-managed tasks on teams a principal owns".
//!
//! The other four legs each own their mutation inside their subsystem
//! (`GoalStore::pause_all_owned_by`, `LoopRegistry::pause_all_owned_by`,
//! `CronService::pause_all_owned_by`, `HeartbeatService::pause_all_owned_by`);
//! this is the same shape for team tasks. `handlers::users` calls the two
//! methods below and never touches a store.
//!
//! [`TeamTaskStores`] holds the RAW (unscoped) team store: the freeze must see
//! every team the deactivated principal owns, not the teams visible to the
//! admin running it (`ScopedTeamStore` filters by the ambient actor). Its
//! fields are private to `teams`, so that store never leaves this module —
//! the only things a holder can do with it are the owner-filtered pause and
//! count here.

use super::TeamTaskStores;
use crate::agents::swarm::tasks::{
    CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, PAUSED_FROM_KEY,
};
use crate::sync_primitives::Arc;

impl TeamTaskStores {
    /// Seal the raw team store and the coordination-task store into the
    /// freeze's handle. Boot builds this right where both stores open; after
    /// that the raw team store is reachable only through the methods below.
    #[must_use]
    pub fn new(teams: Arc<dyn super::TeamStore>, tasks: Arc<dyn CoordTaskStore>) -> Self {
        Self { teams, tasks }
    }

    /// Ids of the teams `user_id` owns, by the shared owner predicate
    /// ([`aleph_protocol::users::owned_by`]) — a legacy NULL-owner team
    /// belongs to nobody, exactly as on the other four legs.
    async fn team_ids_owned_by(&self, user_id: &str) -> crate::error::Result<Vec<String>> {
        Ok(self
            .teams
            .list_teams()
            .await?
            .into_iter()
            .filter(|t| aleph_protocol::users::owned_by(t.owner_user_id.as_deref(), user_id))
            .map(|t| t.id)
            .collect())
    }

    /// Pause every dispatcher-managed task that could still be CLAIMED
    /// (`Pending`, or `Blocked` — stored `pending`, derived at read time) on
    /// teams `user_id` owns, and return how many were paused.
    ///
    /// Left alone on purpose: `InProgress` (pausing a live run makes the
    /// finalize fence discard its work — the run completes instead),
    /// `WaitingReview` (pausing it needs the `PAUSED_FROM_KEY` stamp, and it
    /// is not claimable anyway), and every terminal task (finished work is
    /// not rewritten).
    ///
    /// Each pause writes, in one update: `Paused`, a `result` that names the
    /// deactivated principal and the exit (the shape of the dispatcher's own
    /// refused-authority pause and of the goal / loop legs' "Account '…' was
    /// deactivated"), and a null `PAUSED_FROM_KEY` — the task is Pending
    /// here, so a stale `paused_from` from an earlier pause→retry cycle would
    /// otherwise make a later resume restore it to WaitingReview.
    ///
    /// `None` = "I do not know", never "none": the team list could not be
    /// read, or a task list or a pause failed on some team. The leg keeps
    /// going past a failed team (the other teams are still frozen) but
    /// withholds its count, because some tasks may have been paused and some
    /// not. `Some(0)` means the sweep ran everywhere and found nothing to
    /// pause.
    pub(crate) async fn pause_dispatcher_tasks_owned_by(&self, user_id: &str) -> Option<usize> {
        let team_ids = match self.team_ids_owned_by(user_id).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %e,
                    "users.update: failed to list teams during deactivation"
                );
                return None;
            }
        };
        let reason = deactivation_pause_text(user_id);
        let mut paused = 0usize;
        let mut failed = false;
        for team_id in team_ids {
            let tasks = match self
                .tasks
                .list_tasks(CoordTaskFilter {
                    team_id: Some(team_id.clone()),
                    status: None,
                })
                .await
            {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        user_id = %user_id,
                        team_id = %team_id,
                        error = %e,
                        "users.update: failed to list a team's tasks during deactivation; \
                         continuing with the next team"
                    );
                    failed = true;
                    continue;
                }
            };
            for task in tasks.iter().filter(|t| {
                crate::teams::dispatcher::is_dispatcher_managed(t)
                    && matches!(t.status, CoordTaskStatus::Pending | CoordTaskStatus::Blocked)
            }) {
                let update = CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Paused),
                    result: Some(reason.clone()),
                    metadata: Some(crate::agents::swarm::tasks::merge_metadata_patch(
                        &task.metadata,
                        serde_json::json!({ PAUSED_FROM_KEY: serde_json::Value::Null }),
                    )),
                    ..Default::default()
                };
                match self.tasks.update_task(&task.id, update).await {
                    Ok(_) => paused += 1,
                    Err(e) => {
                        tracing::warn!(
                            user_id = %user_id,
                            task_id = %task.id,
                            error = %e,
                            "users.update: failed to pause an owned team task during deactivation"
                        );
                        failed = true;
                    }
                }
            }
        }
        if paused > 0 {
            tracing::warn!(
                user_id = %user_id,
                count = paused,
                "users.update: deactivation paused owned team tasks"
            );
        }
        (!failed).then_some(paused)
    }

    /// The preview twin: every NON-TERMINAL dispatcher-managed task on teams
    /// `user_id` owns (paused and in-flight ones included — the operator must
    /// see what they are about to strand). `None` when a scan failed; the
    /// failure is logged, like the absent-store arm the caller logs.
    pub(crate) async fn count_dispatcher_tasks_owned_by(&self, user_id: &str) -> Option<usize> {
        let team_ids = match self.team_ids_owned_by(user_id).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %e,
                    "users.get: the team-task leg was NOT measured — listing teams failed"
                );
                return None;
            }
        };
        let mut owned = 0usize;
        let mut failed = false;
        for team_id in team_ids {
            match self
                .tasks
                .list_tasks(CoordTaskFilter {
                    team_id: Some(team_id.clone()),
                    status: None,
                })
                .await
            {
                Ok(tasks) => {
                    owned += tasks
                        .iter()
                        .filter(|t| {
                            crate::teams::dispatcher::is_dispatcher_managed(t)
                                && !t.status.is_terminal()
                        })
                        .count();
                }
                Err(e) => {
                    tracing::warn!(
                        user_id = %user_id,
                        team_id = %team_id,
                        error = %e,
                        "users.get: the team-task leg was NOT measured — listing a team's \
                         tasks failed"
                    );
                    failed = true;
                }
            }
        }
        (!failed).then_some(owned)
    }
}

/// The `result` a frozen team task carries, where the task drawer shows it:
/// who was deactivated, and the exit that exists (reactivate, then set the
/// task back to pending — the dispatcher's claim-time authority check asks
/// again then).
fn deactivation_pause_text(user_id: &str) -> String {
    format!(
        "Paused by the deactivation freeze: account '{user_id}' was deactivated and owns \
         this task's team. An administrator can reactivate '{user_id}'; then set this task \
         back to pending."
    )
}

/// Seeding helpers for the leg's tests here and in `handlers::users` — the
/// store fields are private to `teams`, so tests elsewhere seed through these.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::agents::swarm::tasks::{CoordTask, NewCoordTask, Priority};

    /// Fresh in-memory team + coordination-task stores.
    pub(crate) async fn in_memory_stores() -> TeamTaskStores {
        let teams = crate::teams::SqliteTeamStore::new(rusqlite::Connection::open_in_memory().unwrap());
        teams.migrate().await.unwrap();
        let tasks = crate::agents::swarm::tasks::store::SqliteCoordTaskStore::new(
            rusqlite::Connection::open_in_memory().unwrap(),
        );
        tasks.migrate().await.unwrap();
        TeamTaskStores::new(Arc::new(teams), Arc::new(tasks))
    }

    /// One team owned by `owner` (or legacy when `None`). Asserts its own
    /// precondition: a legacy team really has no owner stamp, so a future
    /// owner-stamp fallback cannot silently turn the legacy case into a
    /// second owned team (判据 §3).
    pub(crate) async fn seed_team(stores: &TeamTaskStores, owner: Option<&str>) -> String {
        let create = stores.teams.create_team(crate::teams::types::NewTeam {
            name: format!("team-{}", uuid::Uuid::new_v4()),
            description: String::new(),
            leader_id: "lead".into(),
        });
        let team = match owner {
            Some(u) => {
                crate::scope::with_scope(Some(crate::scope::ScopeAttribution::personal(u)), create)
                    .await
            }
            None => create.await,
        }
        .unwrap();
        assert_eq!(
            team.owner_user_id.as_deref(),
            owner,
            "fixture precondition: the seeded team's owner stamp"
        );
        team.id
    }

    /// One task on `team_id`, dispatcher-managed or hand-tracked.
    pub(crate) async fn seed_task(
        stores: &TeamTaskStores,
        team_id: &str,
        managed: bool,
        blocked_by: Vec<String>,
    ) -> String {
        let metadata = if managed {
            serde_json::json!({ "managed_by": "dispatcher" })
        } else {
            serde_json::json!({})
        };
        stores
            .tasks
            .create_task(NewCoordTask {
                team_id: Some(team_id.to_string()),
                subject: "s".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by,
                metadata,
            })
            .await
            .unwrap()
            .id
    }

    /// One team owned by `owner` with one unblocked task per `managed` flag.
    pub(crate) async fn seed_team_tasks(
        stores: &TeamTaskStores,
        owner: Option<&str>,
        managed: &[bool],
    ) -> Vec<String> {
        let team_id = seed_team(stores, owner).await;
        let mut ids = Vec::new();
        for m in managed {
            ids.push(seed_task(stores, &team_id, *m, vec![]).await);
        }
        ids
    }

    /// Overwrite one task's stored status (and optionally its metadata).
    pub(crate) async fn set_task(
        stores: &TeamTaskStores,
        id: &str,
        status: CoordTaskStatus,
        metadata: Option<serde_json::Value>,
    ) {
        stores
            .tasks
            .update_task(
                id,
                CoordTaskUpdate {
                    status: Some(status),
                    metadata,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    /// One task, read back from the store (status derived).
    pub(crate) async fn task(stores: &TeamTaskStores, id: &str) -> CoordTask {
        stores.tasks.get_task(id).await.unwrap().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    /// A hand-tracked task (no `managed_by: dispatcher`) is never claimed by
    /// the dispatcher, so the freeze leaves it alone.
    #[tokio::test]
    async fn deactivation_leaves_a_hand_tracked_team_task_alone() {
        let stores = in_memory_stores().await;
        let alice = seed_team_tasks(&stores, Some("u-alice"), &[false]).await;

        assert_eq!(stores.pause_dispatcher_tasks_owned_by("u-alice").await, Some(0));
        assert_eq!(
            task(&stores, &alice[0]).await.status,
            CoordTaskStatus::Pending,
            "not dispatcher-managed"
        );
    }

    /// Someone else's team, and a legacy (NULL-owner) team, are not hers:
    /// the shared owner predicate reads a NULL owner as nobody's.
    #[tokio::test]
    async fn deactivation_leaves_other_and_legacy_teams_alone() {
        let stores = in_memory_stores().await;
        let bob = seed_team_tasks(&stores, Some("u-bob"), &[true]).await;
        let legacy = seed_team_tasks(&stores, None, &[true]).await;

        let paused = stores.pause_dispatcher_tasks_owned_by("u-alice").await;
        assert_eq!(
            task(&stores, &bob[0]).await.status,
            CoordTaskStatus::Pending,
            "not her team"
        );
        assert_eq!(
            task(&stores, &legacy[0]).await.status,
            CoordTaskStatus::Pending,
            "a legacy team belongs to nobody"
        );
        assert_eq!(paused, Some(0), "a measured leg that found nothing");
    }

    /// Only a still-CLAIMABLE task is paused: a derived-`Blocked` one is, a
    /// running one (its run completes), one awaiting review and a finished
    /// one are not. Every task here is dispatcher-managed and on her own
    /// team, so only the status filter decides.
    #[tokio::test]
    async fn deactivation_pauses_blocked_but_leaves_running_review_and_terminal_team_tasks_alone(
    ) {
        let stores = in_memory_stores().await;
        let team = seed_team(&stores, Some("u-alice")).await;
        let running = seed_task(&stores, &team, true, vec![]).await;
        let blocked = seed_task(&stores, &team, true, vec![running.clone()]).await;
        let review = seed_task(&stores, &team, true, vec![]).await;
        let done = seed_task(&stores, &team, true, vec![]).await;
        set_task(&stores, &running, CoordTaskStatus::InProgress, None).await;
        set_task(&stores, &review, CoordTaskStatus::WaitingReview, None).await;
        set_task(&stores, &done, CoordTaskStatus::Completed, None).await;
        assert_eq!(
            task(&stores, &blocked).await.status,
            CoordTaskStatus::Blocked,
            "fixture precondition: a pending task behind a running one reads Blocked"
        );

        assert_eq!(stores.pause_dispatcher_tasks_owned_by("u-alice").await, Some(1));
        assert_eq!(task(&stores, &blocked).await.status, CoordTaskStatus::Paused);
        assert_eq!(task(&stores, &running).await.status, CoordTaskStatus::InProgress);
        assert_eq!(task(&stores, &review).await.status, CoordTaskStatus::WaitingReview);
        assert_eq!(task(&stores, &done).await.status, CoordTaskStatus::Completed);
    }

    /// A stale `paused_from` (left by an earlier pause→retry cycle) is nulled
    /// in the freeze's own write, so a later resume restores Pending rather
    /// than WaitingReview. The rest of the metadata survives the patch.
    #[tokio::test]
    async fn a_frozen_team_task_drops_its_stale_paused_from() {
        let stores = in_memory_stores().await;
        let ids = seed_team_tasks(&stores, Some("u-alice"), &[true]).await;
        set_task(
            &stores,
            &ids[0],
            CoordTaskStatus::Pending,
            Some(serde_json::json!({
                "managed_by": "dispatcher",
                PAUSED_FROM_KEY: "waiting_review",
            })),
        )
        .await;

        assert_eq!(stores.pause_dispatcher_tasks_owned_by("u-alice").await, Some(1));
        let frozen = task(&stores, &ids[0]).await;
        assert_eq!(frozen.status, CoordTaskStatus::Paused);
        assert!(
            frozen.metadata.get(PAUSED_FROM_KEY).is_none_or(serde_json::Value::is_null),
            "{}",
            frozen.metadata
        );
        assert_eq!(frozen.metadata["managed_by"], "dispatcher");
    }

    /// The preview counts dispatcher-managed tasks only: a hand-tracked task
    /// on her own team is not something the dispatcher will run.
    #[tokio::test]
    async fn the_preview_counts_only_dispatcher_managed_team_tasks() {
        let stores = in_memory_stores().await;
        seed_team_tasks(&stores, Some("u-alice"), &[true, false]).await;
        assert_eq!(stores.count_dispatcher_tasks_owned_by("u-alice").await, Some(1));
    }

    /// The preview counts what the operator is about to strand — paused and
    /// running tasks included — but not finished work.
    #[tokio::test]
    async fn the_preview_counts_paused_and_running_but_not_terminal_team_tasks() {
        let stores = in_memory_stores().await;
        let ids = seed_team_tasks(&stores, Some("u-alice"), &[true, true, true]).await;
        set_task(&stores, &ids[0], CoordTaskStatus::Paused, None).await;
        set_task(&stores, &ids[1], CoordTaskStatus::InProgress, None).await;
        set_task(&stores, &ids[2], CoordTaskStatus::Completed, None).await;
        assert_eq!(stores.count_dispatcher_tasks_owned_by("u-alice").await, Some(2));
    }
}
