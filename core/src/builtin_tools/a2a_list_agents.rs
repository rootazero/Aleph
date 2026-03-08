//! A2A List Agents Tool
//!
//! Lists all registered A2A agents with optional name filtering.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::a2a::port::AgentResolver;
use crate::a2a::service::CardRegistry;
use crate::error::AlephError;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct A2AListAgentsArgs {
    /// Optional filter by agent name (case-insensitive substring match)
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub base_url: String,
    pub trust_level: String,
    pub health: String,
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct A2AListAgentsResult {
    pub agents: Vec<AgentSummary>,
    pub total: usize,
}

#[derive(Clone)]
pub struct A2AListAgentsTool {
    registry: Arc<CardRegistry>,
}

impl A2AListAgentsTool {
    pub fn new(registry: Arc<CardRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl AlephTool for A2AListAgentsTool {
    const NAME: &'static str = "a2a_list_agents";
    const DESCRIPTION: &'static str =
        "List all registered A2A agents, optionally filtering by name.";

    type Args = A2AListAgentsArgs;
    type Output = A2AListAgentsResult;

    async fn call(&self, args: Self::Args) -> crate::error::Result<Self::Output> {
        let all_agents = self
            .registry
            .list_agents()
            .await
            .map_err(|e| AlephError::tool(format!("Failed to list agents: {}", e)))?;

        let filter_lower = args.filter.as_deref().map(|f| f.to_lowercase());

        let agents: Vec<AgentSummary> = all_agents
            .into_iter()
            .filter(|a| {
                if let Some(ref f) = filter_lower {
                    a.card.name.to_lowercase().contains(f)
                } else {
                    true
                }
            })
            .map(|a| AgentSummary {
                id: a.card.id,
                name: a.card.name,
                description: a.card.description,
                base_url: a.base_url,
                trust_level: format!("{:?}", a.trust_level),
                health: format!("{:?}", a.health),
                skills: a.card.skills.iter().map(|s| s.name.clone()).collect(),
            })
            .collect();

        let total = agents.len();
        Ok(A2AListAgentsResult { agents, total })
    }
}
