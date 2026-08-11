use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    #[serde(default)]
    pub id: String,
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
    /// Invocation record from `mcp_config.list`. `None` when the server could
    /// not build the report (or predates the field) — an empty cell, which is
    /// a different statement from `calls: Some(0)` = "known never used".
    #[serde(default)]
    pub usage: Option<aleph_protocol::extension_usage::UsageSummary>,
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
        serde_json::from_value(servers).map_err(|e| format!("Failed to parse MCP servers: {e}"))
    }

    /// Get a specific MCP server by id
    pub async fn get(state: &DashboardState, id: String) -> Result<McpServerInfo, String> {
        let params = serde_json::json!({ "id": id });
        let result = state.rpc_call("mcp_config.get", params).await?;
        let server = result.get("server").cloned().unwrap_or(result);
        serde_json::from_value(server).map_err(|e| format!("Failed to parse MCP server: {e}"))
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

    /// Update an existing MCP server by id
    pub async fn update(
        state: &DashboardState,
        id: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({ "id": id, "config": config });
        state.rpc_call("mcp_config.update", params).await?;
        Ok(())
    }

    /// Delete an MCP server by id
    pub async fn delete(state: &DashboardState, id: String) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("mcp_config.delete", params).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twin of `alephcore`'s `mcp_config::tests::a_row_carries_usage_under_its_wire_name`.
    /// The value type (`UsageSummary`) is shared through `aleph_protocol`, so a
    /// field rename inside it breaks both crates at compile time; the container
    /// key below is the one thing still spelled twice, and a drift there fails
    /// open — this DTO would decode `None` forever and the column would go
    /// blank, which reads exactly like "nothing has ever been called".
    #[test]
    fn a_server_row_decodes_the_usage_column() {
        let row: McpServerInfo = serde_json::from_value(serde_json::json!({
            "id": "ctx7",
            "name": "ctx7",
            "command": "npx",
            "usage": { "calls": 12, "errors": 0, "idle_days": 3 }
        }))
        .expect("row must decode");
        let usage = row.usage.expect("usage column must be populated");
        assert_eq!(usage.display_calls(), Some(12));
        assert_eq!(usage.idle_days, Some(3));
        assert!(!usage.never_used());
    }

    /// The Panel ships independently of the daemon; a row from an older server
    /// has no `usage` key and must still decode, leaving the column empty
    /// rather than failing the whole list.
    #[test]
    fn a_row_without_usage_still_decodes() {
        let row: McpServerInfo = serde_json::from_value(serde_json::json!({
            "id": "old", "name": "old", "command": "npx"
        }))
        .expect("row must decode without the usage key");
        assert!(row.usage.is_none());
    }
}
