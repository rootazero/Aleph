//! Panel API for tool permission management (config.* and agent_config.* RPC calls)

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::context::DashboardState;

// -- Types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionsResponse {
    pub default: String,
    #[serde(default)]
    pub overrides: HashMap<String, String>,
    /// Effective default after global policy merge (agent-level only)
    #[serde(default)]
    pub effective_default: Option<String>,
    /// Effective per-tool permissions after global merge (agent-level only)
    #[serde(default)]
    pub effective: Option<HashMap<String, String>>,
    /// Global overrides for reference (agent-level only)
    #[serde(default)]
    pub global_overrides: Option<HashMap<String, String>>,
}

// -- API --

pub struct ToolPermissionsApi;

impl ToolPermissionsApi {
    // Global (Policies) API

    pub async fn get_global(state: &DashboardState) -> Result<ToolPermissionsResponse, String> {
        let result = state.rpc_call("config.get_tool_permissions", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update_global(
        state: &DashboardState,
        default: &str,
        overrides: &HashMap<String, String>,
    ) -> Result<(), String> {
        let params = json!({
            "default": default,
            "overrides": overrides,
        });
        state.rpc_call("config.update_tool_permissions", params).await?;
        Ok(())
    }

    // Per-agent API

    pub async fn get_agent(
        state: &DashboardState,
        agent_id: &str,
    ) -> Result<ToolPermissionsResponse, String> {
        let params = json!({ "agent_id": agent_id });
        let result = state.rpc_call("agent_config.get_tool_permissions", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update_agent(
        state: &DashboardState,
        agent_id: &str,
        default: &str,
        overrides: &HashMap<String, String>,
    ) -> Result<(), String> {
        let params = json!({
            "agent_id": agent_id,
            "default": default,
            "overrides": overrides,
        });
        state.rpc_call("agent_config.update_tool_permissions", params).await?;
        Ok(())
    }
}
