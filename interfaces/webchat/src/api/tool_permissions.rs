//! Panel API for global tool permission management (config.* RPC calls).
//!
//! The single decoder for `config.get_tool_permissions` — Settings → Policies
//! edits the advanced axes, the composer's `ExecTierPicker` reads the tier and
//! its presets. One wire shape, one DTO: two hand-written decoders would drift.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

// -- Types --

/// One selectable execution-permission tier, exactly as Core defines it
/// (`builtin_tiers()`). Labels and descriptions are never authored here — the
/// Panel, CLI and bots must render the identical three.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierPreset {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionsResponse {
    /// Active tier id (`ask` / `auto` / `full`).
    pub exec_tier: String,
    /// Selectable tiers, ordered least → most permissive.
    #[serde(default)]
    pub tiers: Vec<TierPreset>,
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

    /// Set the execution tier. Partial update — the advanced per-tool overrides
    /// are left untouched. Returns the whole permission surface so the caller
    /// re-renders straight from Core.
    pub async fn set_exec_tier(
        state: &DashboardState,
        tier_id: &str,
    ) -> Result<ToolPermissionsResponse, String> {
        let result = state
            .rpc_call(
                "config.update_tool_permissions",
                json!({ "exec_tier": tier_id }),
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_deserializes_from_the_rpc_shape() {
        let v = json!({
            "exec_tier": "auto",
            "tiers": [{ "id": "ask", "label": "Ask 请求", "description": "d" }],
            "default": "allow",
            "overrides": { "bash": "ask" },
        });
        let cfg: ToolPermissionsResponse = serde_json::from_value(v).unwrap();
        assert_eq!(cfg.exec_tier, "auto");
        assert_eq!(cfg.tiers.len(), 1);
        assert_eq!(cfg.tiers[0].id, "ask");
        assert_eq!(cfg.default, "allow");
        assert_eq!(cfg.overrides.get("bash").map(String::as_str), Some("ask"));
    }
}
