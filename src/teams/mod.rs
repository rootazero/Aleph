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
