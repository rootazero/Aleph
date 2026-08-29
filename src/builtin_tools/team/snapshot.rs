//! `TeamSnapshotTool` — unified team-snapshot CRUD via an `action` field.
//!
//! Combines what `ClawTeam` exposes as four CLI subcommands (`create`, `list`,
//! `get`, `restore`) into one builtin tool with an action discriminant. The
//! single-tool shape keeps the registration footprint small (5-file cycle
//! once) and gives the LLM one schema to reason about instead of four.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::agents::swarm::tasks::CoordTaskStore;
use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::snapshots::{
    capture_snapshot, restore_snapshot, CreateSnapshotOutput, RestoreDiff, SnapshotMeta,
    SqliteSnapshotStore, TeamSnapshotPayload,
};
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotAction {
    /// Capture a new snapshot of `team_id`'s current state.
    Create,
    /// List snapshots (optionally filtered by `team_id`), newest first.
    List,
    /// Load a snapshot's full payload (requires `snapshot_id`).
    Get,
    /// Restore a snapshot. Dry-run by default — pass `apply: true` to mutate.
    Restore,
    /// Delete a snapshot (requires `snapshot_id`). Idempotent.
    Delete,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamSnapshotArgs {
    /// Which operation to perform.
    pub action: SnapshotAction,
    /// Team id (required for `create`; optional filter for `list`).
    #[serde(default)]
    pub team_id: Option<String>,
    /// Snapshot id (required for `get`, `restore`, `delete`).
    #[serde(default)]
    pub snapshot_id: Option<String>,
    /// Free-text tag attached at create time (e.g. "pre-refactor").
    #[serde(default)]
    pub tag: Option<String>,
    /// Optional note stored inside the payload (e.g. "after sprint 3").
    #[serde(default)]
    pub note: Option<String>,
    /// For `restore`: when true, perform the mutation. Default false (dry-run).
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TeamSnapshotOutput {
    Create(CreateSnapshotOutput),
    List {
        snapshots: Vec<SnapshotMeta>,
    },
    Get {
        meta: SnapshotMeta,
        payload: TeamSnapshotPayload,
    },
    Restore(RestoreDiff),
    Delete {
        snapshot_id: String,
        existed: bool,
    },
}

// =============================================================================
// Tool
// =============================================================================

#[derive(Clone)]
pub struct TeamSnapshotTool {
    team_store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    snapshot_store: Arc<SqliteSnapshotStore>,
    /// Injected by `ExecutionEngine` before each call (for audit logging only).
    pub current_agent_id: String,
}

impl TeamSnapshotTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    pub fn new(
        team_store: Arc<dyn TeamStore>,
        coord_store: Arc<dyn CoordTaskStore>,
        snapshot_store: Arc<SqliteSnapshotStore>,
        current_agent_id: impl Into<String>,
    ) -> Self {
        Self {
            team_store,
            coord_store,
            snapshot_store,
            current_agent_id: current_agent_id.into(),
        }
    }
}

#[async_trait]
impl AlephTool for TeamSnapshotTool {
    const NAME: &'static str = "team_snapshot";
    const DESCRIPTION: &'static str = "Manage team snapshots — point-in-time bundles of config + \
        members + tasks for backup, branching, or pre-refactor checkpointing. Actions: \
        `create` (capture current state, requires team_id), `list` (newest first, optional \
        team_id filter), `get` (load full payload by snapshot_id), `restore` (defaults to \
        dry-run that returns a diff; pass apply=true to mutate live state), `delete` \
        (idempotent). Restore is conservative: members are added/removed, missing tasks are \
        added Pending, but InProgress tasks and Completed/Failed snapshot states are NEVER \
        clobbered.";

    type Args = TeamSnapshotArgs;
    // Returned as a serde_json::Value so the enum can deserialise on the wire
    // without an external "kind" discriminator at the response level (the
    // action arg already conveys intent).
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(
            action = ?args.action,
            team_id = ?args.team_id,
            snapshot_id = ?args.snapshot_id,
            caller = %self.actor(),
            "team_snapshot: dispatch"
        );

        let result = match args.action {
            SnapshotAction::Create => {
                let team_id = args.team_id.as_deref().ok_or_else(|| {
                    AlephError::other("team_snapshot(create): team_id is required")
                })?;
                // BT-D-R4-23: gate before any snapshot_store write — a
                // caller that isn't a member could otherwise write a
                // snapshot (which then lives in the store, and shows up in
                // every later List for the team).
                crate::builtin_tools::team::require_team_auth(
                    &*self.team_store,
                    team_id,
                    &self.actor(),
                )
                .await?;
                let tag = args.tag.as_deref().unwrap_or("");
                let note = args.note.as_deref().unwrap_or("");
                let out = capture_snapshot(
                    self.snapshot_store.as_ref(),
                    self.team_store.as_ref(),
                    self.coord_store.as_ref(),
                    team_id,
                    tag,
                    note,
                )
                .await?;
                TeamSnapshotOutput::Create(out)
            }
            SnapshotAction::List => {
                let snapshots = self.snapshot_store.list(args.team_id.as_deref()).await?;
                TeamSnapshotOutput::List { snapshots }
            }
            SnapshotAction::Get => {
                let snapshot_id = args.snapshot_id.as_deref().ok_or_else(|| {
                    AlephError::other("team_snapshot(get): snapshot_id is required")
                })?;
                let (meta, payload) =
                    self.snapshot_store.get(snapshot_id).await?.ok_or_else(|| {
                        AlephError::NotFound(format!("snapshot `{snapshot_id}` not found"))
                    })?;
                TeamSnapshotOutput::Get { meta, payload }
            }
            SnapshotAction::Restore => {
                let snapshot_id = args.snapshot_id.as_deref().ok_or_else(|| {
                    AlephError::other("team_snapshot(restore): snapshot_id is required")
                })?;
                let diff = restore_snapshot(
                    self.snapshot_store.as_ref(),
                    self.team_store.as_ref(),
                    self.coord_store.as_ref(),
                    snapshot_id,
                    !args.apply, // dry_run = !apply
                )
                .await?;
                TeamSnapshotOutput::Restore(diff)
            }
            SnapshotAction::Delete => {
                let snapshot_id = args.snapshot_id.as_deref().ok_or_else(|| {
                    AlephError::other("team_snapshot(delete): snapshot_id is required")
                })?;
                let existed = self.snapshot_store.delete(snapshot_id).await?;
                TeamSnapshotOutput::Delete {
                    snapshot_id: snapshot_id.to_string(),
                    existed,
                }
            }
        };

        Ok(serde_json::to_value(result).map_err(|e| {
            AlephError::other(format!("team_snapshot: response serialize failed: {e}"))
        })?)
    }
}
