use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunRequest {
    pub message: String,
    pub session_key: String,
    pub thinking: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResponse {
    pub run_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub run_id: String,
    pub status: String,
    pub result: Option<Value>,
    pub error: Option<String>,
}

pub struct AgentApi;

impl AgentApi {
    /// Start agent execution
    pub async fn run(
        state: &DashboardState,
        request: AgentRunRequest,
    ) -> Result<AgentRunResponse, String> {
        let params = serde_json::to_value(&request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;

        let result = state.rpc_call("agent.run", params).await?;

        serde_json::from_value(result).map_err(|e| format!("Failed to parse response: {}", e))
    }

    /// Get agent run status
    pub async fn status(state: &DashboardState, run_id: String) -> Result<AgentStatus, String> {
        let params = serde_json::json!({
            "run_id": run_id,
        });

        let result = state.rpc_call("agent.status", params).await?;

        serde_json::from_value(result).map_err(|e| format!("Failed to parse status: {}", e))
    }

    /// Cancel a running agent
    pub async fn cancel(state: &DashboardState, run_id: String) -> Result<(), String> {
        let params = serde_json::json!({
            "run_id": run_id,
        });

        state.rpc_call("agent.cancel", params).await?;
        Ok(())
    }

    /// Force abort an agent
    pub async fn abort(state: &DashboardState, run_id: String) -> Result<(), String> {
        let params = serde_json::json!({
            "run_id": run_id,
        });

        state.rpc_call("agent.abort", params).await?;
        Ok(())
    }
}
