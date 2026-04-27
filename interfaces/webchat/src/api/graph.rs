use crate::canvas_engine::adapter::*;
use crate::context::DashboardState;
use serde_json::json;

pub struct GraphApi;

impl GraphApi {
    pub async fn query(
        state: &DashboardState,
        agent_id: &str,
        limit: usize,
        kind_filter: Vec<String>,
    ) -> Result<GraphQueryResponse, String> {
        let params = json!({
            "agent_id": agent_id,
            "limit": limit,
            "kind_filter": kind_filter,
        });
        let result = state.rpc_call("graph.query", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.query: {}", e))
    }

    pub async fn neighbors(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
        depth: u8,
        limit: usize,
    ) -> Result<GraphNeighborsResponse, String> {
        let params = json!({
            "agent_id": agent_id,
            "node_id": node_id,
            "depth": depth,
            "limit": limit,
        });
        let result = state.rpc_call("graph.neighbors", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.neighbors: {}", e))
    }

    pub async fn node_detail(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
    ) -> Result<NoteDetailResponse, String> {
        let params = json!({ "agent_id": agent_id, "node_id": node_id });
        let result = state.rpc_call("graph.node_detail", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.node_detail: {}", e))
    }

    pub async fn search(
        state: &DashboardState,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<GraphSearchResponse, String> {
        let params = json!({ "agent_id": agent_id, "query": query, "limit": limit });
        let result = state.rpc_call("graph.search", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.search: {}", e))
    }
}
