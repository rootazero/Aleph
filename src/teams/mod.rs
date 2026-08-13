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
            let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
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
