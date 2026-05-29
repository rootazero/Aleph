use crate::context::DashboardState;
use serde_json::Value;

/// Agent ↔ channel binding RPC surface.
///
/// Named `WorkspaceApi` for historical reasons — the original `workspace.list`
/// method was dead (no consumer; the project-folder switcher lives in
/// [`crate::api::projects`]) and was removed. Only the agent-binding calls
/// below have live consumers (`agent_binding_selector`, `agents_sidebar`).
pub struct WorkspaceApi;

impl WorkspaceApi {
    /// Set the agent binding for a channel (or unbind with None)
    pub async fn set_channel_agent(
        state: &DashboardState,
        channel_id: &str,
        agent_id: Option<&str>,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "channel_id": channel_id,
            "agent_id": agent_id,
        });
        state.rpc_call("channels.set_agent", params).await?;
        Ok(())
    }

    /// Get all agent→channel bindings
    pub async fn agent_bindings(
        state: &DashboardState,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let result = state.rpc_call("agents.bindings", Value::Null).await?;
        result
            .get("bindings")
            .ok_or_else(|| "Invalid response: missing bindings".to_string())
            .and_then(|b| {
                serde_json::from_value(b.clone())
                    .map_err(|e| format!("Failed to parse bindings: {}", e))
            })
    }
}
