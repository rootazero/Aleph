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
/// Delegates the whole rule — including adoption-by-absence — to
/// [`crate::gateway::visibility::ambient_owner_visible`]. Do not inline an
/// `owner == caller` comparison here or at any call site.
#[must_use]
pub fn team_visible(owner_user_id: Option<&str>) -> bool {
    crate::gateway::visibility::ambient_owner_visible(owner_user_id)
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
            Ok(Some(t)) if team_visible(t.owner_user_id.as_deref()) => Ok(()),
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
            .filter(|t| team_visible(t.owner_user_id.as_deref())))
    }

    async fn get_team_by_name(&self, name: &str) -> crate::error::Result<Option<Team>> {
        Ok(self
            .inner
            .get_team_by_name(name)
            .await?
            .filter(|t| team_visible(t.owner_user_id.as_deref())))
    }

    async fn list_teams(&self) -> crate::error::Result<Vec<TeamSummary>> {
        let mut teams = self.inner.list_teams().await?;
        teams.retain(|t| team_visible(t.owner_user_id.as_deref()));
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
        teams.retain(|t| team_visible(t.owner_user_id.as_deref()));
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
}
