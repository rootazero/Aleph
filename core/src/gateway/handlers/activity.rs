//! Activity stats handler — returns active agent runs + coordination tasks.

use serde::Serialize;
use serde_json::json;
use crate::sync_primitives::Arc;

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::protocol::JsonRpcRequest;
use crate::gateway::protocol::JsonRpcResponse;

#[derive(Debug, Serialize)]
struct ActivityStats {
    active_agent_runs: u64,
    active_coord_tasks: u64,
    active_total: u64,
}

pub async fn handle_stats(
    req: JsonRpcRequest,
    exec_adapter: Arc<dyn ExecutionAdapter>,
    coord_store: Option<Arc<dyn CoordTaskStore>>,
) -> JsonRpcResponse {
    let active_runs = exec_adapter.active_run_count().await as u64;

    let active_tasks = if let Some(store) = coord_store {
        match store
            .list_tasks(CoordTaskFilter {
                status: Some(CoordTaskStatus::InProgress),
                ..Default::default()
            })
            .await
        {
            Ok(tasks) => tasks.len() as u64,
            Err(_) => 0,
        }
    } else {
        0
    };

    let stats = ActivityStats {
        active_agent_runs: active_runs,
        active_coord_tasks: active_tasks,
        active_total: active_runs + active_tasks,
    };

    JsonRpcResponse::success(req.id, json!(stats))
}
