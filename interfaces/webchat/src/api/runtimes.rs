//! RPC client for runtimes.* gateway methods.

use serde::{Deserialize, Serialize};

use crate::context::DashboardState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Missing,
    Probing,
    Bootstrapping,
    Ready,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeInfo {
    pub name: String,
    pub status: RuntimeStatus,
    pub bin_path: Option<String>,
    pub version: Option<String>,
    pub llm_hint: Option<String>,
    pub deps: Vec<String>,
    pub supported_on_current_os: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimesListResponse {
    pub runtimes: Vec<RuntimeInfo>,
}

pub struct RuntimesApi;

impl RuntimesApi {
    pub async fn list(state: &DashboardState) -> Result<RuntimesListResponse, String> {
        let v = state
            .rpc_call("runtimes.list", serde_json::Value::Null)
            .await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    pub async fn refresh(state: &DashboardState) -> Result<RuntimesListResponse, String> {
        let v = state
            .rpc_call("runtimes.refresh", serde_json::Value::Null)
            .await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    pub async fn install(state: &DashboardState, capability: &str) -> Result<(), String> {
        let _ = state
            .rpc_call(
                "runtimes.install",
                serde_json::json!({ "capability": capability }),
            )
            .await?;
        Ok(())
    }
}
