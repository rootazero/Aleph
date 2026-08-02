//! `TeamMemberRemoveTool` — remove a member from a team.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for removing a member from a team.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamMemberRemoveArgs {
    /// ID of the team to remove the member from
    pub team_id: String,
    /// ID of the agent to remove
    pub agent_id: String,
}

/// Output from `team_member_remove`.
#[derive(Debug, Clone, Serialize)]
pub struct TeamMemberRemoveOutput {
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that removes a member from a team.
///
/// Only the team leader may remove members. The leader cannot be removed.
#[derive(Clone)]
pub struct TeamMemberRemoveTool {
    store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl TeamMemberRemoveTool {
    pub fn new(store: Arc<dyn TeamStore>, current_agent_id: String) -> Self {
        Self {
            store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for TeamMemberRemoveTool {
    const NAME: &'static str = "team_member_remove";
    const DESCRIPTION: &'static str =
        "Remove a member from a team. Only the team leader can remove members. \
        The leader themselves cannot be removed.";

    type Args = TeamMemberRemoveArgs;
    type Output = TeamMemberRemoveOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Verify the team exists and the caller is the leader
        let team = self
            .store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| AlephError::tool(format!("team not found: {}", args.team_id)))?;

        if team.leader_id != self.current_agent_id {
            return Err(AlephError::tool(format!(
                "only the team leader ('{}') can remove members",
                team.leader_id
            )));
        }

        // Delegate to the store (which checks disbanded, leader, and membership)
        self.store
            .remove_member(&args.team_id, &args.agent_id)
            .await?;

        info!(
            team_id = %args.team_id,
            agent_id = %args.agent_id,
            "team_member_remove: member removed"
        );

        Ok(TeamMemberRemoveOutput {
            message: format!(
                "Agent '{}' has been removed from team '{}'.",
                args.agent_id, args.team_id
            ),
        })
    }
}
