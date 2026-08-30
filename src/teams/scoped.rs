//! Ownership enforcement for teams — the single chokepoint (P1 data isolation).
//!
//! [`ScopedTeamStore`] decorates a [`TeamStore`] so that a team belonging to a
//! different user is indistinguishable from a team that does not exist: reads
//! return `None` / drop the row, and addressed writes return the store's own
//! [`team_not_found`] error verbatim.
//!
//! # Why a decorator and not a check per call site
//!
//! Teams are reachable from two surfaces with nothing in common:
//!
//! - the `teams.*` gateway RPCs (37 methods, Panel-driven), and
//! - the `team_*` builtin tools, called by a model mid-run.
//!
//! Enforcing at each call site means ~50 places, each individually plausible,
//! and the P1 final review already demonstrated what that costs: a sweep that
//! enumerates handlers by parameter shape is structurally blind to the ones
//! that address the same data another way. Wrapping the trait puts the
//! predicate on the ONE path both surfaces must cross. Anything that reaches a
//! team without going through `Arc<dyn TeamStore>` is, by construction, not a
//! path that exists.
//!
//! The gateway still gates explicitly on top of this (see
//! `gateway::handlers::teams::visibility`) — not to re-derive the predicate but
//! to shape the denial as the byte-identical `not_found` JSON-RPC response the
//! rest of P1 uses, and to cover the handlers that address a team through the
//! *task* DAG (`coord_tasks`, a different database this store cannot see).
//!
//! # What it deliberately does NOT gate
//!
//! `get_members` / `take_auto_name_flag` and friends take a `team_id` and are
//! gated. `create_team` is not gated (there is nothing to compare against yet);
//! it is stamped instead, inside `SqliteTeamStore::create_team`, so every
//! creation path — RPC, tool, template materialization — lands owned.

use async_trait::async_trait;

use crate::sync_primitives::Arc;

use super::store::{team_not_found, TeamStore};
use super::types::{NewTeam, NewTeamMember, Team, TeamMember, TeamSummary};

/// Whether a team's stamp admits the current execution context.
///
/// The rule body is the SAME one sessions use —
/// [`crate::gateway::visibility::owner_and_scope_visible_to`]: a
/// project-scoped team asks the room's roster, anything else falls through
/// to owner-equality (with adoption-by-absence for legacy rows). The actor
/// is [`crate::gateway::visibility::ambient_actor`] — the SPEAKER in a room,
/// not `ambient_owner`, which there answers the room's creator identically
/// for every member. Those two facts together are what let a room-created
/// team belong to the room (round-3's deferred teams semantics, landed in
/// round-5 with the `teams.scope_id` column); swapping in the resolver alone
/// — without the column — was the change round-3 refused to half-do, because
/// a room team stamped only with its creator would vanish for every other
/// member.
///
/// `None` actor (cron / background sweep / in-process test) is unrestricted,
/// matching every sibling predicate. Do not inline an `owner == caller`
/// comparison here or at any call site.
#[must_use]
pub fn team_visible(owner_user_id: Option<&str>, scope_id: Option<&str>) -> bool {
    match crate::gateway::visibility::ambient_actor() {
        None => true,
        Some(actor) => {
            crate::gateway::visibility::owner_and_scope_visible_to(owner_user_id, scope_id, &actor)
        }
    }
}

/// Whether the team owning a coord task admits the current execution context —
/// the tool-side twin of `gateway::handlers::teams::visibility::gate_task`.
///
/// **Nine** tools address a task by id and resolve it in `coord_tasks` alone,
/// so the [`ScopedTeamStore`] decorator never sees them; they call this after
/// fetching the task and refuse with their own "task not found" error. The
/// gateway's twenty task-addressed RPCs are the same shape, and the same rule:
/// a task with no team at all reads as an unstamped record (the legacy
/// owner's), never as public.
///
/// The count was six until 2026-08-08, and the three that were missing say
/// something about how this kind of gap survives:
///
/// - `task_review` DID resolve the team — and then fed the answer to
///   `is_authorized(caller, leader)`, where the decorator's fail-closed
///   `Ok(None)` became `leader == None` became "no leader, so anyone may
///   review". **A fail-closed answer consumed as a value inverts into a
///   permission**; the gate has to run before the fold, not inside it.
/// - `task_update` and `task_wait` had no gate at all, and were constructed
///   three lines below `TaskCommentTool` — which already took the store — in
///   the same function, with `config.team_store` in scope. Nothing prevented
///   it; nobody was asked.
///
/// A new tool that reaches a coord task by id owes this call. On a LIST
/// surface it is a retain (`task_wait`, `task_list`); on an addressed one it is
/// a refusal shaped exactly like "no such task" (`task_update`,
/// `task_review`); on a CREATION surface that names a team it is the same
/// refusal before the write (`task_create`).
///
/// ⚠️ Naming the tools in prose is how `task_list` stayed open for a round
/// after this paragraph named it. The census is now
/// `tests::every_coord_task_tool_answers_the_ownership_question`, which reads
/// the source and fails by name.
///
/// `store == None` means no team database is wired in this deployment, so no
/// coord task can belong to a team. That is unrestricted by construction, NOT
/// a fail-closed case — treating it as a denial would take the task tools
/// offline for every deployment that runs coord tasks without teams.
pub async fn task_team_reachable(
    store: Option<&Arc<dyn TeamStore>>,
    team_id: Option<&str>,
) -> bool {
    let Some(store) = store else { return true };
    match team_id {
        None => team_visible(None, None),
        // Fetch the team, then check ownership/scope visibility. Just
        // checking existence would admit any team the caller can name by
        // id, which inverts the gate into a permission grant — see the
        // module-level docs for the prior `task_review` regression in the
        // same shape. Fail closed on store error (a locked SQLite is a
        // denial, never a pass).
        Some(id) => match store.get_team(id).await {
            Ok(Some(t)) => team_visible(t.owner_user_id.as_deref(), t.scope_id.as_deref()),
            Ok(None) => false,
            Err(_) => false,
        },
    }
}

/// A [`TeamStore`] that only serves teams the current context owns.
pub struct ScopedTeamStore {
    inner: Arc<dyn TeamStore>,
}

impl ScopedTeamStore {
    /// Wrap `inner`. The returned handle is the only one that should ever be
    /// published — see `builder::agent_init::coord_stores`, the single
    /// construction site.
    #[must_use]
    pub fn wrap(inner: Arc<dyn TeamStore>) -> Arc<dyn TeamStore> {
        Arc::new(Self { inner })
    }

    /// Gate for an addressed write.
    ///
    /// Fails closed: a store error is a denial, never a pass. `.ok().flatten()`
    /// here would turn a locked SQLite connection into an open door.
    async fn admits(&self, team_id: &str) -> crate::error::Result<()> {
        match self.inner.get_team(team_id).await {
            Ok(Some(t)) if team_visible(t.owner_user_id.as_deref(), t.scope_id.as_deref()) => {
                Ok(())
            }
            Ok(_) => Err(team_not_found(team_id)),
            Err(e) => Err(e),
        }
    }
}

#[async_trait]
impl TeamStore for ScopedTeamStore {
    async fn create_team(&self, input: NewTeam) -> crate::error::Result<Team> {
        // Not gated — stamped. See the module doc.
        self.inner.create_team(input).await
    }

    async fn get_team(&self, id: &str) -> crate::error::Result<Option<Team>> {
        Ok(self
            .inner
            .get_team(id)
            .await?
            .filter(|t| team_visible(t.owner_user_id.as_deref(), t.scope_id.as_deref())))
    }

    async fn get_team_by_name(&self, name: &str) -> crate::error::Result<Option<Team>> {
        Ok(self
            .inner
            .get_team_by_name(name)
            .await?
            .filter(|t| team_visible(t.owner_user_id.as_deref(), t.scope_id.as_deref())))
    }

    async fn list_teams(&self) -> crate::error::Result<Vec<TeamSummary>> {
        let mut teams = self.inner.list_teams().await?;
        teams.retain(|t| team_visible(t.owner_user_id.as_deref(), t.scope_id.as_deref()));
        Ok(teams)
    }

    async fn disband_team(&self, id: &str) -> crate::error::Result<()> {
        self.admits(id).await?;
        self.inner.disband_team(id).await
    }

    async fn delete_team(&self, id: &str) -> crate::error::Result<()> {
        self.admits(id).await?;
        self.inner.delete_team(id).await
    }

    async fn add_member(&self, input: NewTeamMember) -> crate::error::Result<TeamMember> {
        self.admits(&input.team_id).await?;
        self.inner.add_member(input).await
    }

    async fn get_members(&self, team_id: &str) -> crate::error::Result<Vec<TeamMember>> {
        self.admits(team_id).await?;
        self.inner.get_members(team_id).await
    }

    async fn remove_member(&self, team_id: &str, agent_id: &str) -> crate::error::Result<()> {
        self.admits(team_id).await?;
        self.inner.remove_member(team_id, agent_id).await
    }

    async fn get_agent_teams(&self, agent_id: &str) -> crate::error::Result<Vec<TeamSummary>> {
        let mut teams = self.inner.get_agent_teams(agent_id).await?;
        teams.retain(|t| team_visible(t.owner_user_id.as_deref(), t.scope_id.as_deref()));
        Ok(teams)
    }

    async fn set_protocol(
        &self,
        team_id: &str,
        protocol: Option<String>,
    ) -> crate::error::Result<()> {
        self.admits(team_id).await?;
        self.inner.set_protocol(team_id, protocol).await
    }

    async fn rename_team(&self, id: &str, name: &str) -> crate::error::Result<()> {
        self.admits(id).await?;
        self.inner.rename_team(id, name).await
    }

    async fn set_name_auto(&self, id: &str, value: bool) -> crate::error::Result<()> {
        self.admits(id).await?;
        self.inner.set_name_auto(id, value).await
    }

    async fn take_auto_name_flag(&self, id: &str) -> crate::error::Result<bool> {
        self.admits(id).await?;
        self.inner.take_auto_name_flag(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{with_scope, ScopeAttribution};
    use crate::teams::SqliteTeamStore;

    async fn store() -> Arc<dyn TeamStore> {
        let s = SqliteTeamStore::new(rusqlite::Connection::open_in_memory().unwrap());
        s.migrate().await.unwrap();
        ScopedTeamStore::wrap(Arc::new(s))
    }

    fn new_team(name: &str) -> NewTeam {
        NewTeam {
            name: name.to_string(),
            description: String::new(),
            leader_id: "leader".to_string(),
        }
    }

    async fn create_as(s: &Arc<dyn TeamStore>, user: &str, name: &str) -> Team {
        with_scope(
            Some(ScopeAttribution::personal(user)),
            s.create_team(new_team(name)),
        )
        .await
        .unwrap()
    }

    /// The acceptance shape: alice creates, bob cannot see it in any read and
    /// cannot reach it with any addressed write.
    #[tokio::test]
    async fn a_teams_team_is_invisible_to_b() {
        let s = store().await;
        let t = create_as(&s, "u-alice", "Alpha").await;
        assert_eq!(t.owner_user_id.as_deref(), Some("u-alice"));

        let bob = Some(ScopeAttribution::personal("u-bob"));

        assert!(with_scope(bob.clone(), s.get_team(&t.id))
            .await
            .unwrap()
            .is_none());
        assert!(with_scope(bob.clone(), s.get_team_by_name("Alpha"))
            .await
            .unwrap()
            .is_none());
        assert!(with_scope(bob.clone(), s.list_teams())
            .await
            .unwrap()
            .is_empty());
        assert!(with_scope(bob.clone(), s.get_agent_teams("leader"))
            .await
            .unwrap()
            .is_empty());
        assert!(with_scope(bob.clone(), s.get_members(&t.id)).await.is_err());
        assert!(with_scope(bob.clone(), s.rename_team(&t.id, "Pwned"))
            .await
            .is_err());
        assert!(with_scope(bob.clone(), s.disband_team(&t.id))
            .await
            .is_err());
        assert!(with_scope(bob, s.delete_team(&t.id)).await.is_err());

        // ...and alice still has an intact, un-renamed, active team.
        let alice = Some(ScopeAttribution::personal("u-alice"));
        let seen = with_scope(alice, s.get_team(&t.id)).await.unwrap().unwrap();
        assert_eq!(seen.name, "Alpha");
    }

    /// Round-5, the room half of the teams semantics: a team created INSIDE a
    /// project room is stamped `scope_id = project:<id>` and belongs to the
    /// ROOM — every roster member sees it, runs it, and lists it, while a
    /// non-member does not. This is the change round-3 refused to half-do
    /// (resolver swap without the column would have hidden room-created teams
    /// from the room).
    #[tokio::test]
    async fn a_room_created_team_belongs_to_the_room() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let projects =
            crate::projects::ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
        projects.create_schema().unwrap();
        let room = projects.create("room", Some("u-alice"), None).unwrap();
        projects.add_member(&room.id, "u-bob").unwrap();

        let s = store().await;
        let room_attr = Some(ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: crate::scope::ScopeId::Project(room.id.clone()),
        });
        let t = with_scope(room_attr.clone(), s.create_team(new_team("RoomTeam")))
            .await
            .unwrap();
        assert_eq!(
            t.scope_id.as_deref(),
            Some(format!("project:{}", room.id)).as_deref(),
            "a team created inside a room run must carry the room's scope stamp"
        );
        assert_eq!(t.owner_user_id.as_deref(), Some("u-alice"));

        // Bob — a member, NOT the creator — speaking in the room: sees it,
        // lists it, reaches its members. The speaker is the actor
        // (`with_room_author`), not the room's creator.
        let bob_speaking = crate::scope::with_room_author(
            Some("u-bob".to_string()),
            with_scope(room_attr.clone(), async {
                let direct = s.get_team(&t.id).await.unwrap();
                let listed = s.list_teams().await.unwrap();
                let members = s.get_members(&t.id).await.is_ok();
                (direct, listed, members)
            }),
        )
        .await;
        assert!(bob_speaking.0.is_some(), "a member must see a room team");
        assert_eq!(bob_speaking.1.len(), 1, "a member must list a room team");
        assert!(bob_speaking.2, "a member must reach a room team's members");

        // Carol — not on the roster — gets the fail-closed answer even with
        // the room's scope ambient.
        let carol = crate::scope::with_room_author(
            Some("u-carol".to_string()),
            with_scope(room_attr, s.get_team(&t.id)),
        )
        .await
        .unwrap();
        assert!(carol.is_none(), "a non-member must not see a room team");

        // And alice's PERSONAL team stays hers even when bob is the speaker
        // in a room they share — the scopeless/legacy arm falls through to
        // owner-equality against the SPEAKER, not the room's creator.
        let personal = create_as(&s, "u-alice", "AlicePersonal").await;
        assert_eq!(personal.scope_id.as_deref(), Some("personal:u-alice"));
        let bob_in_room = crate::scope::with_room_author(
            Some("u-bob".to_string()),
            with_scope(
                Some(ScopeAttribution {
                    owner_user_id: "u-alice".to_string(),
                    scope: crate::scope::ScopeId::Project(room.id.clone()),
                }),
                s.get_team(&personal.id),
            ),
        )
        .await
        .unwrap();
        assert!(
            bob_in_room.is_none(),
            "the room must not leak the creator's personal teams to members"
        );
    }

    /// No existence oracle: the denial is byte-identical to a genuinely
    /// unknown id, so bob cannot probe for which team ids exist.
    #[tokio::test]
    async fn a_denial_reads_exactly_like_a_missing_team() {
        let s = store().await;
        let t = create_as(&s, "u-alice", "Alpha").await;
        let bob = Some(ScopeAttribution::personal("u-bob"));

        let denied = with_scope(bob.clone(), s.rename_team(&t.id, "x"))
            .await
            .unwrap_err()
            .to_string()
            .replace(&t.id, "<id>");
        let missing = with_scope(bob, s.rename_team("no-such-team", "x"))
            .await
            .unwrap_err()
            .to_string()
            .replace("no-such-team", "<id>");
        assert_eq!(denied, missing);
    }

    /// Team names are unique PER OWNER, not globally. A global constraint is
    /// a cross-user existence oracle — bob would be told alice's team name is
    /// taken, learning both that it exists and what it is called, and be
    /// blocked from a name he cannot see.
    #[tokio::test]
    async fn two_users_may_each_have_a_team_of_the_same_name() {
        let s = store().await;
        create_as(&s, "u-alice", "Roadmap").await;

        let bobs = with_scope(
            Some(ScopeAttribution::personal("u-bob")),
            s.create_team(new_team("Roadmap")),
        )
        .await
        .expect("bob may name his own team Roadmap");
        assert_eq!(bobs.owner_user_id.as_deref(), Some("u-bob"));

        // ...but one user still cannot hold two active teams of one name.
        let dup = with_scope(
            Some(ScopeAttribution::personal("u-bob")),
            s.create_team(new_team("Roadmap")),
        )
        .await;
        assert!(dup.is_err(), "the per-owner constraint must still bite");
    }

    /// The legacy half of the same rule: unowned rows collapse onto the
    /// effective owner, so a single-user database keeps the exact constraint
    /// it had before the index was re-keyed. Without the `COALESCE`, SQLite's
    /// distinct-NULLs rule would drop the constraint for every legacy row.
    #[tokio::test]
    async fn unowned_teams_still_collide_with_each_other() {
        let s = store().await;
        s.create_team(new_team("Shared")).await.unwrap();
        assert!(
            s.create_team(new_team("Shared")).await.is_err(),
            "two unowned teams of one name must still collide"
        );
    }

    /// Zero-change guarantee: a caller with no ambient owner (cron, internal
    /// sweep, in-process test) is unrestricted, exactly as before P1.
    #[tokio::test]
    async fn an_unscoped_caller_is_unrestricted() {
        let s = store().await;
        let t = create_as(&s, "u-alice", "Alpha").await;

        assert!(s.get_team(&t.id).await.unwrap().is_some());
        assert_eq!(s.list_teams().await.unwrap().len(), 1);
        s.rename_team(&t.id, "Renamed").await.unwrap();
    }

    /// Adoption by absence: a team created with no ambient owner belongs to
    /// the legacy operator, so a member cannot reach it but the owner can.
    #[tokio::test]
    async fn a_legacy_team_belongs_to_the_owner() {
        let s = store().await;
        let t = s.create_team(new_team("Legacy")).await.unwrap();
        assert!(t.owner_user_id.is_none(), "no ambient owner ⇒ no stamp");

        assert!(
            with_scope(Some(ScopeAttribution::personal("u-bob")), s.get_team(&t.id))
                .await
                .unwrap()
                .is_none()
        );

        let owner = crate::gateway::security::store::OWNER_USER_ID;
        assert!(
            with_scope(Some(ScopeAttribution::personal(owner)), s.get_team(&t.id))
                .await
                .unwrap()
                .is_some()
        );
    }

    // -- the coord-task census ---------------------------------------------

    /// Tools that hold a `CoordTaskStore` and are NOT expected to call
    /// [`task_team_reachable`], with the reason each is exempt.
    ///
    /// Everything else holding that store reaches a coord task by a bare id in
    /// a database the `ScopedTeamStore` decorator cannot see, and therefore
    /// owes the call. Prose said so for a full round while `task_list` — named
    /// in that prose — returned every principal's task board.
    const COORD_STORE_EXEMPTIONS: &[(&str, &str)] = &[
        (
            "src/builtin_tools/team/delegate.rs",
            "addresses the team through Arc<dyn TeamStore>, i.e. the decorator itself",
        ),
        (
            "src/builtin_tools/team/from_template.rs",
            "same: resolves via the decorated TeamStore before touching coord tasks",
        ),
        (
            "src/builtin_tools/team/snapshot.rs",
            "same: passes self.team_store into the resolution it performs",
        ),
        (
            "src/builtin_tools/team/status.rs",
            "same: reads the team through the decorated store first",
        ),
        (
            "src/builtin_tools/workflow_tool.rs",
            "holds an Option<Arc<dyn TeamStore>> and gates through it directly",
        ),
    ];

    /// Source-level: the decorator is structurally blind to these call sites,
    /// so the only thing that can notice a missing gate is a census that reads
    /// the files.
    #[test]
    fn every_coord_task_tool_answers_the_ownership_question() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builtin_tools");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 20, "walk found suspiciously few tool sources");

        let mut offenders = Vec::new();
        for file in files {
            let rel = file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            if !text.contains("Arc<dyn CoordTaskStore>") {
                continue;
            }
            if text.contains("task_team_reachable") {
                continue;
            }
            if COORD_STORE_EXEMPTIONS.iter().any(|(f, _)| *f == rel) {
                continue;
            }
            offenders.push(rel);
        }

        assert!(
            offenders.is_empty(),
            "these hold a CoordTaskStore, reach tasks by a bare id in a database the \
             ScopedTeamStore decorator cannot see, and never ask whose team it is. Add the \
             `task_team_reachable` call (retain on a list surface, not-found-shaped refusal on an \
             addressed or creating one) or name the file in COORD_STORE_EXEMPTIONS with the \
             reason it resolves through the decorated TeamStore instead:\n  {offenders:?}"
        );

        // A census that lists things which stopped existing stops being read.
        for (file, reason) in COORD_STORE_EXEMPTIONS {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("exemption names a missing file: {file}"));
            assert!(
                text.contains("Arc<dyn CoordTaskStore>"),
                "exemption is stale — {file} no longer holds a CoordTaskStore ({reason})"
            );
        }
    }
}
