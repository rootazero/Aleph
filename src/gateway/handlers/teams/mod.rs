//! Team Management Handlers
//!
//! RPC handlers for team CRUD and membership operations:
//! - teams.list: List all teams with summaries
//! - teams.get: Get full team detail (team + members + tasks)
//! - teams.disband: Mark a team as disbanded
//! - teams.delete: Permanently delete a disbanded team
//! - agents.teams: List all teams an agent belongs to
//! - `teams.list_tasks` / `teams.create_task` / `teams.update_task`: kanban-facing
//!   `CoordTask` operations
//! - teams.snapshot.{create,list,get,restore,delete}: snapshot lifecycle —
//!   thin direct surface in addition to the `team_snapshot` builtin tool
//!
//! This module was split from a single oversized `teams.rs` into cohesive
//! responsibility submodules. The public path (`...::handlers::teams::X`) is
//! preserved verbatim via the glob re-exports below.

mod canvas;
mod crud;
mod snapshot;
mod tasks;
pub mod visibility;
mod workflow;

#[cfg(test)]
mod tests;

pub use canvas::*;
pub use crud::*;
pub use snapshot::*;
pub use tasks::*;
pub use workflow::*;
