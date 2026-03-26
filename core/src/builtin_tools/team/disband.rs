//! TeamDisbandTool — mark a team as disbanded.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for disbanding a team.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamDisbandArgs {
    /// ID of the team to disband
    pub team_id: String,
}

/// Output from team_disband.
#[derive(Debug, Clone, Serialize)]
pub struct TeamDisbandOutput {
    pub team_id: String,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that marks a team as disbanded.
///
/// A disbanded team is no longer active. Its records are preserved for
/// history. Use `team_delete` (if available) to permanently remove the record.
#[derive(Clone)]
pub struct TeamDisbandTool {
    store: Arc<dyn TeamStore>,
}

impl TeamDisbandTool {
    pub fn new(store: Arc<dyn TeamStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AlephTool for TeamDisbandTool {
    const NAME: &'static str = "team_disband";
    const DESCRIPTION: &'static str =
        "Mark a team as disbanded. The team's history is preserved but it becomes \
        inactive. Members and tasks are retained for reference. \
        This action cannot be undone.";

    type Args = TeamDisbandArgs;
    type Output = TeamDisbandOutput;

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "team_disband(team_id='abc123')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.store.disband_team(&args.team_id).await?;

        info!(team_id = %args.team_id, "team_disband: team disbanded");

        Ok(TeamDisbandOutput {
            team_id: args.team_id.clone(),
            message: format!("Team '{}' has been disbanded.", args.team_id),
        })
    }
}
