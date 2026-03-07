//! Panel API for agent management (agents.* RPC calls)

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::context::DashboardState;

// -- Types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsListResponse {
    pub agents: Vec<AgentSummary>,
    pub default_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelConfig {
    pub primary: String,
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDetail {
    pub definition: Value,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceFile {
    pub filename: String,
    pub size_bytes: u64,
    pub modified_at: i64,
    pub is_bootstrap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesListResponse {
    pub files: Vec<WorkspaceFile>,
}

// -- API --

pub struct AgentsApi;

impl AgentsApi {
    pub async fn list(state: &DashboardState) -> Result<AgentsListResponse, String> {
        let result = state.rpc_call("agents.list", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, id: &str) -> Result<AgentDetail, String> {
        let result = state.rpc_call("agents.get", json!({"id": id})).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn create(
        state: &DashboardState,
        id: &str,
        name: Option<&str>,
        identity: Option<&AgentIdentity>,
    ) -> Result<(), String> {
        let params = json!({
            "id": id,
            "name": name,
            "identity": identity,
        });
        state.rpc_call("agents.create", params).await?;
        Ok(())
    }

    pub async fn update(state: &DashboardState, id: &str, patch: Value) -> Result<(), String> {
        let params = json!({"id": id, "patch": patch});
        state.rpc_call("agents.update", params).await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, id: &str) -> Result<(), String> {
        state.rpc_call("agents.delete", json!({"id": id})).await?;
        Ok(())
    }

    pub async fn set_default(state: &DashboardState, id: &str) -> Result<(), String> {
        state.rpc_call("agents.set_default", json!({"id": id})).await?;
        Ok(())
    }

    // Files

    pub async fn files_list(state: &DashboardState, agent_id: &str) -> Result<FilesListResponse, String> {
        let result = state.rpc_call("agents.files.list", json!({"agent_id": agent_id})).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn files_get(state: &DashboardState, agent_id: &str, filename: &str) -> Result<String, String> {
        let result = state.rpc_call("agents.files.get", json!({"agent_id": agent_id, "filename": filename})).await?;
        result.get("content").and_then(|v| v.as_str()).map(|s| s.to_string())
            .ok_or_else(|| "Missing content in response".to_string())
    }

    pub async fn files_set(state: &DashboardState, agent_id: &str, filename: &str, content: &str) -> Result<(), String> {
        state.rpc_call("agents.files.set", json!({
            "agent_id": agent_id,
            "filename": filename,
            "content": content,
        })).await?;
        Ok(())
    }

    pub async fn files_delete(state: &DashboardState, agent_id: &str, filename: &str) -> Result<(), String> {
        state.rpc_call("agents.files.delete", json!({
            "agent_id": agent_id,
            "filename": filename,
        })).await?;
        Ok(())
    }
}
