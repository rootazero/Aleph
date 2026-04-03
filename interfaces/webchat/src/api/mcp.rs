use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub requires_runtime: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
}

pub struct McpConfigApi;

impl McpConfigApi {
    /// List all MCP servers
    pub async fn list(state: &DashboardState) -> Result<Vec<McpServerInfo>, String> {
        let result = state
            .rpc_call("mcp_config.list", serde_json::Value::Null)
            .await?;

        let servers = result
            .get("servers")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(servers).map_err(|e| format!("Failed to parse MCP servers: {}", e))
    }

    /// Get a specific MCP server
    pub async fn get(state: &DashboardState, name: String) -> Result<McpServerInfo, String> {
        let params = serde_json::json!({
            "name": name,
        });

        let result = state.rpc_call("mcp_config.get", params).await?;

        let server = result.get("server").cloned().unwrap_or(result);
        serde_json::from_value(server).map_err(|e| format!("Failed to parse MCP server: {}", e))
    }

    /// Create a new MCP server
    pub async fn create(
        state: &DashboardState,
        name: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });

        state.rpc_call("mcp_config.create", params).await?;
        Ok(())
    }

    /// Update an existing MCP server
    pub async fn update(
        state: &DashboardState,
        name: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });

        state.rpc_call("mcp_config.update", params).await?;
        Ok(())
    }

    /// Delete an MCP server
    pub async fn delete(state: &DashboardState, name: String) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
        });

        state.rpc_call("mcp_config.delete", params).await?;
        Ok(())
    }
}
