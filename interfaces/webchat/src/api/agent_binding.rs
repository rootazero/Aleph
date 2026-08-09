use crate::context::DashboardState;
use serde_json::Value;

/// Agent ↔ channel binding RPC surface — `channels.set_agent`,
/// `agents.bindings`.
///
/// This was `api::workspace::WorkspaceApi` until 2026-08-09, and its own doc
/// said the name was historical: the original `workspace.list` call had been
/// removed as dead (no consumer; the project-folder switcher lives in
/// [`crate::api::projects`]), leaving a module named after a family it no
/// longer spoke to. The name was harmless while nothing else wanted it and
/// became a trap the moment something did — `/settings/workspaces` needed a
/// real `WorkspaceApi`, and two types one letter apart in the same namespace is
/// how a call ends up on the wrong surface.
///
/// So the misnomer was renamed rather than worked around, and
/// [`crate::api::workspace`] is now what it says it is.
pub struct AgentBindingApi;

impl AgentBindingApi {
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

    /// Get all agent→channels bindings (many-to-one: an agent may be bound to
    /// several channels; the server returns the full sorted list per agent).
    pub async fn agent_bindings(
        state: &DashboardState,
    ) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
        let result = state.rpc_call("agents.bindings", Value::Null).await?;
        result
            .get("bindings")
            .ok_or_else(|| "Invalid response: missing bindings".to_string())
            .and_then(|b| {
                serde_json::from_value(b.clone())
                    .map_err(|e| format!("Failed to parse bindings: {e}"))
            })
    }
}
