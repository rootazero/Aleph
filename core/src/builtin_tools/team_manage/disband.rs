//! TeamDisbandTool — disband a team, cancelling all pending tasks.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::sub_agents::{SubAgentRegistry, RunStatus};
use crate::agents::swarm::tasks::{
    CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, TeamStatus, TeamUpdate,
};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
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

/// Output from team disbanding.
#[derive(Debug, Clone, Serialize)]
pub struct TeamDisbandOutput {
    pub team_id: String,
    pub cancelled_tasks: Vec<String>,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that disbands a team, cancels all non-completed tasks, and kills
/// keep-alive sub-agent runs.
#[derive(Clone)]
pub struct TeamDisbandTool {
    store: Arc<dyn CoordTaskStore>,
    sub_registry: Arc<SubAgentRegistry>,
}

impl TeamDisbandTool {
    pub fn new(store: Arc<dyn CoordTaskStore>, sub_registry: Arc<SubAgentRegistry>) -> Self {
        Self { store, sub_registry }
    }
}

#[async_trait]
impl AlephTool for TeamDisbandTool {
    const NAME: &'static str = "team_disband";
    const DESCRIPTION: &'static str =
        "Disband a team by cancelling all non-completed tasks and marking the \
         team as disbanded. Only active teams can be disbanded.";

    type Args = TeamDisbandArgs;
    type Output = TeamDisbandOutput;

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // 1. Verify team exists and is active
        let team = self
            .store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| AlephError::NotFound(format!("Team '{}' not found", args.team_id)))?;

        if team.status != TeamStatus::Active {
            return Err(AlephError::Other {
                message: format!(
                    "Team '{}' is already {} and cannot be disbanded",
                    args.team_id,
                    team.status.as_str()
                ),
                suggestion: None,
            });
        }

        // 2. Cancel all non-completed tasks
        let tasks = self
            .store
            .list_tasks(CoordTaskFilter {
                team_id: Some(args.team_id.clone()),
                ..Default::default()
            })
            .await?;

        let mut cancelled_tasks = Vec::new();
        for task in &tasks {
            if !matches!(
                task.status,
                CoordTaskStatus::Completed | CoordTaskStatus::Cancelled | CoordTaskStatus::Failed
            ) {
                let update = CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Cancelled),
                    ..Default::default()
                };
                // Best-effort cancellation — log but don't fail the whole operation
                match self.store.update_task(&task.id, update).await {
                    Ok(_) => cancelled_tasks.push(task.id.clone()),
                    Err(e) => {
                        tracing::warn!(
                            task_id = %task.id,
                            error = %e,
                            "Failed to cancel task during team disband"
                        );
                    }
                }
            }
        }

        // 3. Mark team as disbanded
        let update = TeamUpdate {
            status: Some(TeamStatus::Disbanded),
        };
        self.store.update_team(&args.team_id, update).await?;

        // 4. Kill keep_alive sub-agents (best-effort)
        if let Ok(Some(team_data)) = self.store.get_team(&args.team_id).await {
            for member in &team_data.members {
                if let Some(run_id) = &member.run_id {
                    if let Err(e) = self.sub_registry.transition(run_id, RunStatus::Cancelled).await {
                        tracing::warn!(
                            run_id = %run_id,
                            role = %member.role,
                            error = %e,
                            "Failed to cancel sub-agent during team disband (will be cleaned up at session end)"
                        );
                    }
                }
            }
        }

        info!(
            team_id = %args.team_id,
            cancelled = cancelled_tasks.len(),
            "Team disbanded"
        );

        let msg = format!(
            "Team '{}' disbanded. {} tasks cancelled.",
            args.team_id,
            cancelled_tasks.len()
        );

        Ok(TeamDisbandOutput {
            team_id: args.team_id,
            cancelled_tasks,
            message: msg,
        })
    }
}
