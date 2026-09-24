//! Fire-time authority for dispatcher-managed team tasks (round 11, N1).
//!
//! The dispatcher claims tasks from a bare loop with no live caller, so it
//! decides at CLAIM time whose authority the member run executes under: the
//! task's author (stamped by `CoordTaskStore::create_task`), else the team's
//! owner/scope, else — for a legacy NULL-owner team — the session that
//! created the task. `schedule::dispatch_once` asks
//! `scope::authority::resolve` with these facts, maps the verdict through
//! [`claim_authority`] and launches the run through
//! [`spawn_under_authority`], so `runner::task_run_metadata` and the
//! `allowed_users` fence read live task-locals instead of `None`.
//!
//! Why a carrier and not `gateway::fire_gate::apply`: `fire_gate` stamps a
//! `Granted` onto a metadata map the executor already holds. The dispatcher
//! holds none — the member run's metadata is built deep inside
//! `execute_member_task` from task-locals, and the agent's `allowed_users`
//! fence reads `ambient_actor()` from the same task-locals — so the grant has
//! to be re-established around the spawn, not stamped.

use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate};
use crate::scope::authority::{FireAuthority, FireSubject};

/// The persisted facts one task's authority is resolved from.
pub(super) struct TaskFireFacts {
    owner: Option<String>,
    scope: Option<String>,
    author: Option<String>,
}

impl TaskFireFacts {
    /// No carried role: a coordination task stores none, so the resolver
    /// reads it as the operator default and downgrades it to `member` for a
    /// member person.
    pub(super) fn subject(&self) -> FireSubject<'_> {
        FireSubject {
            owner: self.owner.as_deref(),
            scope: self.scope.as_deref(),
            author: self.author.as_deref(),
            carried_role: None,
        }
    }
}

/// Read the facts. `Err` = a row could not be READ (R-a: the caller leaves
/// the task pending and asks again next tick). A missing row is not an error:
/// it just contributes nothing.
pub(super) async fn task_fire_facts(
    teams: &dyn crate::teams::TeamStore,
    sessions: &dyn crate::gateway::session_store::SessionStore,
    task: &CoordTask,
) -> Result<TaskFireFacts, String> {
    let author = task
        .metadata
        .get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    if let Some(team_id) = task.team_id.as_deref() {
        let team = teams
            .get_team(team_id)
            .await
            .map_err(|e| format!("team row unreadable: {e}"))?;
        if let Some(team) = team {
            if team.owner_user_id.is_some() || team.scope_id.is_some() {
                return Ok(TaskFireFacts {
                    owner: team.owner_user_id,
                    scope: team.scope_id,
                    author,
                });
            }
        }
    }
    // A legacy (NULL-owner) team: the session that created the task is the
    // only other durable owner record.
    if let Some(origin) = crate::gateway::goal_budget::origin_session_from_metadata(&task.metadata)
    {
        if let Some(key) = crate::routing::session_key::SessionKey::from_key_string(&origin) {
            let row = sessions
                .get_metadata(&key)
                .await
                .map_err(|e| format!("origin session row unreadable: {e}"))?;
            if let Some(row) = row {
                return Ok(TaskFireFacts {
                    owner: row.owner_user_id,
                    scope: row.scope_id,
                    author,
                });
            }
        }
    }
    Ok(TaskFireFacts {
        owner: None,
        scope: None,
        author,
    })
}

/// What the claim loop does with one task after the authority answer.
pub(super) enum ClaimAuthority {
    /// Claim and launch. `Some` re-establishes the granted attribution around
    /// the spawn; `None` (Legacy) spawns bare, byte-identical to before.
    Launch {
        carried: Option<crate::scope::CarriedAttribution>,
    },
    /// Do not claim this tick. Already acted on: a refusal has parked the
    /// task `Paused` with its reason; an unknown answer left it `Pending`.
    Hold,
}

/// Map a fire-time verdict onto the claim loop — the one place the
/// dispatcher turns a [`FireAuthority`] into a launch decision, kept out of
/// `dispatch_once` so a test can drive it with `resolve_with` (no lib test may
/// install the global users store that `resolve` reads).
///
/// Ruling R-a / 判据 §8: `Unknown` is NOT a failure. The task is not failed,
/// not paused, not retried-as-failed and releases no dependents — it stays
/// `Pending` and is asked about again on the next tick. Only a settled
/// `Refused` stops it, and says why (§14).
pub(super) async fn claim_authority(
    verdict: FireAuthority,
    tasks: &dyn CoordTaskStore,
    task_id: &str,
) -> ClaimAuthority {
    let reason = verdict.reason();
    match verdict {
        FireAuthority::Legacy => ClaimAuthority::Launch { carried: None },
        FireAuthority::Granted(granted) => ClaimAuthority::Launch {
            carried: Some(granted.carried()),
        },
        FireAuthority::Refused(_) => {
            let reason = reason.unwrap_or_else(|| "authority refused".to_string());
            pause_for_refused_authority(tasks, task_id, &reason).await;
            ClaimAuthority::Hold
        }
        FireAuthority::Unknown(_) => {
            tracing::warn!(task_id = %task_id, reason = ?reason,
                "dispatcher: authority unknown; task left pending");
            ClaimAuthority::Hold
        }
    }
}

/// A settled refusal (deactivated / deleted person): park the task `Paused`
/// with the reason on its `result`, where the task drawer shows it. Paused is
/// never claimed and does not satisfy dependents, so nothing downstream runs
/// under the refused authority. Resuming (`task_update status=pending`) asks
/// again on the next tick.
pub(super) async fn pause_for_refused_authority(
    tasks: &dyn CoordTaskStore,
    task_id: &str,
    reason: &str,
) {
    let update = CoordTaskUpdate {
        status: Some(CoordTaskStatus::Paused),
        result: Some(format!(
            "Paused by the dispatcher: {reason}. Set it back to pending once an \
             administrator has reactivated that person."
        )),
        ..Default::default()
    };
    match tasks.update_task(task_id, update).await {
        Ok(_) => tracing::warn!(task_id = %task_id, reason = %reason,
            "dispatcher: task authority refused at claim time; task paused"),
        Err(e) => tracing::warn!(task_id = %task_id, error = %e,
            "dispatcher: failed to pause a task whose authority was refused"),
    }
}

/// Spawn `fut` with the granted attribution re-established inside the new
/// task — `tokio::spawn` drops every task-local, which is exactly how N1
/// happened. `None` (Legacy) spawns bare, byte-identical to before.
pub(in crate::teams::dispatcher) fn spawn_under_authority<F, T>(
    carried: Option<crate::scope::CarriedAttribution>,
    fut: F,
) -> tokio::task::JoinHandle<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match carried {
        Some(carried) => tokio::spawn(carried.reestablish(fut)),
        None => tokio::spawn(fut),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;
    use crate::agents::swarm::tasks::{NewCoordTask, Priority};
    use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus, OWNER_USER_ID};
    use crate::scope::authority::resolve_with;
    use crate::teams::store::SqliteTeamStore;
    use crate::teams::types::NewTeam;
    use crate::teams::TeamStore;
    use rusqlite::Connection;

    async fn teams() -> SqliteTeamStore {
        let s = SqliteTeamStore::new(Connection::open_in_memory().unwrap());
        s.migrate().await.unwrap();
        s
    }

    async fn coord() -> SqliteCoordTaskStore {
        let s = SqliteCoordTaskStore::new(Connection::open_in_memory().unwrap());
        s.migrate().await.unwrap();
        s
    }

    fn sessions(dir: &tempfile::TempDir) -> crate::gateway::session_manager::SessionManager {
        crate::gateway::session_manager::SessionManager::new(
            crate::gateway::session_manager::SessionManagerConfig {
                db_path: dir.path().join("sessions.db"),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn task(team_id: Option<String>, metadata: serde_json::Value) -> CoordTask {
        CoordTask {
            id: "task-1".into(),
            team_id,
            subject: "s".into(),
            description: String::new(),
            status: CoordTaskStatus::Pending,
            owner: Some("worker".into()),
            priority: Priority::Normal,
            result: None,
            metadata,
            dependencies: vec![],
            created_at: 0,
            started_at: None,
            completed_at: None,
            locked_by: None,
            locked_at: None,
        }
    }

    /// A team created under `owner`'s attribution in `scope`.
    async fn team_owned_by(
        teams: &SqliteTeamStore,
        owner: &str,
        scope: crate::scope::ScopeId,
    ) -> crate::teams::types::Team {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution {
                owner_user_id: owner.into(),
                scope,
            }),
            teams.create_team(NewTeam {
                name: "t".into(),
                description: String::new(),
                leader_id: "lead".into(),
            }),
        )
        .await
        .unwrap()
    }

    /// A pending, dispatcher-managed task authored by `author` (stamped the
    /// production way: `create_task` under that person's ambient scope).
    async fn authored_task(coord: &SqliteCoordTaskStore, team_id: &str, author: &str) -> CoordTask {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal(author)),
            coord.create_task(NewCoordTask {
                team_id: Some(team_id.to_string()),
                subject: "s".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({ "managed_by": "dispatcher" }),
            }),
        )
        .await
        .unwrap()
    }

    /// The same two reads the production member run makes inside the spawn:
    /// `task_run_metadata` (runner.rs) and the Agent arm's `allowed_users`
    /// fence over `ambient_actor()`.
    fn run_metadata_and_fence(
        allowed: Option<Vec<String>>,
    ) -> (std::collections::HashMap<String, String>, bool) {
        let m = crate::teams::dispatcher::runner::task_run_metadata("t1", "task-1", None, None);
        let actor = crate::gateway::visibility::ambient_actor();
        let admitted =
            crate::config::types::agent_admits_user(allowed.as_deref(), actor.as_deref());
        (m, admitted)
    }

    /// The owner half comes from the TEAM row, the author half from the task.
    #[tokio::test]
    async fn fire_facts_take_the_owner_from_the_team_row_and_the_author_from_the_task() {
        let teams = teams().await;
        let team = team_owned_by(
            &teams,
            "u-alice",
            crate::scope::ScopeId::Project("p-room".into()),
        )
        .await;
        let dir = tempfile::tempdir().unwrap();
        let t = task(
            Some(team.id.clone()),
            serde_json::json!({ crate::gateway::execution_engine::AUTHOR_USER_KEY: "u-bob" }),
        );

        let facts = task_fire_facts(&teams, &sessions(&dir), &t).await.unwrap();
        let s = facts.subject();
        assert_eq!(s.owner, Some("u-alice"));
        assert_eq!(s.scope, Some("project:p-room"));
        assert_eq!(s.author, Some("u-bob"));
        assert_eq!(s.carried_role, None);
    }

    /// A legacy team (NULL owner) falls back to the session that created the
    /// task (`goal_budget::origin_session_from_metadata`).
    #[tokio::test]
    async fn a_legacy_team_falls_back_to_the_origin_session_row() {
        let teams = teams().await;
        let team = teams
            .create_team(NewTeam {
                name: "legacy".into(),
                description: String::new(),
                leader_id: "lead".into(),
            })
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions(&dir);
        let origin = crate::routing::session_key::SessionKey::main("main");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-carol")),
            crate::gateway::session_store::SessionStore::get_or_create(&sessions, &origin),
        )
        .await
        .unwrap();
        let t = task(
            Some(team.id),
            serde_json::json!({
                crate::gateway::goal_budget::ORIGIN_SESSION_METADATA_KEY: origin.to_key_string()
            }),
        );

        let facts = task_fire_facts(&teams, &sessions, &t).await.unwrap();
        assert_eq!(facts.subject().owner, Some("u-carol"));
    }

    #[tokio::test]
    async fn a_refused_task_is_paused_with_the_reason_on_its_result() {
        let coord = coord().await;
        let created = coord
            .create_task(NewCoordTask {
                team_id: None,
                subject: "s".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({ "managed_by": "dispatcher" }),
            })
            .await
            .unwrap();

        pause_for_refused_authority(&coord, &created.id, "principal deactivated").await;

        let after = coord.get_task(&created.id).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Paused);
        assert!(after
            .result
            .as_deref()
            .is_some_and(|r| r.contains("principal deactivated")));
    }

    /// Ruling (b) / 判据 §4: the GRANT the resolver produced is the one the
    /// member run executes under — not `Granted::legacy`, not a bare spawn.
    /// Drives the production chain from the stored rows: `create_task`
    /// stamps Bob, `task_fire_facts` reads Alice's room off the team row,
    /// the resolver caps Bob (a member) at `member`, and [`claim_authority`]
    /// hands over the carrier `dispatch_once` spawns under. Re-established
    /// here with `reestablish` directly so this test and the
    /// `spawn_under_authority` one in `runner.rs` each have exactly one
    /// mutation that reddens them.
    #[tokio::test]
    async fn a_granted_claim_launches_under_the_resolved_member_authority() {
        let users = SecurityStore::in_memory().unwrap();
        users
            .create_user("u-alice", "Alice", UserRole::Admin)
            .unwrap();
        users.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let team = team_owned_by(
            &teams,
            "u-alice",
            crate::scope::ScopeId::Project("p-room".into()),
        )
        .await;
        let t = authored_task(&coord, &team.id, "u-bob").await;

        let facts = task_fire_facts(&teams, &sessions(&dir), &t).await.unwrap();
        let verdict = resolve_with(Some(&users), &facts.subject());
        let ClaimAuthority::Launch {
            carried: Some(carried),
        } = claim_authority(verdict, &coord, &t.id).await
        else {
            panic!("an active member author must launch under a carried grant");
        };
        let (m, admitted) = tokio::spawn(
            carried.reestablish(async { run_metadata_and_fence(Some(vec!["u-alice".into()])) }),
        )
        .await
        .unwrap();

        assert_eq!(m.get("caller_role").map(String::as_str), Some("member"));
        assert_eq!(
            m.get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
                .map(String::as_str),
            Some("u-bob")
        );
        assert_eq!(
            crate::scope::scope_from_metadata(&m).map(|a| a.owner_user_id),
            Some("u-alice".to_string())
        );
        assert!(!admitted, "an agent fenced to Alice must refuse Bob's task");
    }

    /// Controller ruling C3: single-user AUTHORITY is unchanged. A u-owner
    /// team and a u-owner-authored task resolve `Granted` (not `Legacy`) and
    /// now carry u-owner attribution — intended — but an active admin gets
    /// NO role ceiling, and the agent fence admits the owner both when it
    /// names u-owner and when it is unset / empty.
    #[tokio::test]
    async fn an_owner_authored_task_on_an_owner_team_runs_with_no_role_ceiling() {
        let users = SecurityStore::in_memory().unwrap(); // bootstraps u-owner (Admin)
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let team = team_owned_by(
            &teams,
            OWNER_USER_ID,
            crate::scope::ScopeId::Personal(OWNER_USER_ID.into()),
        )
        .await;
        let t = authored_task(&coord, &team.id, OWNER_USER_ID).await;
        assert_eq!(
            t.metadata[crate::gateway::execution_engine::AUTHOR_USER_KEY],
            OWNER_USER_ID
        );

        for allowed in [
            Some(vec![OWNER_USER_ID.to_string()]),
            None,
            Some(Vec::new()),
        ] {
            let facts = task_fire_facts(&teams, &sessions(&dir), &t).await.unwrap();
            let verdict = resolve_with(Some(&users), &facts.subject());
            assert!(
                matches!(verdict, FireAuthority::Granted(_)),
                "u-owner rows resolve Granted"
            );
            let ClaimAuthority::Launch { carried } = claim_authority(verdict, &coord, &t.id).await
            else {
                panic!("an active admin must launch");
            };
            let fence = allowed.clone();
            let (m, admitted) =
                spawn_under_authority(carried, async move { run_metadata_and_fence(fence) })
                    .await
                    .unwrap();
            assert!(
                !m.contains_key("caller_role"),
                "an active admin gets no caller_role ceiling ({allowed:?}): {m:?}"
            );
            assert_eq!(
                m.get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
                    .map(String::as_str),
                Some(OWNER_USER_ID)
            );
            assert_eq!(
                crate::scope::scope_from_metadata(&m),
                Some(crate::scope::ScopeAttribution::personal(OWNER_USER_ID))
            );
            assert!(admitted, "the owner passes the fence {allowed:?}");
        }
    }

    /// Ruling (a) / R-a: an unknown answer is not a failure — the task is
    /// neither launched nor paused nor failed; it stays Pending, untouched.
    #[tokio::test]
    async fn an_unknown_authority_holds_the_task_pending_and_unmarked() {
        let coord = coord().await;
        let created = coord
            .create_task(NewCoordTask {
                team_id: None,
                subject: "s".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({ "managed_by": "dispatcher" }),
            })
            .await
            .unwrap();

        let decision = claim_authority(
            FireAuthority::Unknown("disk I/O error".into()),
            &coord,
            &created.id,
        )
        .await;

        assert!(matches!(decision, ClaimAuthority::Hold));
        let after = coord.get_task(&created.id).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Pending);
        assert_eq!(after.result, None, "an unknown answer writes no outcome");
    }

    /// A settled refusal at the claim: held AND parked Paused with the reason.
    #[tokio::test]
    async fn a_refused_claim_is_held_and_paused_with_the_principal_named() {
        let users = SecurityStore::in_memory().unwrap();
        users.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        users
            .update_user("u-bob", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let team = team_owned_by(
            &teams,
            OWNER_USER_ID,
            crate::scope::ScopeId::Project("p-room".into()),
        )
        .await;
        let t = authored_task(&coord, &team.id, "u-bob").await;

        let facts = task_fire_facts(&teams, &sessions(&dir), &t).await.unwrap();
        let verdict = resolve_with(Some(&users), &facts.subject());
        assert!(matches!(
            claim_authority(verdict, &coord, &t.id).await,
            ClaimAuthority::Hold
        ));
        let after = coord.get_task(&t.id).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Paused);
        assert!(after
            .result
            .as_deref()
            .is_some_and(|r| r.contains("principal deactivated")));
    }

    /// Ruling (b), the one seam no behaviour test can reach: `dispatch_once`
    /// resolves through the GLOBAL users store, which no lib test may
    /// install, so the wiring "resolve these facts → map → spawn under the
    /// result" is pinned at the source. A swap to a bare `tokio::spawn`
    /// (the N1 shape) or a resolver over any other subject goes red here.
    #[test]
    fn dispatch_once_launches_every_claimed_task_under_its_resolved_authority() {
        let src = include_str!("mod.rs");
        let start = src
            .find("pub(crate) async fn dispatch_once")
            .expect("dispatch_once present");
        let end = src[start..]
            .find("async fn resolve_dispatch_target")
            .map(|i| start + i)
            .expect("dispatch_once is followed by resolve_dispatch_target");
        let body: String = src[start..end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            body.matches("crate::scope::authority::resolve(&facts.subject())")
                .count(),
            1,
            "the claim loop resolves exactly the stored facts"
        );
        assert_eq!(body.matches("authority::claim_authority(").count(), 1);
        assert_eq!(
            body.matches("spawn_under_authority(carried,").count(),
            1,
            "the member run is launched under the resolved carrier"
        );
        assert_eq!(
            body.matches("tokio::spawn(").count(),
            0,
            "a bare spawn in the claim loop drops every task-local (N1)"
        );
    }
}
