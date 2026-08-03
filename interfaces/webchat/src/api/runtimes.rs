//! RPC client for runtimes.* gateway methods.

use serde::{Deserialize, Serialize};

use crate::context::DashboardState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Missing,
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

/// Topic carrying live `runtimes.install` progress. Must match
/// `RUNTIME_INSTALL_PROGRESS_TOPIC` in the core (`src/gateway/event_bus.rs`).
pub const RUNTIME_INSTALL_PROGRESS_TOPIC: &str = "runtimes.install.progress";

/// One progress event streamed during an install. The Panel subscribes to
/// [`RUNTIME_INSTALL_PROGRESS_TOPIC`] and renders this per runtime card.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct InstallProgress {
    /// Capability being installed (matches `RuntimeInfo::name`).
    pub step: String,
    /// "started" | "done" | "failed".
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub stderr: Option<String>,
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
