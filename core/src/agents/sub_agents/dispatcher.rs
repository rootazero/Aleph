//! Sub-Agent Dispatcher
//!
//! Routes requests to the appropriate sub-agent based on the request
//! characteristics and available sub-agents.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::traits::{SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult};
use super::{McpSubAgent, SkillSubAgent};
use crate::dispatcher::ToolRegistry;
use crate::error::{AetherError, Result};

/// Type of sub-agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubAgentType {
    /// MCP tool execution agent
    Mcp,
    /// Skill execution agent
    Skill,
    /// Custom agent (user-defined)
    Custom,
}

impl std::fmt::Display for SubAgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mcp => write!(f, "mcp"),
            Self::Skill => write!(f, "skill"),
            Self::Custom => write!(f, "custom"),
        }
    }
}

impl std::str::FromStr for SubAgentType {
    type Err = AetherError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "mcp" | "mcp_agent" => Ok(Self::Mcp),
            "skill" | "skill_agent" => Ok(Self::Skill),
            "custom" => Ok(Self::Custom),
            _ => Err(AetherError::Other {
                message: format!("Unknown sub-agent type: {}", s),
                suggestion: None,
            }),
        }
    }
}

/// Sub-Agent Dispatcher
///
/// Routes requests to the appropriate sub-agent and manages
/// the sub-agent lifecycle.
pub struct SubAgentDispatcher {
    /// Registered sub-agents
    agents: HashMap<String, Arc<dyn SubAgent>>,
    /// Default agent for unmatched requests
    default_agent: Option<String>,
}

impl SubAgentDispatcher {
    /// Create a new dispatcher
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            default_agent: None,
        }
    }

    /// Create a dispatcher with default sub-agents
    pub fn with_defaults(tool_registry: Arc<RwLock<ToolRegistry>>) -> Self {
        let mut dispatcher = Self::new();

        // Register MCP agent
        let mcp_agent = McpSubAgent::new(tool_registry.clone());
        dispatcher.register(Arc::new(mcp_agent));

        // Register Skill agent
        let skill_agent = SkillSubAgent::new(tool_registry);
        dispatcher.register(Arc::new(skill_agent));

        dispatcher
    }

    /// Register a sub-agent
    pub fn register(&mut self, agent: Arc<dyn SubAgent>) {
        let id = agent.id().to_string();
        info!("Registering sub-agent: {} ({})", agent.name(), id);
        self.agents.insert(id, agent);
    }

    /// Unregister a sub-agent
    pub fn unregister(&mut self, agent_id: &str) -> Option<Arc<dyn SubAgent>> {
        self.agents.remove(agent_id)
    }

    /// Set the default agent for unmatched requests
    pub fn set_default(&mut self, agent_id: impl Into<String>) {
        self.default_agent = Some(agent_id.into());
    }

    /// Get a sub-agent by ID
    pub fn get(&self, agent_id: &str) -> Option<&Arc<dyn SubAgent>> {
        self.agents.get(agent_id)
    }

    /// Get all registered sub-agents
    pub fn list_agents(&self) -> Vec<&Arc<dyn SubAgent>> {
        self.agents.values().collect()
    }

    /// Find agents with a specific capability
    pub fn find_by_capability(&self, capability: SubAgentCapability) -> Vec<&Arc<dyn SubAgent>> {
        self.agents
            .values()
            .filter(|agent| agent.capabilities().contains(&capability))
            .collect()
    }

    /// Dispatch a request to the appropriate sub-agent
    ///
    /// The dispatcher selects a sub-agent based on:
    /// 1. Explicit agent_id in context
    /// 2. Target matching (e.g., MCP server name)
    /// 3. Capability matching
    /// 4. can_handle() check on each agent
    /// 5. Default agent (if set)
    pub async fn dispatch(&self, request: SubAgentRequest) -> Result<SubAgentResult> {
        info!("Dispatching request: {}", request.id);

        // 1. Check for explicit agent_id in context
        if let Some(agent_id) = request.context.get("agent_id").and_then(|v| v.as_str()) {
            if let Some(agent) = self.agents.get(agent_id) {
                debug!("Dispatching to explicit agent: {}", agent_id);
                return agent.execute(request).await;
            }
            warn!("Explicit agent_id not found: {}", agent_id);
        }

        // 2. Try to match by agent type in context
        if let Some(agent_type) = request.context.get("agent_type").and_then(|v| v.as_str()) {
            if let Ok(agent_type) = agent_type.parse::<SubAgentType>() {
                let agent_id = match agent_type {
                    SubAgentType::Mcp => "mcp_agent",
                    SubAgentType::Skill => "skill_agent",
                    SubAgentType::Custom => "custom_agent",
                };
                if let Some(agent) = self.agents.get(agent_id) {
                    debug!("Dispatching to agent type: {:?}", agent_type);
                    return agent.execute(request).await;
                }
            }
        }

        // 3. Find agents that can handle this request
        let mut capable_agents: Vec<_> = self
            .agents
            .values()
            .filter(|agent| agent.can_handle(&request))
            .collect();

        if !capable_agents.is_empty() {
            // Prefer more specific agents (fewer capabilities = more specialized)
            capable_agents.sort_by_key(|agent| agent.capabilities().len());
            let agent = capable_agents[0];
            debug!(
                "Dispatching to capable agent: {} ({})",
                agent.name(),
                agent.id()
            );
            return agent.execute(request).await;
        }

        // 4. Try default agent
        if let Some(ref default_id) = self.default_agent {
            if let Some(agent) = self.agents.get(default_id) {
                debug!("Dispatching to default agent: {}", default_id);
                return agent.execute(request).await;
            }
        }

        // No suitable agent found
        Err(AetherError::NotFound(format!(
            "No sub-agent found to handle request: {}",
            request.prompt.chars().take(50).collect::<String>()
        )))
    }

    /// Dispatch by agent type
    pub async fn dispatch_to(
        &self,
        agent_type: SubAgentType,
        request: SubAgentRequest,
    ) -> Result<SubAgentResult> {
        let agent_id = match agent_type {
            SubAgentType::Mcp => "mcp_agent",
            SubAgentType::Skill => "skill_agent",
            SubAgentType::Custom => "custom_agent",
        };

        self.agents
            .get(agent_id)
            .ok_or_else(|| AetherError::NotFound(format!("Sub-agent not found: {}", agent_id)))?
            .execute(request)
            .await
    }

    /// Dispatch to multiple agents and aggregate results
    pub async fn dispatch_parallel(
        &self,
        requests: Vec<(SubAgentRequest, Option<String>)>,
    ) -> Vec<Result<SubAgentResult>> {
        let futures: Vec<_> = requests
            .into_iter()
            .map(|(request, agent_id)| async move {
                if let Some(ref id) = agent_id {
                    if let Some(agent) = self.agents.get(id) {
                        return agent.execute(request).await;
                    }
                }
                self.dispatch(request).await
            })
            .collect();

        futures::future::join_all(futures).await
    }

    /// Get dispatcher info for prompt/context
    pub fn get_info(&self) -> DispatcherInfo {
        DispatcherInfo {
            agent_count: self.agents.len(),
            agents: self
                .agents
                .values()
                .map(|agent| AgentInfo {
                    id: agent.id().to_string(),
                    name: agent.name().to_string(),
                    description: agent.description().to_string(),
                    capabilities: agent.capabilities(),
                })
                .collect(),
            default_agent: self.default_agent.clone(),
        }
    }
}

impl Default for SubAgentDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about the dispatcher
#[derive(Debug, Clone, Serialize)]
pub struct DispatcherInfo {
    pub agent_count: usize,
    pub agents: Vec<AgentInfo>,
    pub default_agent: Option<String>,
}

/// Information about a sub-agent
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<SubAgentCapability>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolRegistry;

    #[tokio::test]
    async fn test_dispatcher_creation() {
        let dispatcher = SubAgentDispatcher::new();
        assert_eq!(dispatcher.list_agents().len(), 0);
    }

    #[tokio::test]
    async fn test_dispatcher_with_defaults() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = SubAgentDispatcher::with_defaults(registry);

        assert_eq!(dispatcher.list_agents().len(), 2);
        assert!(dispatcher.get("mcp_agent").is_some());
        assert!(dispatcher.get("skill_agent").is_some());
    }

    #[tokio::test]
    async fn test_dispatcher_find_by_capability() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = SubAgentDispatcher::with_defaults(registry);

        let mcp_agents = dispatcher.find_by_capability(SubAgentCapability::McpToolExecution);
        assert_eq!(mcp_agents.len(), 1);
        assert_eq!(mcp_agents[0].id(), "mcp_agent");

        let skill_agents = dispatcher.find_by_capability(SubAgentCapability::SkillExecution);
        assert_eq!(skill_agents.len(), 1);
        assert_eq!(skill_agents[0].id(), "skill_agent");
    }

    #[tokio::test]
    async fn test_dispatcher_dispatch_by_can_handle() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = SubAgentDispatcher::with_defaults(registry);

        // MCP-related request
        let request = SubAgentRequest::new("Use github MCP to list PRs");
        let result = dispatcher.dispatch(request).await;
        // Will fail because no MCP tools are registered, but routing should work
        assert!(result.is_ok());

        // Skill-related request
        let request = SubAgentRequest::new("Execute the workflow skill");
        let result = dispatcher.dispatch(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dispatcher_no_matching_agent() {
        let dispatcher = SubAgentDispatcher::new();

        let request = SubAgentRequest::new("Do something");
        let result = dispatcher.dispatch(request).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_sub_agent_type_parsing() {
        assert_eq!("mcp".parse::<SubAgentType>().unwrap(), SubAgentType::Mcp);
        assert_eq!("skill".parse::<SubAgentType>().unwrap(), SubAgentType::Skill);
        assert!("unknown".parse::<SubAgentType>().is_err());
    }

    #[tokio::test]
    async fn test_dispatcher_info() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = SubAgentDispatcher::with_defaults(registry);

        let info = dispatcher.get_info();
        assert_eq!(info.agent_count, 2);
        assert_eq!(info.agents.len(), 2);
    }
}
