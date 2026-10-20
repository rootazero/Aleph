use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// General Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub default_provider: Option<String>,
    pub language: Option<String>,
}

pub struct GeneralConfigApi;

impl GeneralConfigApi {
    pub async fn get(state: &DashboardState) -> Result<GeneralConfig, String> {
        let result = state.rpc_call("general_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: GeneralConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("general_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Behavior Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    pub output_mode: String,
    pub typing_speed: u32,
}

pub struct BehaviorConfigApi;

impl BehaviorConfigApi {
    pub async fn get(state: &DashboardState) -> Result<BehaviorConfig, String> {
        let result = state.rpc_call("behavior_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: BehaviorConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("behavior_config.update", params).await?;
        Ok(())
    }
}

// ============================================================================
// Generation Config API
// ============================================================================

/// The `generation_config.*` body, from the protocol crate rather than a hand
/// copy here.
///
/// The copy this replaces declared `output_dir: String` while the server has
/// always sent `Option<String>`, so on an install that never set one the
/// response was `null` and serde failed the entire object — the settings
/// section showed a bare `invalid type: null, expected a string` and none of
/// its remaining controls. Sharing the type makes that shape of drift a compile
/// error.
pub use aleph_protocol::providers::GenerationSettings as GenerationConfig;

pub struct GenerationConfigApi;

impl GenerationConfigApi {
    pub async fn get(state: &DashboardState) -> Result<GenerationConfig, String> {
        let result = state.rpc_call("generation_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: GenerationConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("generation_config.update", params).await?;
        Ok(())
    }
}
