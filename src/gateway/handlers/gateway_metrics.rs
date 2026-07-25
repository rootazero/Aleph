//! `gateway.metrics.lanes` / `gateway.metrics.run_concurrency` — live
//! occupancy gauges for diagnostics.
//!
//! `lanes` returns the snapshot produced by [`LaneManager::snapshot`] as a
//! JSON array, and `run_concurrency` returns the engine's run-lifetime
//! `ConcurrencyLimiter` snapshot (Task 4/8, audit 3.4) — "N/M run slots in
//! use". Both are suitable for ops dashboards / panel UIs to detect
//! saturation before it manifests as user-visible timeouts.
//!
//! Both live on the Query lane (registered in `Lane::override_for`).

use crate::sync_primitives::Arc;

use serde_json::json;

use super::super::lane::LaneManager;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use super::agent::AgentRunManager;

/// Handle `gateway.metrics.lanes`. Returns the live occupancy snapshot of
/// every lane in a fixed order (Query / Execute / Mutate / System).
pub async fn handle_gateway_metrics_lanes(
    request: JsonRpcRequest,
    lane_manager: Arc<LaneManager>,
) -> JsonRpcResponse {
    let lanes = lane_manager.snapshot();
    JsonRpcResponse::success(request.id, json!({ "lanes": lanes }))
}

/// Handle `gateway.metrics.run_concurrency`. Returns the live run-slot
/// occupancy snapshot from the execution engine's `ConcurrencyLimiter` (Task
/// 4) — "N/M run slots in use", per-agent breakdown, and queue depth — plus
/// `running_sessions`, the authoritative set of session keys with an in-flight
/// run (from the `SessionRunRegistry`). The latter lets a Panel paint
/// per-session running indicators on a fresh load and for runs started by any
/// interface, independent of client-side run-event refcounting.
pub async fn handle_gateway_metrics_run_concurrency(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    let run_concurrency = run_manager.concurrency_snapshot();
    let running_sessions = run_manager.running_sessions();
    // Backlog waiting *behind* the run slots. Without it the gauge showed a
    // saturated engine but not the queue depth piling up behind it, and a
    // queued message was invisible on every surface (codex renders queued
    // messages in its bottom pane; Aleph showed nothing at all).
    let busy = crate::gateway::busy_queue::snapshot();
    let per_session: Vec<_> = busy
        .per_session
        .iter()
        .map(|(session_key, depth)| json!({ "session_key": session_key, "depth": depth }))
        .collect();
    JsonRpcResponse::success(
        request.id,
        json!({
            "run_concurrency": run_concurrency,
            "running_sessions": running_sessions,
            "busy_queue": {
                "total_waiting": busy.total_waiting,
                "per_session": per_session,
            },
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::lane::LaneConfig;
    use serde_json::json;

    #[tokio::test]
    async fn returns_one_entry_per_lane_in_fixed_order() {
        let manager = Arc::new(LaneManager::new(LaneConfig::default()));
        let req = JsonRpcRequest::with_id("gateway.metrics.lanes", None, json!(1));
        let resp = handle_gateway_metrics_lanes(req, manager).await;

        assert!(resp.is_success());
        let result = resp.result.unwrap();
        let lanes = result["lanes"].as_array().expect("lanes must be array");
        assert_eq!(lanes.len(), 4);
        assert_eq!(lanes[0]["lane"], "Query");
        assert_eq!(lanes[1]["lane"], "Execute");
        assert_eq!(lanes[2]["lane"], "Mutate");
        assert_eq!(lanes[3]["lane"], "System");

        // Single-pool lanes omit the desktop split (None → JSON null).
        assert!(lanes[0]["desktop_total"].is_null());
        assert!(lanes[0]["desktop_available"].is_null());

        // Channel-class-split lanes carry both pool sizes.
        assert!(lanes[1]["desktop_total"].as_u64().is_some());
        assert!(lanes[1]["shared_total"].as_u64().is_some());
    }

    /// Minimal `ToolRegistry` double: this test never looks up or executes a
    /// tool, so an empty registry satisfies `ExecutionEngine<P, R>`'s generic
    /// bound without pulling in the real (heavy) `BuiltinToolRegistry`.
    /// Mirrors `execution_engine::tests::EmptyToolRegistry`.
    struct EmptyToolRegistry;

    impl crate::executor::ToolRegistry for EmptyToolRegistry {
        fn get_tool(&self, _name: &str) -> Option<&crate::tool_metadata::UnifiedTool> {
            None
        }

        fn execute_tool(
            &self,
            _tool_name: &str,
            _arguments: serde_json::Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = crate::error::Result<serde_json::Value>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async { Err(crate::error::AlephError::tool("no tools in test registry")) })
        }
    }

    /// Wires a real `ExecutionEngine` (default config: `max_runs_global = 8`)
    /// through the exact same `Arc<dyn ExecutionAdapter>` → `AgentRunManager`
    /// path production boot uses, so this proves the whole access chain —
    /// not just a hand-built double — reports the true configured default.
    #[tokio::test]
    async fn run_concurrency_reports_default_global_total() {
        use crate::gateway::agent_instance::AgentRegistry;
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::execution_adapter::ExecutionAdapter;
        use crate::gateway::execution_engine::{ExecutionEngine, ExecutionEngineConfig};
        use crate::gateway::router::AgentRouter;

        let engine = ExecutionEngine::new(
            ExecutionEngineConfig::default(),
            Arc::new(crate::thinker::SingleProviderRegistry::new(
                crate::providers::create_mock_provider(),
            )),
            Arc::new(EmptyToolRegistry),
            Vec::new(),
            None,
        );
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(engine);
        let run_manager = Arc::new(AgentRunManager::new(
            Arc::new(AgentRouter::new()),
            Arc::new(GatewayEventBus::new()),
            Arc::new(AgentRegistry::new()),
            execution_adapter,
        ));

        let req = JsonRpcRequest::with_id("gateway.metrics.run_concurrency", None, json!(1));
        let resp = handle_gateway_metrics_run_concurrency(req, run_manager).await;

        assert!(resp.is_success());
        let result = resp.result.unwrap();
        assert_eq!(result["run_concurrency"]["global_total"], 8);
        assert_eq!(result["run_concurrency"]["global_in_use"], 0);
        // Second-optimization fields: per-agent sub-cap, idle queue depth, and
        // an empty per-agent breakdown when nothing is running.
        assert_eq!(result["run_concurrency"]["per_agent_cap"], 3);
        assert_eq!(result["run_concurrency"]["waiting"], 0);
        assert!(result["run_concurrency"]["per_agent"]
            .as_array()
            .expect("per_agent is an array")
            .is_empty());
        // Sibling field sourced from the SessionRunRegistry — empty at rest.
        assert!(result["running_sessions"]
            .as_array()
            .expect("running_sessions is an array")
            .is_empty());
    }
}
