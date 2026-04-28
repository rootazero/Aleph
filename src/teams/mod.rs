//! Team management module.
//!
//! Provides types and a SQLite-backed store for managing teams of agents,
//! team membership, per-team task tracking, lifecycle management, and plan approval.

pub mod artifacts;
pub mod context;
pub mod events;
pub mod kanban;
pub mod lifecycle;
pub mod messages;
pub mod plans;
pub mod sessions;
pub mod store;
pub mod types;

#[cfg(test)]
pub mod integration_tests;

pub use artifacts::{ArtifactType, TaskArtifact, TaskStatus};
pub use events::{EventLogStore, SqliteEventLogStore, TeamEventLogger};
pub use kanban::{KanbanBoard, KanbanColumns, SqliteKanbanBoard};
pub use store::{SqliteTeamStore, TeamStore};
pub use types::{NewTeam, NewTeamMember, Team, TeamId, TeamMember, TeamStatus, TeamSummary};
