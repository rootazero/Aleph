//! Panel API for global tool permission management (config.* RPC calls).
//! Used by the Settings → Policies page.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

// -- Types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionsResponse {
    pub default: String,
    #[serde(default)]
    pub overrides: HashMap<String, String>,
}

// -- API --

pub struct ToolPermissionsApi;

impl ToolPermissionsApi {
    // Global (Policies) API

    pub async fn get_global(state: &DashboardState) -> Result<ToolPermissionsResponse, String> {
        let result = state
            .rpc_call("config.get_tool_permissions", Value::Null)
            .await?;
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
        state
            .rpc_call("config.update_tool_permissions", params)
            .await?;
        Ok(())
    }
}
