//! MCP Client - Service Registry and Tool Router
//!
//! Manages builtin and external MCP services, providing:
//! - Service registration
//! - Tool discovery and aggregation
//! - Tool call routing

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{AetherError, Result};
use crate::mcp::builtin::BuiltinMcpService;
use crate::mcp::types::{McpTool, McpToolResult};

/// MCP Client - the central registry for MCP services
pub struct McpClient {
    /// Registered builtin services
    builtin_services: Vec<Arc<dyn BuiltinMcpService>>,
    /// Tool name to service index mapping
    tool_service_map: HashMap<String, usize>,
}

impl McpClient {
    /// Create a new empty MCP client
    pub fn new() -> Self {
        Self {
            builtin_services: Vec::new(),
            tool_service_map: HashMap::new(),
        }
    }

    /// Register a builtin service
    pub fn register_builtin(&mut self, service: Arc<dyn BuiltinMcpService>) {
        let service_idx = self.builtin_services.len();

        // Map all tools from this service
        for tool in service.list_tools() {
            self.tool_service_map.insert(tool.name.clone(), service_idx);
        }

        self.builtin_services.push(service);

        tracing::info!(
            service = %self.builtin_services[service_idx].name(),
            tools = self.builtin_services[service_idx].list_tools().len(),
            "Registered MCP service"
        );
    }

    /// List all available tools from all services
    pub fn list_tools(&self) -> Vec<McpTool> {
        let mut tools = Vec::new();
        for service in &self.builtin_services {
            tools.extend(service.list_tools());
        }
        tools
    }

    /// Get tools as a formatted list for context injection
    pub fn get_tools_for_context(&self) -> Vec<(String, String, serde_json::Value)> {
        self.list_tools()
            .into_iter()
            .map(|t| (t.name, t.description, t.input_schema))
            .collect()
    }

    /// Check if a tool requires confirmation
    pub fn requires_confirmation(&self, tool_name: &str) -> bool {
        if let Some(&service_idx) = self.tool_service_map.get(tool_name) {
            return self.builtin_services[service_idx].requires_confirmation(tool_name);
        }
        // Default to requiring confirmation for unknown tools
        true
    }

    /// Call a tool by name
    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpToolResult> {
        // Find the service that provides this tool
        if let Some(&service_idx) = self.tool_service_map.get(name) {
            let service = &self.builtin_services[service_idx];
            return service.call_tool(name, args).await;
        }

        // Tool not found
        Err(AetherError::McpToolNotFound(name.to_string()))
    }

    /// Get list of registered service names
    pub fn service_names(&self) -> Vec<&str> {
        self.builtin_services.iter().map(|s| s.name()).collect()
    }

    /// Check if any services are registered
    pub fn has_services(&self) -> bool {
        !self.builtin_services.is_empty()
    }

    /// Get total number of available tools
    pub fn tool_count(&self) -> usize {
        self.tool_service_map.len()
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating McpClient with configuration
pub struct McpClientBuilder {
    client: McpClient,
}

impl McpClientBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            client: McpClient::new(),
        }
    }

    /// Add a builtin service
    pub fn with_builtin(mut self, service: Arc<dyn BuiltinMcpService>) -> Self {
        self.client.register_builtin(service);
        self
    }

    /// Build the client
    pub fn build(self) -> McpClient {
        self.client
    }
}

impl Default for McpClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::types::McpResource;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockService {
        name: &'static str,
        tools: Vec<McpTool>,
    }

    impl MockService {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                tools: vec![McpTool {
                    name: format!("{}_tool", name),
                    description: "A mock tool".to_string(),
                    input_schema: json!({"type": "object"}),
                    requires_confirmation: false,
                }],
            }
        }
    }

    #[async_trait]
    impl BuiltinMcpService for MockService {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "Mock service"
        }

        async fn list_resources(&self) -> Result<Vec<McpResource>> {
            Ok(vec![])
        }

        async fn read_resource(&self, _uri: &str) -> Result<String> {
            Ok("mock".to_string())
        }

        fn list_tools(&self) -> Vec<McpTool> {
            self.tools.clone()
        }

        async fn call_tool(&self, name: &str, _args: serde_json::Value) -> Result<McpToolResult> {
            Ok(McpToolResult::success(json!({"tool": name})))
        }

        fn requires_confirmation(&self, _tool_name: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn test_register_and_list_tools() {
        let mut client = McpClient::new();
        client.register_builtin(Arc::new(MockService::new("service1")));
        client.register_builtin(Arc::new(MockService::new("service2")));

        let tools = client.list_tools();
        assert_eq!(tools.len(), 2);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"service1_tool"));
        assert!(tool_names.contains(&"service2_tool"));
    }

    #[tokio::test]
    async fn test_call_tool() {
        let mut client = McpClient::new();
        client.register_builtin(Arc::new(MockService::new("test")));

        let result = client.call_tool("test_tool", json!({})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content["tool"], "test_tool");
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        let client = McpClient::new();

        let result = client.call_tool("unknown_tool", json!({})).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            AetherError::McpToolNotFound(name) => {
                assert_eq!(name, "unknown_tool");
            }
            _ => panic!("Expected McpToolNotFound error"),
        }
    }

    #[test]
    fn test_builder() {
        let client = McpClientBuilder::new()
            .with_builtin(Arc::new(MockService::new("builder_test")))
            .build();

        assert!(client.has_services());
        assert_eq!(client.tool_count(), 1);
    }
}
