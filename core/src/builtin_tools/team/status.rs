//! TeamStatusTool — query team state and task history.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::{TeamStore, TeamStatus as TeamStatusEnum, TeamTaskStatus};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for querying team status.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamStatusArgs {
    /// ID of the team to query
    pub team_id: String,
}

/// Summary of a team member.
#[derive(Debug, Clone, Serialize)]
pub struct MemberInfo {
    pub agent_id: String,
    pub role: String,
}

/// Summary of a team task.
#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub id: String,
    pub agent_id: String,
    pub subject: String,
    pub status: TeamTaskStatus,
    pub result: Option<String>,
}

/// Output from team_status.
#[derive(Debug, Clone, Serialize)]
pub struct TeamStatusOutput {
    pub team_id: String,
    pub name: String,
    pub status: TeamStatusEnum,
    pub leader_id: String,
    pub members: Vec<MemberInfo>,
    pub tasks: Vec<TaskInfo>,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that queries the current state of a team, including members and tasks.
#[derive(Clone)]
pub struct TeamStatusTool {
    store: Arc<dyn TeamStore>,
}

impl TeamStatusTool {
    pub fn new(store: Arc<dyn TeamStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AlephTool for TeamStatusTool {
    const NAME: &'static str = "team_status";
    const DESCRIPTION: &'static str =
        "Query the current state of a team, including its members and task history. \
        Returns the team info, all enrolled members with their roles, and all tasks \
        with their current status and results.";

    type Args = TeamStatusArgs;
    type Output = TeamStatusOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec!["team_status(team_id='abc123')".to_string()])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let team = self
            .store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| {
                AlephError::other(format!("Team '{}' not found", args.team_id))
            })?;

        let members_raw = self.store.get_members(&args.team_id).await?;
        let tasks_raw = self.store.get_tasks(&args.team_id).await?;

        debug!(
            team_id = %args.team_id,
            members = members_raw.len(),
            tasks = tasks_raw.len(),
            "team_status: queried"
        );

        let members = members_raw
            .into_iter()
            .map(|m| MemberInfo {
                agent_id: m.agent_id,
                role: m.role,
            })
            .collect();

        let tasks = tasks_raw
            .into_iter()
            .map(|t| TaskInfo {
                id: t.id,
                agent_id: t.agent_id,
                subject: t.subject,
                status: t.status,
                result: t.result,
            })
            .collect();

        Ok(TeamStatusOutput {
            team_id: team.id,
            name: team.name,
            status: team.status,
            leader_id: team.leader_id,
            members,
            tasks,
        })
    }
}
