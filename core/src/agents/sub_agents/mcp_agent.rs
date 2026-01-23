//! MCP Sub-Agent
//!
//! A specialized sub-agent for interacting with MCP (Model Context Protocol) tools.
//! This agent is delegated to when the main agent needs to discover and understand
//! external MCP servers and their tools.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::traits::{SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult, ToolCallRecord};
use crate::dispatcher::{ToolRegistry, ToolSource, UnifiedTool};
use crate::error::Result;

/// MCP Sub-Agent for discovering and interacting with MCP tools
///
/// This sub-agent specializes in:
/// - Listing available MCP tools from connected servers
/// - Providing tool schemas and descriptions
/// - Recommending appropriate MCP tools for tasks
pub struct McpSubAgent {
    /// Unique ID for this sub-agent
    id: String,
    /// Tool registry for accessing MCP tools
    registry: Arc<RwLock<ToolRegistry>>,
}

impl McpSubAgent {
    /// Create a new MCP sub-agent
    pub fn new(registry: Arc<RwLock<ToolRegistry>>) -> Self {
        Self {
            id: "mcp_agent".to_string(),
            registry,
        }
    }

    /// Get all MCP tools from the registry
    async fn get_mcp_tools(&self) -> Vec<UnifiedTool> {
        let registry = self.registry.read().await;
        registry
            .list_all()
            .await
            .into_iter()
            .filter(|tool| matches!(tool.source, ToolSource::Mcp { .. }))
            .collect()
    }

    /// Get MCP tools for a specific server
    async fn get_server_tools(&self, server_name: &str) -> Vec<UnifiedTool> {
        let registry = self.registry.read().await;
        registry.list_by_mcp_server(server_name).await
    }

    /// List available MCP servers
    async fn list_servers(&self) -> Vec<String> {
        let tools = self.get_mcp_tools().await;
        let mut servers: Vec<String> = tools
            .iter()
            .filter_map(|tool| {
                if let ToolSource::Mcp { server } = &tool.source {
                    Some(server.clone())
                } else {
                    None
                }
            })
            .collect();
        servers.sort();
        servers.dedup();
        servers
    }

    /// Find tools matching a query
    fn find_matching_tools<'a>(&self, prompt: &str, tools: &'a [UnifiedTool]) -> Vec<&'a UnifiedTool> {
        let prompt_lower = prompt.to_lowercase();
        let keywords: Vec<&str> = prompt_lower.split_whitespace().collect();

        tools
            .iter()
            .filter(|tool| {
                let name_lower = tool.name.to_lowercase();
                let desc_lower = tool.description.to_lowercase();

                // Check for keyword matches
                keywords.iter().any(|kw| {
                    kw.len() > 2 && (name_lower.contains(kw) || desc_lower.contains(kw))
                })
            })
            .collect()
    }

    /// Format tool info for response
    fn format_tool_info(&self, tool: &UnifiedTool) -> String {
        let server = if let ToolSource::Mcp { server } = &tool.source {
            server.clone()
        } else {
            "unknown".to_string()
        };

        let params = tool
            .parameters_schema
            .as_ref()
            .map(|s| serde_json::to_string_pretty(s).unwrap_or_default())
            .unwrap_or_else(|| "{}".to_string());

        format!(
            "**{}** (server: {})\n{}\nParameters:\n```json\n{}\n```",
            tool.name, server, tool.description, params
        )
    }
}

#[async_trait]
impl SubAgent for McpSubAgent {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        "MCP Agent"
    }

    fn description(&self) -> &str {
        "Specialized agent for discovering and understanding MCP (Model Context Protocol) tools from external servers"
    }

    fn capabilities(&self) -> Vec<SubAgentCapability> {
        vec![SubAgentCapability::McpToolExecution]
    }

    fn can_handle(&self, request: &SubAgentRequest) -> bool {
        // Can handle if:
        // 1. Target is specified and is an MCP server
        // 2. Prompt mentions MCP-related keywords
        if let Some(ref target) = request.target {
            return !target.is_empty();
        }

        let prompt_lower = request.prompt.to_lowercase();
        prompt_lower.contains("mcp")
            || prompt_lower.contains("github")
            || prompt_lower.contains("notion")
            || prompt_lower.contains("slack")
            || prompt_lower.contains("external tool")
    }

    async fn execute(&self, request: SubAgentRequest) -> Result<SubAgentResult> {
        info!("MCP Agent executing request: {}", request.id);

        let server_name = request.target.clone();

        // Get available tools
        let available_tools = if let Some(ref server) = server_name {
            self.get_server_tools(server).await
        } else {
            self.get_mcp_tools().await
        };

        if available_tools.is_empty() {
            let servers = self.list_servers().await;
            return Ok(SubAgentResult::success(
                &request.id,
                if servers.is_empty() {
                    "No MCP servers are currently connected. To use MCP tools, first configure and connect an MCP server.".to_string()
                } else {
                    format!(
                        "No MCP tools available from server '{}'. Connected servers: {}",
                        server_name.unwrap_or_default(),
                        servers.join(", ")
                    )
                },
            )
            .with_output(json!({
                "connected_servers": servers,
                "tool_count": 0,
            })));
        }

        debug!("MCP Agent has {} tools available", available_tools.len());

        // Find matching tools based on the prompt
        let matching_tools = self.find_matching_tools(&request.prompt, &available_tools);

        if !matching_tools.is_empty() {
            // Found matching tools - provide detailed info
            let tool_infos: Vec<String> = matching_tools
                .iter()
                .take(5)
                .map(|t| self.format_tool_info(t))
                .collect();

            let summary = format!(
                "Found {} relevant MCP tool(s) for your request:\n\n{}",
                matching_tools.len(),
                tool_infos.join("\n\n")
            );

            let tool_names: Vec<_> = matching_tools.iter().map(|t| t.name.clone()).collect();

            Ok(SubAgentResult::success(&request.id, summary)
                .with_output(json!({
                    "matching_tools": tool_names,
                    "tool_count": matching_tools.len(),
                    "recommendation": if matching_tools.len() == 1 {
                        format!("Use the '{}' tool to complete this task.", matching_tools[0].name)
                    } else {
                        "Review the tools above and select the most appropriate one.".to_string()
                    }
                }))
                .with_tools_called(vec![ToolCallRecord {
                    name: "list_mcp_tools".to_string(),
                    arguments: json!({"query": request.prompt}),
                    success: true,
                    result_summary: format!("Found {} matching tools", matching_tools.len()),
                }]))
        } else {
            // No matching tools - list all available
            let tool_list: Vec<_> = available_tools
                .iter()
                .map(|t| {
                    let server = if let ToolSource::Mcp { server } = &t.source {
                        server.clone()
                    } else {
                        "?".to_string()
                    };
                    format!("- **{}** [{}]: {}", t.name, server, t.description)
                })
                .collect();

            Ok(SubAgentResult::success(
                &request.id,
                format!(
                    "Available MCP tools ({} total):\n\n{}",
                    available_tools.len(),
                    tool_list.join("\n")
                ),
            )
            .with_output(json!({
                "available_tools": available_tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
                "tool_count": available_tools.len(),
                "servers": self.list_servers().await,
            })))
        }
    }

    fn available_actions(&self) -> Vec<String> {
        vec![
            "list_tools".to_string(),
            "list_servers".to_string(),
            "get_tool_info".to_string(),
            "find_tools".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolRegistry;

    #[tokio::test]
    async fn test_mcp_agent_creation() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = McpSubAgent::new(registry);

        assert_eq!(agent.id(), "mcp_agent");
        assert_eq!(agent.name(), "MCP Agent");
        assert!(agent.capabilities().contains(&SubAgentCapability::McpToolExecution));
    }

    #[tokio::test]
    async fn test_mcp_agent_can_handle() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = McpSubAgent::new(registry);

        // With target
        let request = SubAgentRequest::new("List PRs").with_target("github");
        assert!(agent.can_handle(&request));

        // With MCP keyword
        let request = SubAgentRequest::new("Use MCP to fetch data");
        assert!(agent.can_handle(&request));

        // Without relevant keywords
        let request = SubAgentRequest::new("Read a file");
        assert!(!agent.can_handle(&request));
    }

    #[tokio::test]
    async fn test_mcp_agent_no_tools() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = McpSubAgent::new(registry);

        let request = SubAgentRequest::new("List PRs").with_target("github");
        let result = agent.execute(request).await.unwrap();

        // Should succeed with info about no tools
        assert!(result.success);
        assert!(result.summary.contains("No MCP"));
    }
}
