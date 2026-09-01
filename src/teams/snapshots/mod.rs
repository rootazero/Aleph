//! Team snapshots — point-in-time bundles of a team's config + members +
//! tasks + recent events. Restore is **dry-run by default** so callers can
//! inspect the diff before clobbering live state.
//!
//! Inspired by `ClawTeam`'s `SnapshotManager`. Adapted to Aleph's `SQLite` split:
//! snapshots live in the **coord task** database alongside `coord_tasks`
//! (not in `teams.db`) so the read path holds one lock for the bulk content.
//! No FK on `coord_tasks` — historical snapshots must survive task deletion.

use serde::{Deserialize, Serialize};

use crate::agents::swarm::tasks::CoordTask;
use crate::teams::types::{Team, TeamMember};

mod operations;
mod store;

#[cfg(test)]
mod tests;

pub use operations::{capture_snapshot, restore_snapshot};
pub use store::SqliteSnapshotStore;

// =============================================================================
// Snapshot record types
// =============================================================================

/// Metadata returned by `list_snapshots`. The full payload is not loaded —
/// callers explicitly `get_snapshot` when they need the body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub id: String,
    pub team_id: String,
    pub tag: String,
    pub created_at: i64,
    pub size_bytes: i64,
}

/// Full snapshot payload. Stored as JSON in `coord_team_snapshots.payload`.
///
/// `recent_events` is OPTIONAL — empty when the event log isn't available at
/// snapshot time. Tasks include their full state at the moment of capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSnapshotPayload {
    pub team: Team,
    pub members: Vec<TeamMember>,
    pub tasks: Vec<CoordTask>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateSnapshotOutput {
    pub snapshot_id: String,
    pub team_id: String,
    pub tag: String,
    pub created_at: i64,
    pub task_count: usize,
    pub member_count: usize,
    pub size_bytes: usize,
}

/// Diff summary returned by `restore` (always, with or without `dry_run`).
///
/// `edges_restored` counts the dependency edges that the restore step
/// re-attached to newly-created tasks. Edges targeting snapshot tasks not
/// present in `payload.tasks` are silently dropped; edges onto already-live
/// tasks are remapped to the live id. On `dry_run`, the count reflects what
/// *would* be restored.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreDiff {
    pub dry_run: bool,
    pub team_id: String,
    pub snapshot_id: String,
    pub current_task_count: usize,
    pub snapshot_task_count: usize,
    pub tasks_to_add: Vec<String>,
    pub tasks_to_update: Vec<String>,
    pub tasks_to_skip_active: Vec<String>,
    pub members_to_add: Vec<String>,
    pub members_to_remove: Vec<String>,
    #[serde(default)]
    pub edges_restored: usize,
    /// `true` if the restore had to recreate the team (because no live team
    /// row matched the snapshot's `team_id`). When `true`, `team_id` is a
    /// freshly-minted id, `original_team_id` carries the snapshot's
    /// pre-delete id, and every audit anchor (`created_at`, owner
    /// adoption window) refers to the new row, not the snapshot's.
    #[serde(default)]
    pub recreated_team: bool,
    /// The snapshot's pre-delete `team_id`; populated only when the restore
    /// recreated the team (see `recreated_team`). Absent on the common path
    /// where the team's id is preserved verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_team_id: Option<String>,
}

// =============================================================================
// Shared helpers (used across submodules)
// =============================================================================

pub(super) fn db_err(e: impl std::fmt::Display) -> crate::error::AlephError {
    crate::error::AlephError::ConfigError {
        message: format!("SnapshotStore: {e}"),
        suggestion: None,
    }
}
