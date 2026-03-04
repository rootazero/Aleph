//! Sub-Agent Dispatcher
//!
//! Routes requests to the appropriate sub-agent based on the request
//! characteristics and available sub-agents.

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::coordinator::{CoordinatorConfig, ExecutionCoordinator, ExecutionError, ToolCallSummary};
use super::result_collector::ResultCollector;
use super::traits::{SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult, ToolCallRecord};
use super::{McpSubAgent, SkillSubAgent};
use crate::dispatcher::ToolRegistry;
use crate::error::{AlephError, Result};

/// Convert ToolCallSummary to ToolCallRecord
fn summary_to_record(summary: &ToolCallSummary) -> ToolCallRecord {
    let success = summary.state.status == "completed";
    ToolCallRecord {
        name: summary.tool.clone(),
        arguments: serde_json::Value::Null, // Arguments not preserved in summary
        success,
        result_summary: summary.state.title.clone().unwrap_or_default(),
    }
}

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
    type Err = AlephError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "mcp" | "mcp_agent" => Ok(Self::Mcp),
            "skill" | "skill_agent" => Ok(Self::Skill),
            "custom" => Ok(Self::Custom),
            _ => Err(AlephError::Other {
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
///
/// # Synchronous Execution
///
/// The dispatcher supports synchronous execution modes that block until
/// sub-agent completion:
///
/// ```rust,ignore
/// // Single synchronous dispatch
/// let result = dispatcher.dispatch_sync(request, Duration::from_secs(60)).await?;
///
/// // Parallel synchronous dispatch (waits for all)
/// let results = dispatcher.dispatch_parallel_sync(requests, Duration::from_secs(120)).await;
/// ```
pub struct SubAgentDispatcher {
    /// Registered sub-agents
    agents: HashMap<String, Arc<dyn SubAgent>>,
    /// Default agent for unmatched requests
    default_agent: Option<String>,
    /// Coordinator for synchronous execution
    coordinator: Arc<ExecutionCoordinator>,
    /// Collector for tool call aggregation
    collector: Arc<ResultCollector>,
    /// Optional authorization checker for sub-agent delegation
    authority: Option<Arc<dyn super::authority::SubagentAuthority>>,
}

impl SubAgentDispatcher {
    /// Create a new dispatcher
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            default_agent: None,
            coordinator: Arc::new(ExecutionCoordinator::new(CoordinatorConfig::default())),
            collector: Arc::new(ResultCollector::new()),
            authority: None,
        }
    }

    /// Create a new dispatcher with custom configuration
    pub fn with_config(coordinator_config: CoordinatorConfig) -> Self {
        Self {
            agents: HashMap::new(),
            default_agent: None,
            coordinator: Arc::new(ExecutionCoordinator::new(coordinator_config)),
            collector: Arc::new(ResultCollector::new()),
            authority: None,
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

    /// Set the authorization checker for sub-agent delegation
    pub fn with_authority(mut self, authority: Arc<dyn super::authority::SubagentAuthority>) -> Self {
        self.authority = Some(authority);
        self
    }

    /// Get the execution coordinator (for external event integration)
    pub fn coordinator(&self) -> &Arc<ExecutionCoordinator> {
        &self.coordinator
    }

    /// Get the result collector (for external event integration)
    pub fn collector(&self) -> &Arc<ResultCollector> {
        &self.collector
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
            // Break ties by name for deterministic selection
            capable_agents.sort_by(|a, b| {
                a.capabilities().len().cmp(&b.capabilities().len())
                    .then_with(|| a.name().cmp(b.name()))
            });
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
        Err(AlephError::NotFound(format!(
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
            .ok_or_else(|| AlephError::NotFound(format!("Sub-agent not found: {}", agent_id)))?
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

    /// Dispatch a request and wait for the result synchronously
    ///
    /// This method blocks until the sub-agent completes or the timeout expires.
    /// It also collects tool call information via the ResultCollector.
    ///
    /// # Arguments
    ///
    /// * `request` - The sub-agent request to execute
    /// * `timeout` - Maximum time to wait for completion
    ///
    /// # Returns
    ///
    /// * `Ok(SubAgentResult)` - The completed result with tool call summaries
    /// * `Err(ExecutionError)` - On timeout or execution failure
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = dispatcher.dispatch_sync(request, Duration::from_secs(60)).await?;
    /// println!("Result: {}", result.summary);
    /// println!("Tools called: {:?}", result.tools_called);
    /// ```
    pub async fn dispatch_sync(
        &self,
        request: SubAgentRequest,
        timeout: Duration,
    ) -> std::result::Result<SubAgentResult, ExecutionError> {
        let request_id = request.id.clone();
        info!("Dispatching sync request: {}", request_id);

        // Check authorization before dispatch
        if let Some(ref authority) = self.authority {
            let parent_id = request.context.get("parent_agent_id")
                .and_then(|v| v.as_str())
                .unwrap_or("main");
            let child_id = request.context.get("agent_id")
                .and_then(|v| v.as_str())
                .or_else(|| request.target.as_deref())
                .unwrap_or("unknown");
            if !authority.can_delegate(parent_id, child_id) {
                return Err(ExecutionError::Internal(format!(
                    "Agent '{}' is not authorized to delegate to '{}'",
                    parent_id, child_id
                )));
            }
        }

        // Initialize result collection
        self.collector.init_request(&request_id).await;

        // Start execution tracking
        let _handle = self.coordinator.start_execution(&request_id).await;

        // Spawn the actual execution
        let dispatcher_agents = self.agents.clone();
        let default_agent = self.default_agent.clone();
        let coordinator = self.coordinator.clone();
        let collector = self.collector.clone();
        let req_id_clone = request_id.clone();

        tokio::spawn(async move {
            let result = Self::execute_dispatch(
                &dispatcher_agents,
                &default_agent,
                request,
            )
            .await;

            // Get tool summaries and convert to records
            let summaries = collector.get_summary(&req_id_clone).await;
            let tool_records: Vec<ToolCallRecord> = summaries.iter().map(summary_to_record).collect();

            // Enrich result with tool call information
            let enriched_result = match result {
                Ok(mut res) => {
                    res.request_id = req_id_clone.clone();
                    res.tools_called = tool_records;
                    res
                }
                Err(e) => SubAgentResult::failure(&req_id_clone, e.to_string())
                    .with_tools_called(tool_records),
            };

            // Signal completion
            coordinator.on_execution_completed(enriched_result).await;
        });

        // Wait for result
        let result = self.coordinator.wait_for_result(&request_id, timeout).await;

        // Cleanup
        self.collector.cleanup(&request_id).await;

        result
    }

    /// Dispatch multiple requests in parallel and wait for all to complete
    ///
    /// This method blocks until all sub-agents complete or the timeout expires.
    /// Results are returned in the same order as the input requests.
    ///
    /// # Arguments
    ///
    /// * `requests` - List of sub-agent requests with optional target agent IDs
    /// * `timeout` - Maximum time to wait for all completions
    ///
    /// # Returns
    ///
    /// A vector of (request_id, result) pairs in the same order as input.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let requests = vec![
    ///     (SubAgentRequest::new("task1"), Some("mcp_agent".into())),
    ///     (SubAgentRequest::new("task2"), None),
    /// ];
    /// let results = dispatcher.dispatch_parallel_sync(requests, Duration::from_secs(120)).await;
    /// for (id, result) in results {
    ///     match result {
    ///         Ok(res) => println!("{}: {}", id, res.summary),
    ///         Err(e) => println!("{}: failed - {:?}", id, e),
    ///     }
    /// }
    /// ```
    pub async fn dispatch_parallel_sync(
        &self,
        requests: Vec<(SubAgentRequest, Option<String>)>,
        timeout: Duration,
    ) -> Vec<(String, std::result::Result<SubAgentResult, ExecutionError>)> {
        if requests.is_empty() {
            return vec![];
        }

        info!("Dispatching {} parallel sync requests", requests.len());

        let mut request_ids = Vec::with_capacity(requests.len());

        // Start all executions
        for (request, agent_id) in requests {
            let request_id = request.id.clone();
            request_ids.push(request_id.clone());

            // Check authorization before dispatch
            if let Some(ref authority) = self.authority {
                let parent_id = request.context.get("parent_agent_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                let child_id = agent_id.as_deref()
                    .or_else(|| request.context.get("agent_id").and_then(|v| v.as_str()))
                    .or_else(|| request.target.as_deref())
                    .unwrap_or("unknown");
                if !authority.can_delegate(parent_id, child_id) {
                    // For parallel dispatch, track the failure without spawning
                    let req_id = request_id.clone();
                    self.collector.init_request(&req_id).await;
                    let _handle = self.coordinator.start_execution(&req_id).await;
                    let err_result = SubAgentResult::failure(
                        &req_id,
                        format!("Agent '{}' is not authorized to delegate to '{}'", parent_id, child_id),
                    );
                    self.coordinator.on_execution_completed(err_result).await;
                    continue;
                }
            }

            // Initialize result collection
            self.collector.init_request(&request_id).await;

            // Start execution tracking
            let _handle = self.coordinator.start_execution(&request_id).await;

            // Spawn the execution
            let dispatcher_agents = self.agents.clone();
            let default_agent = self.default_agent.clone();
            let coordinator = self.coordinator.clone();
            let collector = self.collector.clone();
            let req_id_clone = request_id.clone();

            tokio::spawn(async move {
                let result = if let Some(ref id) = agent_id {
                    if let Some(agent) = dispatcher_agents.get(id) {
                        agent.execute(request).await
                    } else {
                        Self::execute_dispatch(&dispatcher_agents, &default_agent, request).await
                    }
                } else {
                    Self::execute_dispatch(&dispatcher_agents, &default_agent, request).await
                };

                // Get tool summaries and convert to records
                let summaries = collector.get_summary(&req_id_clone).await;
                let tool_records: Vec<ToolCallRecord> = summaries.iter().map(summary_to_record).collect();

                // Enrich result
                let enriched_result = match result {
                    Ok(mut res) => {
                        res.request_id = req_id_clone.clone();
                        res.tools_called = tool_records;
                        res
                    }
                    Err(e) => SubAgentResult::failure(&req_id_clone, e.to_string())
                        .with_tools_called(tool_records),
                };

                coordinator.on_execution_completed(enriched_result).await;
            });
        }

        // Wait for all results
        let results = self.coordinator.wait_for_all(&request_ids, timeout).await;

        // Cleanup
        for id in &request_ids {
            self.collector.cleanup(id).await;
        }

        results
    }

    /// Internal helper to execute dispatch logic (for use in spawned tasks)
    async fn execute_dispatch(
        agents: &HashMap<String, Arc<dyn SubAgent>>,
        default_agent: &Option<String>,
        request: SubAgentRequest,
    ) -> Result<SubAgentResult> {
        // 1. Check for explicit agent_id in context
        if let Some(agent_id) = request.context.get("agent_id").and_then(|v| v.as_str()) {
            if let Some(agent) = agents.get(agent_id) {
                return agent.execute(request).await;
            }
        }

        // 2. Try to match by agent type in context
        if let Some(agent_type) = request.context.get("agent_type").and_then(|v| v.as_str()) {
            if let Ok(agent_type) = agent_type.parse::<SubAgentType>() {
                let agent_id = match agent_type {
                    SubAgentType::Mcp => "mcp_agent",
                    SubAgentType::Skill => "skill_agent",
                    SubAgentType::Custom => "custom_agent",
                };
                if let Some(agent) = agents.get(agent_id) {
                    return agent.execute(request).await;
                }
            }
        }

        // 3. Find agents that can handle this request
        let mut capable_agents: Vec<_> = agents
            .values()
            .filter(|agent| agent.can_handle(&request))
            .collect();

        if !capable_agents.is_empty() {
            capable_agents.sort_by_key(|agent| agent.capabilities().len());
            return capable_agents[0].execute(request).await;
        }

        // 4. Try default agent
        if let Some(ref default_id) = default_agent {
            if let Some(agent) = agents.get(default_id) {
                return agent.execute(request).await;
            }
        }

        // No suitable agent found
        Err(AlephError::NotFound(format!(
            "No sub-agent found to handle request: {}",
            request.prompt.chars().take(50).collect::<String>()
        )))
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

    #[tokio::test]
    async fn test_dispatcher_with_config() {
        let config = CoordinatorConfig {
            execution_timeout_ms: 60_000,
            result_ttl_ms: 300_000,
            max_concurrent: 10,
            progress_events_enabled: false,
        };
        let dispatcher = SubAgentDispatcher::with_config(config);
        assert_eq!(dispatcher.list_agents().len(), 0);
    }

    #[tokio::test]
    async fn test_coordinator_and_collector_accessors() {
        let dispatcher = SubAgentDispatcher::new();

        // Can access coordinator
        let coordinator = dispatcher.coordinator();
        assert!(Arc::strong_count(coordinator) >= 1);

        // Can access collector
        let collector = dispatcher.collector();
        assert!(Arc::strong_count(collector) >= 1);
    }

    #[tokio::test]
    async fn test_dispatch_sync_success() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = SubAgentDispatcher::with_defaults(registry);

        let request = SubAgentRequest::new("Execute skill task");
        let result = dispatcher.dispatch_sync(request, Duration::from_secs(5)).await;

        // Should succeed (skill agent handles it)
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_dispatch_sync_no_agent() {
        let dispatcher = SubAgentDispatcher::new(); // No agents registered

        let request = SubAgentRequest::new("Do something");
        let result = dispatcher.dispatch_sync(request, Duration::from_secs(5)).await;

        // Should fail because no agents
        assert!(result.is_ok()); // We get a result, but it indicates failure
        let result = result.unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_dispatch_parallel_sync_empty() {
        let dispatcher = SubAgentDispatcher::new();

        let results = dispatcher.dispatch_parallel_sync(vec![], Duration::from_secs(5)).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_parallel_sync_multiple() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = SubAgentDispatcher::with_defaults(registry);

        let requests = vec![
            (SubAgentRequest::new("Task 1"), None),
            (SubAgentRequest::new("Task 2"), None),
        ];

        let results = dispatcher.dispatch_parallel_sync(requests, Duration::from_secs(5)).await;

        assert_eq!(results.len(), 2);
        for (id, result) in results {
            assert!(!id.is_empty());
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_summary_to_record_completed() {
        let summary = ToolCallSummary {
            id: "call-1".to_string(),
            tool: "web_fetch".to_string(),
            state: super::super::coordinator::ToolCallState {
                status: "completed".to_string(),
                title: Some("Fetched page".to_string()),
            },
        };

        let record = summary_to_record(&summary);
        assert_eq!(record.name, "web_fetch");
        assert!(record.success);
        assert_eq!(record.result_summary, "Fetched page");
    }

    #[test]
    fn test_summary_to_record_error() {
        let summary = ToolCallSummary {
            id: "call-2".to_string(),
            tool: "search".to_string(),
            state: super::super::coordinator::ToolCallState {
                status: "error".to_string(),
                title: Some("Connection timeout".to_string()),
            },
        };

        let record = summary_to_record(&summary);
        assert_eq!(record.name, "search");
        assert!(!record.success);
        assert_eq!(record.result_summary, "Connection timeout");
    }

    #[tokio::test]
    async fn test_dispatch_with_authority_denied() {
        use super::super::authority::{ConfigDrivenAuthority, SubagentAuthority};
        use crate::config::types::agents_def::SubagentPolicy;
        use std::collections::HashMap as PolicyMap;

        let mut policies = PolicyMap::new();
        policies.insert("restricted".to_string(), SubagentPolicy {
            allow: vec!["allowed_agent".to_string()],
        });
        let authority = Arc::new(ConfigDrivenAuthority::from_policies(policies));

        let dispatcher = SubAgentDispatcher::new()
            .with_authority(authority);

        let mut request = SubAgentRequest::new("test task");
        request = request
            .with_context("parent_agent_id", serde_json::Value::String("restricted".to_string()))
            .with_context("agent_id", serde_json::Value::String("forbidden_agent".to_string()));

        let result = dispatcher.dispatch_sync(request, Duration::from_secs(5)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not authorized"), "Expected auth error, got: {}", err);
    }

    #[tokio::test]
    async fn test_dispatch_without_authority_allows_all() {
        // Without authority set, dispatch proceeds (may fail for other reasons, but not auth)
        let dispatcher = SubAgentDispatcher::new();
        let request = SubAgentRequest::new("test task")
            .with_context("parent_agent_id", serde_json::Value::String("any".to_string()));

        let result = dispatcher.dispatch_sync(request, Duration::from_secs(1)).await;
        // Should not be an auth error (may timeout or fail for other reasons)
        if let Err(ref e) = result {
            assert!(!e.to_string().contains("not authorized"), "Should not get auth error without authority");
        }
    }
}
