//! Team management module.
//!
//! Provides types and a SQLite-backed store for managing teams of agents,
//! team membership, and per-team task tracking.

pub mod artifacts;
pub mod context;
pub mod events;
pub mod messages;
pub mod roles;
pub mod sessions;
pub mod store;
pub mod types;

pub use store::{SqliteTeamStore, TeamStore};
pub use types::{
    NewTeam, NewTeamMember, Team, TeamId, TeamMember, TeamStatus, TeamSummary,
};
