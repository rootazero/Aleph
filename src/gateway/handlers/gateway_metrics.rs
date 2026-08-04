//! `gateway.metrics.lanes` / `gateway.metrics.run_concurrency` /
//! `gateway.metrics.subagent_concurrency` — live occupancy gauges for
//! diagnostics.
//!
//! `lanes` returns the snapshot produced by [`LaneManager::snapshot`] as a
//! JSON array, `run_concurrency` returns the engine's run-lifetime
//! `ConcurrencyLimiter` snapshot (Task 4/8, audit 3.4) — "N/M run slots in
//! use" — and `subagent_concurrency` (Round-8, §4.11) returns the
//! `BackgroundAgentTracker` occupancy: live sub-agents per session plus
//! completed / consumed counts so a panel can surface the
//! `consumed / completed` dedup-hygiene ratio without scraping
//! `subagent.list`. All three live on the Query lane (registered in
//! `Lane::override_for`).

use crate::agents::background_tracker::BackgroundAgentTracker;
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

/// Round-8 (§4.11) — Handle `gateway.metrics.subagent_concurrency`. Returns
/// the live background-sub-agent occupancy snapshot from
/// [`BackgroundAgentTracker::subagent_snapshot`]: live sub-agents per
/// session (sorted by session key), the presence-only subtotal (sync
/// fan-out seats that are not enumerated by the `subagent` tool but DO
/// count against the parent's Interrupt-demote budget), and the
/// `completed_total` / `consumed_total` pair so a panel can surface the
/// `consumed / completed` dedup-hygiene ratio.
///
/// Process-wide by default; pass `params = {"scope": "agent:<id>:peer:user"}`
/// to limit to one session.
pub async fn handle_gateway_metrics_subagent_concurrency(
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let scope = request
        .params
        .as_ref()
        .and_then(|p| p.get("scope"))
        .and_then(|v| v.as_str());
    let snap = BackgroundAgentTracker::global().subagent_snapshot(scope);
    JsonRpcResponse::success(request.id, json!({ "subagent_concurrency": snap }))
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

    /// Round-8 — `gateway.metrics.subagent_concurrency` reads the
    /// process-global `BackgroundAgentTracker` directly. We seed a couple of
    /// entries (with unique ids so the process-global map stays isolated
    /// across cargo-test invocations) and assert the snapshot reflects them.
    #[tokio::test]
    async fn subagent_concurrency_reports_tracker_occupancy() {
        use crate::agents::background_tracker::{CompletedOutcome, SpawnMeta};
        use tokio_util::sync::CancellationToken;

        let tracker = crate::agents::background_tracker::BackgroundAgentTracker::global();
        let live_id = format!("gmc-live-{}", uuid::Uuid::new_v4());
        let done_id = format!("gmc-done-{}", uuid::Uuid::new_v4());
        tracker.register_with_meta(
            live_id.clone(),
            CancellationToken::new(),
            "live task".into(),
            SpawnMeta {
                root_session: "agent:main:peer:user".into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        tracker.register_with_meta(
            done_id.clone(),
            CancellationToken::new(),
            "done task".into(),
            SpawnMeta {
                root_session: "agent:main:peer:user".into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        tracker.mark_completed(&done_id, CompletedOutcome::ok_text("x"));
        tracker.mark_consumed(&done_id);

        // Process-wide view.
        let req = JsonRpcRequest::with_id("gateway.metrics.subagent_concurrency", None, json!(1));
        let resp = handle_gateway_metrics_subagent_concurrency(req).await;
        assert!(resp.is_success());
        let result = resp.result.expect("ok");
        let snap = &result["subagent_concurrency"];
        // `running_total` reflects the process-global set, which other
        // tests in this run may have populated; the exact number is
        // unstable. Instead we assert OUR id is in `running_per_session`.
        let per_session = snap["running_per_session"]
            .as_array()
            .expect("running_per_session is an array");
        let own_session = per_session
            .iter()
            .find(|r| r["session"] == "agent:main:peer:user")
            .expect("our seeded session must appear");
        assert!(
            own_session["count"].as_u64().unwrap() >= 1,
            "seeded live id must count toward the session's running tally"
        );
        // Completed / consumed are also process-global, so we just check
        // our id was seen by the dedup counter:
        assert!(snap["consumed_total"].as_u64().unwrap() >= 1);

        // Scoped view: ask for the seeded session and assert the count
        // matches.
        let req_scoped = JsonRpcRequest::with_id(
            "gateway.metrics.subagent_concurrency",
            Some(json!({ "scope": "agent:main:peer:user" })),
            json!(2),
        );
        let resp_scoped = handle_gateway_metrics_subagent_concurrency(req_scoped).await;
        assert!(resp_scoped.is_success());
        let snap_scoped = resp_scoped.result.unwrap();
        let own_session_scoped = snap_scoped["subagent_concurrency"]["running_per_session"]
            .as_array()
            .expect("array")
            .iter()
            .find(|r| r["session"] == "agent:main:peer:user")
            .expect("our session in scoped view");
        assert_eq!(
            own_session_scoped["count"], own_session["count"],
            "scoped view must report the same per-session count for the seeded session"
        );
    }
}
