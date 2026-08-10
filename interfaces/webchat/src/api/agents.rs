//! Panel API for agent management (agents.* RPC calls)

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelConfig {
    pub primary: String,
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDetail {
    pub definition: Value,
    pub file_count: usize,
}

/// What an `agents.update` actually did, beyond "the TOML write succeeded".
///
/// Mirrors `gateway::handlers::agents::handle_update`'s response. Only
/// `allowed_users` has a runtime half (`AgentRegistry::set_allowed_users`), so
/// this is the one field where "saved" and "in force" are different claims and
/// the page must not collapse them.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AgentUpdateOutcome {
    /// The patched admission list reached the live registry, so a revocation
    /// binds on the refused caller's next turn. Absent ⇒ `false`.
    #[serde(default)]
    pub allowed_users_applied_live: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGroupInfo {
    pub id: String,
    pub name: String,
    pub tools: Vec<ToolInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsSchemaResponse {
    pub groups: Vec<ToolGroupInfo>,
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
        archetype: Option<&str>,
    ) -> Result<(), String> {
        let params = json!({
            "id": id,
            "name": name,
            "identity": identity,
            "archetype": archetype,
        });
        state.rpc_call("agents.create", params).await?;
        Ok(())
    }

    /// Patch an agent. The outcome carries whether the admission list — the
    /// only patchable field with a runtime half — actually reached the live
    /// registry, so the page can say "in force now" instead of guessing.
    pub async fn update(
        state: &DashboardState,
        id: &str,
        patch: Value,
    ) -> Result<AgentUpdateOutcome, String> {
        let params = json!({"id": id, "patch": patch});
        let result = state.rpc_call("agents.update", params).await?;
        // A server that predates the field, or one built without a runtime
        // context, simply omits it — and `false` ("not in force") is the right
        // reading of an absent flag, because the reassuring answer is the one
        // that must be earned.
        Ok(serde_json::from_value(result).unwrap_or_default())
    }

    pub async fn delete(state: &DashboardState, id: &str) -> Result<(), String> {
        state.rpc_call("agents.delete", json!({"id": id})).await?;
        Ok(())
    }

    pub async fn set_default(state: &DashboardState, id: &str) -> Result<(), String> {
        state
            .rpc_call("agents.set_default", json!({"id": id}))
            .await?;
        Ok(())
    }

    // Files

    pub async fn files_list(
        state: &DashboardState,
        agent_id: &str,
    ) -> Result<FilesListResponse, String> {
        let result = state
            .rpc_call("agents.files.list", json!({"agent_id": agent_id}))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn files_get(
        state: &DashboardState,
        agent_id: &str,
        filename: &str,
    ) -> Result<String, String> {
        let result = state
            .rpc_call(
                "agents.files.get",
                json!({"agent_id": agent_id, "filename": filename}),
            )
            .await?;
        result
            .get("content")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .ok_or_else(|| "Missing content in response".to_string())
    }

    pub async fn files_set(
        state: &DashboardState,
        agent_id: &str,
        filename: &str,
        content: &str,
    ) -> Result<(), String> {
        state
            .rpc_call(
                "agents.files.set",
                json!({
                    "agent_id": agent_id,
                    "filename": filename,
                    "content": content,
                }),
            )
            .await?;
        Ok(())
    }

    // Tools schema

    pub async fn tools_schema(state: &DashboardState) -> Result<ToolsSchemaResponse, String> {
        let result = state.rpc_call("agents.tools_schema", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}
