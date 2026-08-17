use crate::context::DashboardState;
use crate::memory_graph::adapter::{GraphQueryResponse, GraphSearchResponse, NoteDetailResponse};
use serde_json::json;

pub struct GraphApi;

impl GraphApi {
    pub async fn query(
        state: &DashboardState,
        agent_id: &str,
        limit: usize,
    ) -> Result<GraphQueryResponse, String> {
        let params = json!({
            "agent_id": agent_id,
            "limit": limit,
        });
        let result = state.rpc_call("graph.query", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.query: {e}"))
    }

    pub async fn node_detail(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
    ) -> Result<NoteDetailResponse, String> {
        let params = json!({ "agent_id": agent_id, "node_id": node_id });
        let result = state.rpc_call("graph.node_detail", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.node_detail: {e}"))
    }

    pub async fn search(
        state: &DashboardState,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<GraphSearchResponse, String> {
        let params = json!({ "agent_id": agent_id, "query": query, "limit": limit });
        let result = state.rpc_call("graph.search", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.search: {e}"))
    }

    /// Persist an edited note body. `content` is the full raw markdown
    /// (frontmatter + body); the server writes it verbatim and re-indexes.
    pub async fn update_note(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
        content: &str,
    ) -> Result<(), String> {
        let params = json!({ "agent_id": agent_id, "node_id": node_id, "content": content });
        state.rpc_call("graph.update_note", params).await?;
        Ok(())
    }

    /// Rename a note title. Returns the new canonical node id (store-canonical path).
    pub async fn rename_note(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
        new_title: &str,
    ) -> Result<String, String> {
        let params = json!({ "agent_id": agent_id, "node_id": node_id, "new_title": new_title });
        let result = state.rpc_call("graph.rename_note", params).await?;
        result
            .get("new_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| "rename_note: missing new_id in response".to_string())
    }

    /// Delete a note. Returns success after removal from the store.
    pub async fn delete_note(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
    ) -> Result<(), String> {
        let params = json!({ "agent_id": agent_id, "node_id": node_id });
        state.rpc_call("graph.delete_note", params).await?;
        Ok(())
    }
}
