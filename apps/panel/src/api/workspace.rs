use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

/// A workspace entry returned by workspace.list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

pub struct WorkspaceApi;

impl WorkspaceApi {
    /// List all available workspaces
    pub async fn list(state: &DashboardState) -> Result<Vec<WorkspaceEntry>, String> {
        let result = state.rpc_call("workspace.list", Value::Null).await?;

        result.get("workspaces")
            .ok_or_else(|| "Invalid response: missing workspaces".to_string())
            .and_then(|workspaces| {
                serde_json::from_value(workspaces.clone())
                    .map_err(|e| format!("Failed to parse workspaces: {}", e))
            })
    }

    /// Set the agent binding for a channel (or unbind with None)
    pub async fn set_channel_agent(
        state: &DashboardState,
        channel_id: &str,
        agent_id: Option<&str>,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "channel_id": channel_id,
            "agent_id": agent_id,
        });
        state.rpc_call("channels.set_agent", params).await?;
        Ok(())
    }

    /// Get all agent→channel bindings
    pub async fn agent_bindings(
        state: &DashboardState,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let result = state.rpc_call("agents.bindings", Value::Null).await?;
        result.get("bindings")
            .ok_or_else(|| "Invalid response: missing bindings".to_string())
            .and_then(|b| {
                serde_json::from_value(b.clone())
                    .map_err(|e| format!("Failed to parse bindings: {}", e))
            })
    }
}
