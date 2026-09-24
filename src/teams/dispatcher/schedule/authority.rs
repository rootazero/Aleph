//! Fire-time authority for dispatcher-managed team tasks (round 11, N1).
//!
//! The dispatcher claims tasks from a bare loop with no live caller, so it
//! decides at CLAIM time whose authority the member run executes under: the
//! task's author (stamped by `CoordTaskStore::create_task`), else the team's
//! owner/scope, else — for a legacy NULL-owner team — the session that
//! created the task. `schedule::dispatch_once` hands
//! `scope::authority::resolve` to [`authorize_claim`], which reads these
//! facts, asks the resolver and maps the verdict; the member run is then
//! launched through the returned [`AuthorizedLaunch`], so
//! `runner::task_run_metadata` and the `allowed_users` fence read live
//! task-locals instead of `None`.
//!
//! Why a carrier and not `gateway::fire_gate::apply`: `fire_gate` stamps a
//! `Granted` onto a metadata map the executor already holds. The dispatcher
//! holds none — the member run's metadata is built deep inside
//! `execute_member_task` from task-locals, and the agent's `allowed_users`
//! fence reads `ambient_actor()` from the same task-locals — so the grant has
//! to be re-established around the spawn, not stamped.

use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate};
use crate::scope::authority::{FireAuthority, FireSubject, RefusalReason};

/// The persisted facts one task's authority is resolved from.
pub(super) struct TaskFireFacts {
    owner: Option<String>,
    scope: Option<String>,
    author: Option<String>,
    /// Where `owner` was read from, for a refusal that has to name the
    /// person it checked: `"team owner"` or `"origin session owner"`.
    owner_label: &'static str,
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

    /// The person the resolver checked, labelled: the task author when one
    /// is carried, else the owner — `resolve_with`'s own `author.or(owner)`,
    /// with the same "an empty id is absent" reading.
    fn checked_principal(&self) -> Option<(&'static str, &str)> {
        match self.author.as_deref().filter(|a| !a.is_empty()) {
            Some(author) => Some(("task author", author)),
            None => self
                .owner
                .as_deref()
                .filter(|o| !o.is_empty())
                .map(|owner| (self.owner_label, owner)),
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
                    owner_label: "team owner",
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
                    owner_label: "origin session owner",
                });
            }
        }
    }
    Ok(TaskFireFacts {
        owner: None,
        scope: None,
        author,
        owner_label: "team owner",
    })
}

/// A claim the fire-time authority admitted. The only way to launch the
/// member run: the carrier is private, so `dispatch_once` cannot build,
/// swap or shadow it — it can only spawn what [`authorize_claim`] resolved.
pub(super) struct AuthorizedLaunch {
    /// `Some` re-establishes the granted attribution around the spawn;
    /// `None` (Legacy) spawns bare, byte-identical to before round 11.
    carried: Option<crate::scope::CarriedAttribution>,
}

impl AuthorizedLaunch {
    /// Spawn `fut` under the resolved authority.
    pub(super) fn spawn<F, T>(self, fut: F) -> tokio::task::JoinHandle<T>
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        spawn_under_authority(self.carried, fut)
    }
}

/// The dispatcher's whole fire-time authority step, as ONE seam: read the
/// task's facts, ask `resolve`, map the verdict. `dispatch_once` passes
/// `scope::authority::resolve`; tests pass `resolve_with` over their own
/// store (no lib test may install the global users store `resolve` reads),
/// so the chain a test drives is the production chain.
///
/// `None` = do not claim this tick, already acted on:
/// - facts unreadable, or `Unknown`: ruling R-a / 判据 §8 — NOT a failure.
///   The task is not failed, not paused, not retried-as-failed and releases
///   no dependents; it stays `Pending` and is asked about again next tick.
/// - `Refused`: parked `Paused`, with the reason and the checked principal
///   named (§14).
pub(super) async fn authorize_claim<R>(
    resolve: R,
    teams: &dyn crate::teams::TeamStore,
    sessions: &dyn crate::gateway::session_store::SessionStore,
    tasks: &dyn CoordTaskStore,
    task: &CoordTask,
) -> Option<AuthorizedLaunch>
where
    R: Fn(FireSubject<'_>) -> FireAuthority,
{
    let facts = match task_fire_facts(teams, sessions, task).await {
        Ok(facts) => facts,
        Err(e) => {
            tracing::warn!(task_id = %task.id, error = %e,
                "dispatcher: authority unknown (owner row unreadable); task left pending");
            return None;
        }
    };
    match resolve(facts.subject()) {
        FireAuthority::Legacy => Some(AuthorizedLaunch { carried: None }),
        FireAuthority::Granted(granted) => Some(AuthorizedLaunch {
            carried: Some(granted.carried()),
        }),
        FireAuthority::Refused(refusal) => {
            pause_for_refused_authority(tasks, task, refusal, facts.checked_principal()).await;
            None
        }
        unknown @ FireAuthority::Unknown(_) => {
            tracing::warn!(task_id = %task.id, reason = ?unknown.reason(),
                "dispatcher: authority unknown; task left pending");
            None
        }
    }
}

/// The text a refused task carries on its `result`, where the task drawer
/// shows it. Starts with [`FireAuthority::reason`]'s wording, names the
/// person that was checked, and gives the exit that actually exists for
/// that refusal:
/// - Deactivated: an administrator reactivates the person, then the task is
///   set back to pending (the next tick asks again).
/// - Gone: the person no longer exists and the author is pinned, so no
///   resume can ever succeed and there is no verb that reassigns a task's
///   author — stop it (cancel) and create it again as a current user.
fn refusal_text(refusal: RefusalReason, checked: Option<(&'static str, &str)>) -> String {
    let reason = FireAuthority::Refused(refusal)
        .reason()
        .unwrap_or_else(|| "authority refused".to_string());
    let (who, id) = match checked {
        Some((label, id)) => (format!("{label} `{id}`"), format!("`{id}`")),
        None => (
            "the checked principal".to_string(),
            "that person".to_string(),
        ),
    };
    match refusal {
        RefusalReason::Deactivated => format!(
            "Paused by the dispatcher: {reason} — {who} is deactivated. An administrator \
             can reactivate {id}; then set this task back to pending."
        ),
        RefusalReason::Gone => format!(
            "Paused by the dispatcher: {reason} — {who} no longer exists, so this task can \
             never run as them. Stop it (cancel) and create it again as a current user."
        ),
    }
}

/// A settled refusal (deactivated / deleted person): park the task `Paused`
/// with [`refusal_text`] on its `result`. Paused is never claimed and does
/// not satisfy dependents, so nothing downstream runs under the refused
/// authority.
///
/// Nulls `PAUSED_FROM_KEY` in the same write, like the two twin pause
/// surfaces (`team_task_control pause`, `teams.workflow` pause): the task is
/// Pending here, so a stale `paused_from` left by an earlier pause→retry
/// cycle would otherwise make a later resume restore it to WaitingReview.
pub(super) async fn pause_for_refused_authority(
    tasks: &dyn CoordTaskStore,
    task: &CoordTask,
    refusal: RefusalReason,
    checked: Option<(&'static str, &str)>,
) {
    let text = refusal_text(refusal, checked);
    let update = CoordTaskUpdate {
        status: Some(CoordTaskStatus::Paused),
        result: Some(text.clone()),
        metadata: Some(crate::agents::swarm::tasks::merge_metadata_patch(
            &task.metadata,
            serde_json::json!({
                crate::agents::swarm::tasks::PAUSED_FROM_KEY: serde_json::Value::Null,
            }),
        )),
        ..Default::default()
    };
    match tasks.update_task(&task.id, update).await {
        Ok(_) => tracing::warn!(task_id = %task.id, reason = %text,
            "dispatcher: task authority refused at claim time; task paused"),
        Err(e) => tracing::warn!(task_id = %task.id, error = %e,
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

    /// A users store where `u-alice` is an active Admin, `u-bob` an active
    /// Member and `u-walled` a deactivated Member (`u-owner` is bootstrapped).
    fn users() -> SecurityStore {
        let users = SecurityStore::in_memory().unwrap();
        users
            .create_user("u-alice", "Alice", UserRole::Admin)
            .unwrap();
        users.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        users
            .create_user("u-walled", "Walled", UserRole::Member)
            .unwrap();
        users
            .update_user("u-walled", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        users
    }

    /// A pending, dispatcher-managed task with NO ambient person at creation
    /// (an internal producer), so it carries no author.
    async fn authorless_task(coord: &SqliteCoordTaskStore, team_id: Option<String>) -> CoordTask {
        coord
            .create_task(NewCoordTask {
                team_id,
                subject: "s".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({ "managed_by": "dispatcher" }),
            })
            .await
            .unwrap()
    }

    /// M1: the refusal pause is the third pause surface, and like its two
    /// twins it nulls a stale `paused_from` in the same write — otherwise a
    /// later `team_task_control resume` restores the task to WaitingReview.
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
                metadata: serde_json::json!({
                    "managed_by": "dispatcher",
                    crate::agents::swarm::tasks::PAUSED_FROM_KEY: "waiting_review",
                }),
            })
            .await
            .unwrap();

        pause_for_refused_authority(
            &coord,
            &created,
            RefusalReason::Deactivated,
            Some(("task author", "u-bob")),
        )
        .await;

        let after = coord.get_task(&created.id).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Paused);
        assert!(after
            .result
            .as_deref()
            .is_some_and(|r| r.contains("principal deactivated")));
        assert!(
            after
                .metadata
                .get(crate::agents::swarm::tasks::PAUSED_FROM_KEY)
                .is_none(),
            "a stale paused_from must be nulled in the pausing write: {}",
            after.metadata
        );
        assert_eq!(after.metadata["managed_by"], "dispatcher");
    }

    /// Ruling (b) / 判据 §4, review I3: the GRANT the resolver produced is the
    /// one the member run executes under — not `Granted::legacy`, not a bare
    /// spawn. Drives the one production seam end to end from the stored rows:
    /// `create_task` stamps Bob, `authorize_claim` reads Alice's room off the
    /// team row and asks the injected resolver, which caps Bob (a member) at
    /// `member`, and the returned `AuthorizedLaunch` is what `dispatch_once`
    /// spawns. The distinct author (Bob, not the room's owner Alice) and the
    /// distinct role (`member`, not the absent = operator default) must both
    /// arrive, with the room's scope.
    #[tokio::test]
    async fn a_granted_claim_launches_under_the_resolved_member_authority() {
        let users = users();
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions(&dir);
        let team = team_owned_by(
            &teams,
            "u-alice",
            crate::scope::ScopeId::Project("p-room".into()),
        )
        .await;
        let t = authored_task(&coord, &team.id, "u-bob").await;

        let Some(launch) = authorize_claim(
            |s| resolve_with(Some(&users), &s),
            &teams,
            &sessions,
            &coord,
            &t,
        )
        .await
        else {
            panic!("an active member author must be admitted");
        };
        let (m, admitted) = launch
            .spawn(async { run_metadata_and_fence(Some(vec!["u-alice".into()])) })
            .await
            .unwrap();

        assert_eq!(m.get("caller_role").map(String::as_str), Some("member"));
        assert_eq!(
            m.get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
                .map(String::as_str),
            Some("u-bob")
        );
        assert_eq!(
            crate::scope::scope_from_metadata(&m),
            Some(crate::scope::ScopeAttribution {
                owner_user_id: "u-alice".into(),
                scope: crate::scope::ScopeId::Project("p-room".into()),
            })
        );
        assert!(!admitted, "an agent fenced to Alice must refuse Bob's task");
    }

    /// Review I4 (AM-1): a grant with a named author but NO scope — a legacy
    /// NULL-owner team with no origin session. The person must be the author
    /// for BOTH the `allowed_users` fence (`ambient_actor`) and the spend floor
    /// (`Principal::from_person(ambient_principal())`). Reading the person
    /// through `scope::ambient_room_author` (the transcript byline) answers
    /// `None` with no Project scope, so the fence saw nobody and admitted
    /// while spend charged the author.
    #[tokio::test]
    async fn a_scopeless_grant_names_its_author_to_the_fence_and_to_spend() {
        let users = users();
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions(&dir);
        let legacy = teams
            .create_team(NewTeam {
                name: "legacy".into(),
                description: String::new(),
                leader_id: "lead".into(),
            })
            .await
            .unwrap();
        let t = authored_task(&coord, &legacy.id, "u-bob").await;

        let Some(launch) = authorize_claim(
            |s| resolve_with(Some(&users), &s),
            &teams,
            &sessions,
            &coord,
            &t,
        )
        .await
        else {
            panic!("an active member author must be admitted");
        };
        let (actor, spend, admitted) = launch
            .spawn(async {
                let actor = crate::gateway::visibility::ambient_actor();
                let spend = crate::spend::Principal::from_person(
                    crate::gateway::visibility::ambient_principal(),
                );
                let allowed = vec!["u-alice".to_string()];
                let admitted = crate::config::types::agent_admits_user(
                    Some(allowed.as_slice()),
                    actor.as_deref(),
                );
                (actor, spend, admitted)
            })
            .await
            .unwrap();

        assert_eq!(
            actor.as_deref(),
            Some("u-bob"),
            "the fence must see the author"
        );
        assert_eq!(spend, crate::spend::Principal::User("u-bob".to_string()));
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
        let sessions = sessions(&dir);
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
        let facts = task_fire_facts(&teams, &sessions, &t).await.unwrap();
        assert!(
            matches!(
                resolve_with(Some(&users), &facts.subject()),
                FireAuthority::Granted(_)
            ),
            "u-owner rows resolve Granted"
        );

        for allowed in [
            Some(vec![OWNER_USER_ID.to_string()]),
            None,
            Some(Vec::new()),
        ] {
            let Some(launch) = authorize_claim(
                |s| resolve_with(Some(&users), &s),
                &teams,
                &sessions,
                &coord,
                &t,
            )
            .await
            else {
                panic!("an active admin must launch");
            };
            let fence = allowed.clone();
            let (m, admitted) = launch
                .spawn(async move { run_metadata_and_fence(fence) })
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
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions(&dir);
        let created = authorless_task(&coord, None).await;

        let launch = authorize_claim(
            |_| FireAuthority::Unknown("disk I/O error".into()),
            &teams,
            &sessions,
            &coord,
            &created,
        )
        .await;

        assert!(launch.is_none(), "an unknown answer must not launch");
        let after = coord.get_task(&created.id).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Pending);
        assert_eq!(after.result, None, "an unknown answer writes no outcome");
    }

    /// A settled refusal at the claim: held AND parked Paused, naming the
    /// author it checked and the way out (reactivate).
    #[tokio::test]
    async fn a_refused_claim_is_held_and_paused_with_the_principal_named() {
        let users = users();
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions(&dir);
        let team = team_owned_by(
            &teams,
            OWNER_USER_ID,
            crate::scope::ScopeId::Project("p-room".into()),
        )
        .await;
        let t = authored_task(&coord, &team.id, "u-walled").await;

        let launch = authorize_claim(
            |s| resolve_with(Some(&users), &s),
            &teams,
            &sessions,
            &coord,
            &t,
        )
        .await;

        assert!(launch.is_none());
        let after = coord.get_task(&t.id).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Paused);
        let result = after.result.unwrap_or_default();
        assert!(result.contains("principal deactivated"), "{result}");
        assert!(result.contains("task author `u-walled`"), "{result}");
        assert!(result.contains("reactivate"), "{result}");
    }

    /// Review I2: a deleted principal cannot be reactivated, so the text must
    /// not say so — it names the person and the real exit (stop the task and
    /// create it again; no verb reassigns a task's pinned author).
    #[tokio::test]
    async fn a_gone_author_is_paused_without_reactivate_advice() {
        let users = users();
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions(&dir);
        let team = team_owned_by(
            &teams,
            OWNER_USER_ID,
            crate::scope::ScopeId::Project("p-room".into()),
        )
        .await;
        let t = authored_task(&coord, &team.id, "u-ghost").await;

        let launch = authorize_claim(
            |s| resolve_with(Some(&users), &s),
            &teams,
            &sessions,
            &coord,
            &t,
        )
        .await;

        assert!(launch.is_none());
        let after = coord.get_task(&t.id).await.unwrap().unwrap();
        assert_eq!(after.status, CoordTaskStatus::Paused);
        let result = after.result.unwrap_or_default();
        assert!(result.contains("principal gone"), "{result}");
        assert!(result.contains("task author `u-ghost`"), "{result}");
        assert!(result.contains("cancel"), "{result}");
        assert!(
            !result.contains("reactivat"),
            "a deleted person cannot be reactivated: {result}"
        );
    }

    /// With no author the OWNER is the person checked, and the refusal says
    /// so — "team owner", not "task author".
    #[tokio::test]
    async fn an_authorless_refusal_names_the_team_owner() {
        let users = users();
        let teams = teams().await;
        let coord = coord().await;
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions(&dir);
        let team = team_owned_by(
            &teams,
            "u-walled",
            crate::scope::ScopeId::Personal("u-walled".into()),
        )
        .await;
        let t = authorless_task(&coord, Some(team.id.clone())).await;

        let launch = authorize_claim(
            |s| resolve_with(Some(&users), &s),
            &teams,
            &sessions,
            &coord,
            &t,
        )
        .await;

        assert!(launch.is_none());
        let result = coord
            .get_task(&t.id)
            .await
            .unwrap()
            .unwrap()
            .result
            .unwrap_or_default();
        assert!(result.contains("team owner `u-walled`"), "{result}");
    }

    /// Ruling (b), the one seam no behaviour test can reach: `dispatch_once`
    /// passes the GLOBAL resolver, which no lib test may install. Pinned at
    /// the source: the claim loop hands exactly `scope::authority::resolve`
    /// to `authorize_claim` and launches the member run through the
    /// `AuthorizedLaunch` it returned — whose carrier it cannot construct or
    /// replace. A bare `tokio::spawn` (the N1 shape), a direct
    /// `spawn_under_authority` or any other resolver goes red here.
    /// Whitespace is stripped first so formatting cannot move the pin.
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
            .flat_map(|l| l.chars())
            .filter(|c| !c.is_whitespace())
            .collect();
        assert_eq!(
            body.matches(
                "authority::authorize_claim(|subject|crate::scope::authority::resolve(&subject),"
            )
            .count(),
            1,
            "the claim loop authorizes through the production resolver exactly once"
        );
        assert_eq!(
            body.matches("authority::resolve(").count(),
            1,
            "no second resolver call beside the seam"
        );
        assert_eq!(
            body.matches("launch.spawn(").count(),
            1,
            "the member run is launched through the authorized launch"
        );
        assert_eq!(
            body.matches("tokio::spawn(").count(),
            0,
            "a bare spawn (N1)"
        );
        assert_eq!(
            body.matches("spawn_under_authority(").count(),
            0,
            "the claim loop must not pick its own carrier"
        );
    }
}
