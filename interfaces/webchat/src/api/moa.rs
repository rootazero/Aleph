//! Panel-side wrapper for the `moa.*` gateway RPCs. Pure I/O (R4).

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MoaSlotDto {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoaPresetDto {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub advisors: Vec<MoaSlotDto>,
    pub aggregator: MoaSlotDto,
    /// One scalar on the wire, matching core's `MoaFanout`:
    /// `"per_iteration"` | `"user_turn"` | `"every_n:<N>"` (N >= 2).
    /// Defaults to the core default rather than `""`, which the server would
    /// reject on save with an opaque parse error.
    #[serde(default = "per_iteration")]
    pub fanout: String,
    #[serde(default)]
    pub advisor_timeout_secs: u64,
    #[serde(default)]
    pub advisor_max_tokens: Option<u32>,
    #[serde(default)]
    pub advisor_temperature: Option<f32>,
    #[serde(default)]
    pub aggregator_temperature: Option<f32>,
}

fn yes() -> bool {
    true
}

fn per_iteration() -> String {
    "per_iteration".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoaConfigDto {
    #[serde(default)]
    pub default_preset: Option<String>,
    #[serde(default)]
    pub save_traces: bool,
    #[serde(default)]
    pub presets: HashMap<String, MoaPresetDto>,
}

pub struct MoaApi;

impl MoaApi {
    /// Fetch the `[moa]` config section. On a fresh install with no `[moa]`
    /// section yet, the gateway returns JSON `null` (since `Config.moa` is
    /// `Option<MoaToml>`) — guard for that and fall back to the default.
    pub async fn list_presets(state: &DashboardState) -> Result<MoaConfigDto, String> {
        let v = state.rpc_call("moa.listPresets", Value::Null).await?;
        if v.is_null() {
            return Ok(MoaConfigDto::default());
        }
        serde_json::from_value(v).map_err(|e| format!("parse moa config: {e}"))
    }

    pub async fn save_preset(
        state: &DashboardState,
        name: &str,
        preset: &MoaPresetDto,
        make_default: bool,
    ) -> Result<(), String> {
        let mut params = serde_json::to_value(preset).map_err(|e| e.to_string())?;
        params["name"] = serde_json::json!(name);
        params["make_default"] = serde_json::json!(make_default);
        state.rpc_call("moa.savePreset", params).await.map(|_| ())
    }

    pub async fn delete_preset(state: &DashboardState, name: &str) -> Result<(), String> {
        state
            .rpc_call("moa.deletePreset", serde_json::json!({ "name": name }))
            .await
            .map(|_| ())
    }

    pub async fn set_default(state: &DashboardState, name: &str) -> Result<(), String> {
        state
            .rpc_call("moa.setDefault", serde_json::json!({ "name": name }))
            .await
            .map(|_| ())
    }

    pub async fn set_save_traces(state: &DashboardState, on: bool) -> Result<(), String> {
        state
            .rpc_call("moa.setSaveTraces", serde_json::json!({ "on": on }))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_presets() {
        let cfg = MoaConfigDto::default();
        assert!(cfg.presets.is_empty());
        assert!(cfg.default_preset.is_none());
        assert!(!cfg.save_traces);
    }
}
