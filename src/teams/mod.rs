//! Team management module.
//!
//! Provides types and a SQLite-backed store for managing teams of agents,
//! team membership, per-team task tracking, plan approval, and an autonomous
//! task DAG dispatcher.

pub mod artifacts;
pub mod broadcast;
pub mod context;
pub mod dispatcher;
pub mod events;
pub mod leader_prompt;
pub mod member_provision;
pub mod messages;
pub mod notifier;
pub mod plans;
pub mod run_mode;
pub mod scoped;
pub mod sessions;
pub mod snapshots;
pub mod store;
pub mod templates;
pub mod types;
pub mod workflow_canvas;

#[cfg(test)]
pub mod integration_tests;

pub use artifacts::{ArtifactType, TaskArtifact, TaskStatus};
pub use broadcast::BroadcastConfig;
pub use dispatcher::{DispatcherConfig, TeamDispatcher};
pub use events::{EventLogStore, SqliteEventLogStore, TeamEventLogger};
pub use notifier::TeamNotifier;
pub use scoped::{task_team_reachable, team_visible, ScopedTeamStore};
pub use snapshots::{
    capture_snapshot, restore_snapshot, CreateSnapshotOutput, RestoreDiff, SnapshotMeta,
    SqliteSnapshotStore, TeamSnapshotPayload,
};
pub use store::{SqliteTeamStore, TeamStore};
pub use types::{
    acp_member_id, NewTeam, NewTeamMember, Team, TeamId, TeamMember, TeamMemberKind, TeamStatus,
    TeamSummary,
};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};

/// The two stores the deactivation freeze (and the `users.get` preview) need
/// to reach a principal's team tasks (round 11, N2).
///
/// A slot rather than a handler parameter: the `users.*` handlers are
/// registered at boot BEFORE the team stores exist, and
/// `BackgroundWorkHandles::from_globals` reads process globals for every
/// other leg.
///
/// `teams` is the RAW (unscoped) store: the freeze must see every team the
/// deactivated principal owns, not the teams visible to the admin running it.
/// That is also why [`background_stores`] is `pub(crate)` — this handle
/// bypasses [`ScopedTeamStore`] and must not become a second way in.
#[derive(Clone)]
pub struct TeamTaskStores {
    pub teams: crate::sync_primitives::Arc<dyn TeamStore>,
    pub tasks: crate::sync_primitives::Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>,
}

/// `ConsumerDecides`: the one consumer (`handlers::users`) reads absence as
/// "leg not measured" (`FrozenBackgroundWork::team_tasks: None`), which the
/// receipt and the CLI both say in their own sentence.
static BACKGROUND_STORES: CapabilitySlot<TeamTaskStores> =
    CapabilitySlot::new("teams/background-stores", MissingSemantics::ConsumerDecides);

/// The handle above, type-erased for the roster.
pub(crate) const fn background_stores_slot() -> &'static dyn SlotStatus {
    &BACKGROUND_STORES
}

/// Install at boot, once both stores opened. Idempotent.
pub fn install_background_stores(stores: TeamTaskStores) {
    let _ = BACKGROUND_STORES.install(stores);
}

/// Record that boot reached this slot and one of the two stores did not open.
pub fn decline_background_stores(because: &'static str) {
    BACKGROUND_STORES.decline(because);
}

/// Read the stores, if installed.
#[must_use]
pub(crate) fn background_stores() -> Option<TeamTaskStores> {
    BACKGROUND_STORES.get().cloned()
}

#[cfg(test)]
mod background_stores_slot_tests {
    use super::*;

    /// The roster id and the missing semantics the one consumer relies on:
    /// absence must read as "leg not measured", not fail anything open.
    #[test]
    fn the_background_stores_slot_pins_its_id_and_missing_semantics() {
        assert_eq!(background_stores_slot().id(), "teams/background-stores");
        assert!(
            matches!(
                background_stores_slot().missing(),
                MissingSemantics::ConsumerDecides
            ),
            "`teams/background-stores` is ConsumerDecides: `handlers::users` reports an \
             absent slot as an unmeasured leg"
        );
    }
}

#[cfg(test)]
mod agent_axis_census {
    /// Every teams face that RUNS an agent must ask that agent's
    /// `allowed_users` whether this caller may select it.
    ///
    /// `[agents.X.tool_permissions]` is a permission set chosen by naming an
    /// agent, so `allowed_users` is the fence on choosing. That fence lives at
    /// `handlers::agent::build_run_request`, which covers `chat.send` and
    /// `agent.run` — and covered nothing here: `team_create` and
    /// `team_delegate` are member-open, and both the dispatcher and the
    /// group-chat fan-out resolved their target straight out of the registry.
    /// A member refused `ops` at run start could name `ops` as a team member
    /// and get its permissions. Both steps legal, the pair equivalent — the
    /// two-step bypass §5.17 closed for `sessions_send`.
    ///
    /// The census is FILE-granular, and that is a real limitation worth
    /// stating rather than hiding: it proves each file that starts a member run
    /// asks the question somewhere, not that every site in it does. It is here
    /// to catch the next FACE — a fourth file that learns to run an agent —
    /// which is how all three of these were born. The two functions that
    /// actually start one are `dispatcher::runner::execute_member_task` and
    /// `broadcast::GroupChatExecutor::run_member`; a new one belongs in this
    /// list on the day it is written.
    #[test]
    fn every_teams_face_that_runs_an_agent_asks_its_allowed_users() {
        let faces: &[(&str, &str)] = &[
            (
                "teams/dispatcher/runner.rs",
                include_str!("dispatcher/runner.rs"),
            ),
            ("teams/broadcast/mod.rs", include_str!("broadcast/mod.rs")),
        ];
        for (path, src) in faces {
            // Production prefix only. Unanchored separator: `"\n#[cfg(test)]\n"`
            // matches nothing on a CRLF checkout, which would silently make
            // `prod` the whole file and let this test be satisfied by its own
            // assertion strings (§10).
            let prod = crate::utils::source_scan::production_prefix(src);
            assert!(
                prod.contains("get_allowed_users"),
                "{path}: this face starts a member run but never asks the target \
                 agent's `allowed_users` whether the caller may select it. \
                 Naming a restricted agent as a team member would then run it \
                 with its `tool_permissions` — the same two-step bypass \
                 `sessions_send` closed. Use `agent_registry.get_allowed_users` \
                 + `config::types::agent_admits_user` with \
                 `visibility::ambient_actor()` as the actor."
            );
            assert!(
                prod.contains("agent_admits_user"),
                "{path}: reads `allowed_users` but does not decide with \
                 `config::types::agent_admits_user` — that function owns what \
                 an unset or empty list means, and a second spelling of it is \
                 how the two answers drift apart."
            );
        }
    }
}
