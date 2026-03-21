//! TeamCreateTool — create a new coordination team.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::swarm::tasks::{CoordTaskStore, NewTeam};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::launch::generate_team_id;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for creating a coordination team.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamCreateArgs {
    /// Human-readable team name
    pub name: String,
    /// Description of the team's purpose
    #[serde(default)]
    pub description: Option<String>,
    /// Agent ID of the team leader
    pub leader: String,
}

/// Output from team creation.
#[derive(Debug, Clone, Serialize)]
pub struct TeamCreateOutput {
    pub team_id: String,
    pub name: String,
    pub leader: String,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that creates a new coordination team.
#[derive(Clone)]
pub struct TeamCreateTool {
    store: Arc<dyn CoordTaskStore>,
}

impl TeamCreateTool {
    pub fn new(store: Arc<dyn CoordTaskStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AlephTool for TeamCreateTool {
    const NAME: &'static str = "team_create";
    const DESCRIPTION: &'static str =
        "Create a new coordination team with a leader agent. Teams group agents \
         together for collaborative task execution. Add members and tasks after creation.";

    type Args = TeamCreateArgs;
    type Output = TeamCreateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let team_id = generate_team_id(&args.name);

        let new_team = NewTeam {
            id: team_id.clone(),
            name: args.name.clone(),
            description: args.description.unwrap_or_default(),
            leader: args.leader.clone(),
        };

        let team = self.store.create_team(new_team).await?;

        info!(team_id = %team.id, name = %team.name, "Team created");

        Ok(TeamCreateOutput {
            message: format!("Team '{}' created (id: {})", team.name, team.id),
            team_id: team.id,
            name: team.name,
            leader: team.leader,
        })
    }
}
