//! Swarm Tools
//!
//! AlephTool implementations for swarm intelligence features.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::collective_memory::{CollectiveMemory, TeamHistoryQuery};
use crate::error::Result;
use crate::tools::AlephTool;

/// Get Team Activity Tool
///
/// Allows agents to query what other agents in the team have been working on.
#[derive(Clone)]
pub struct GetTeamActivityTool {
    memory: Arc<CollectiveMemory>,
}

impl GetTeamActivityTool {
    /// Create a new GetTeamActivityTool
    pub fn new(memory: Arc<CollectiveMemory>) -> Self {
        Self { memory }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetTeamActivityArgs {
    /// Query string for filtering events.
    ///
    /// Supported formats:
    /// - "agent:agent_id" - Get events from specific agent
    /// - "path:/src/auth" - Get events related to path
    /// - "recent:50" - Get last N events
    /// - Default: Get last 20 events
    #[serde(default = "default_query")]
    pub query: String,
}

fn default_query() -> String {
    "recent:20".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct GetTeamActivityOutput {
    pub events: Vec<String>,
    pub count: usize,
}

#[async_trait]
impl AlephTool for GetTeamActivityTool {
    const NAME: &'static str = "get_team_activity";

    const DESCRIPTION: &'static str = "Query what other agents in the team have been working on. \
         Use this when you need context about parallel work or want to \
         avoid duplicate efforts.\n\n\
         Query formats:\n\
         - agent:agent_id - Get events from specific agent\n\
         - path:/src/auth - Get events related to path\n\
         - recent:50 - Get last N events\n\
         - Default: Get last 20 events";

    type Args = GetTeamActivityArgs;
    type Output = GetTeamActivityOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Parse query
        let query = TeamHistoryQuery::from_string(&args.query)?;

        // Search team history
        let events = self.memory.search_team_history(query).await?;
        let count = events.len();

        Ok(GetTeamActivityOutput { events, count })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::bus::AgentMessageBus;

    #[tokio::test]
    async fn test_tool_creation() {
        let bus = Arc::new(AgentMessageBus::new());
        let memory = Arc::new(CollectiveMemory::new(bus));
        let _tool = GetTeamActivityTool::new(memory);

        assert_eq!(GetTeamActivityTool::NAME, "get_team_activity");
        assert!(!GetTeamActivityTool::DESCRIPTION.is_empty());
    }

    #[tokio::test]
    async fn test_tool_execution_empty() {
        let bus = Arc::new(AgentMessageBus::new());
        let memory = Arc::new(CollectiveMemory::new(bus));
        let tool = GetTeamActivityTool::new(memory);

        let args = GetTeamActivityArgs {
            query: "recent:10".to_string(),
        };

        let result = tool.call(args).await.unwrap();
        assert_eq!(result.count, 0);
        assert!(result.events.is_empty());
    }
}
