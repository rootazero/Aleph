//! Team-ownership gating for the `teams.*` RPC surface (P1 data isolation).
//!
//! The predicate itself lives in [`crate::teams::scoped`] — the `TeamStore`
//! decorator every consumer goes through. This module exists for the two things
//! a store decorator structurally cannot do:
//!
//! 1. **Shape the denial.** A decorator returns `None` / a store error; the
//!    wire needs the byte-identical `RESOURCE_NOT_FOUND` response a genuinely
//!    missing team produces (spec §5.4 GC4, no existence oracle).
//! 2. **Reach teams addressed through the task DAG.** Half the `teams.*`
//!    methods take a `task_id`, not a `team_id`, and resolve it in
//!    `coord_tasks` — a different database the team store cannot see. Those
//!    are the methods a sweep keyed on parameter shape misses, which is
//!    exactly how P1's own final review found `agent.run`.
//!
//! Every handler in this module's siblings calls [`gate_team`] or [`gate_task`]
//! before touching any store. Do not write `team.owner_user_id == caller` at a
//! call site — that is the second derivation this whole layer exists to avoid.

use serde_json::Value;

use crate::agents::swarm::tasks::{CoordTask, CoordTaskStore};
use crate::gateway::protocol::{JsonRpcResponse, RESOURCE_NOT_FOUND};
use crate::sync_primitives::Arc;
use crate::teams::{Team, TeamStore};

/// The response an unreachable team produces — whether it does not exist, is
/// owned by someone else, or could not be resolved at all.
///
/// One constructor so the three causes stay indistinguishable. The wording is
/// the one `teams.get` already returned for a missing team, so a single-user
/// deployment sees byte-identical output to before P1.
#[must_use]
pub fn team_not_found(id: Option<Value>, team_id: &str) -> JsonRpcResponse {
    JsonRpcResponse::error(
        id,
        RESOURCE_NOT_FOUND,
        format!("Team '{team_id}' not found"),
    )
}

/// Resolve a team the caller addressed by id, or produce the denial response.
///
/// Reads through the scoped store, so "invisible" already arrives as `None`.
/// Fails closed on a store error: a caller must never be let through because
/// the check itself could not complete (spec §5.4 GC3).
pub async fn gate_team(
    id: Option<Value>,
    store: &Arc<dyn TeamStore>,
    team_id: &str,
) -> Result<Team, JsonRpcResponse> {
    match store.get_team(team_id).await {
        Ok(Some(team)) => Ok(team),
        Ok(None) => Err(team_not_found(id, team_id)),
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "teams: ownership gate failed closed");
            Err(team_not_found(id, team_id))
        }
    }
}

/// The response an unreachable TASK produces — absent, orphaned, or belonging
/// to another user's team.
///
/// Keyed on the task id the caller supplied. Echoing back the team id would
/// hand them the identifier of a team they are not allowed to see, which is the
/// oracle this layer exists to close.
#[must_use]
pub fn task_not_found(id: Option<Value>, task_id: &str) -> JsonRpcResponse {
    JsonRpcResponse::error(
        id,
        RESOURCE_NOT_FOUND,
        format!("Task '{task_id}' not found"),
    )
}

/// Whether the team a snapshot belongs to is reachable by the caller.
///
/// A snapshot whose team is gone is refused, same reasoning as [`gate_task`]:
/// there is no owner left to check, and an orphan must not read as public.
/// `teams.delete` cascades `delete_team_snapshots`, so an orphan only exists on
/// the store-only fallback delete path.
pub async fn snapshot_team_visible(store: &Arc<dyn TeamStore>, team_id: &str) -> bool {
    matches!(store.get_team(team_id).await, Ok(Some(_)))
}

/// Resolve a task the caller addressed by id, gating on ITS team's owner.
///
/// The returned task is the same row the handler would have fetched, so this
/// replaces the handler's own lookup rather than adding one.
///
/// A task whose team no longer exists is refused, not waved through: an
/// orphaned row has no owner to check against, and "unowned" must never read
/// as "public". Same fail-closed contract as [`gate_team`], and the denial is
/// keyed on the TASK id the caller supplied — echoing back a team id they
/// could not see would be the oracle this is preventing.
pub async fn gate_task(
    id: Option<Value>,
    store: &Arc<dyn TeamStore>,
    coord_store: &Arc<dyn CoordTaskStore>,
    task_id: &str,
) -> Result<CoordTask, JsonRpcResponse> {
    let task = match coord_store.get_task(task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Err(task_not_found(id, task_id)),
        Err(e) => {
            tracing::warn!(task_id = %task_id, error = %e, "teams: task gate failed closed");
            return Err(task_not_found(id, task_id));
        }
    };
    let Some(team_id) = task.team_id.as_deref() else {
        // A task with no team is not team-owned, so there is no team stamp to
        // check. It reads as an unstamped record — the org-era operator's, by
        // the same adoption-by-absence rule everywhere else in P1. Waving it
        // through unconditionally would hand every member the entire
        // team-less coord DAG through the `teams.task.*` verbs.
        return if crate::teams::team_visible(None) {
            Ok(task)
        } else {
            Err(task_not_found(id, task_id))
        };
    };
    match store.get_team(team_id).await {
        Ok(Some(_)) => Ok(task),
        Ok(None) => Err(task_not_found(id, task_id)),
        Err(e) => {
            tracing::warn!(task_id = %task_id, error = %e, "teams: task gate failed closed");
            Err(task_not_found(id, task_id))
        }
    }
}
