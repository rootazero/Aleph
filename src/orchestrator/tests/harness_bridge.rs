//! Bridge-level smoke tests. Full end-to-end coverage lives in
//! tests/orchestrator_e2e.rs (Task 13).

use crate::sync_primitives::Arc;

use crate::orchestrator::harness_bridge::{
    active_execution_plan, compute_runtime_state_blocks, AgentHarnessRunner,
};
use crate::tool_metadata::{HealthReason, ProbeResult, ToolHealthProbe};
use crate::tools::runtime_state::ToolStatus;

#[test]
fn agent_harness_runner_is_send_sync() {
    fn _requires_send_sync<T: Send + Sync>() {}
    _requires_send_sync::<AgentHarnessRunner>();
}

#[test]
fn compute_runtime_state_blocks_empty_when_no_tool_catalog() {
    assert!(compute_runtime_state_blocks(None).is_empty());
}

#[tokio::test]
async fn active_execution_plan_none_for_empty_session_key() {
    // Empty key short-circuits in `scratchpad_registry::active` → no plan.
    assert!(active_execution_plan("").await.is_none());
}

#[tokio::test]
async fn active_execution_plan_none_for_unbound_session() {
    // A session that never touched the scratchpad has no registry binding,
    // so prompt assembly stays byte-identical (the layer renders nothing).
    assert!(active_execution_plan("unbound-session-key-no-scratchpad")
        .await
        .is_none());
}

#[test]
fn compute_runtime_state_blocks_empty_when_no_probes_registered() {
    let registry = Arc::new(crate::tool_metadata::ToolCatalog::new());
    assert!(compute_runtime_state_blocks(Some(&registry)).is_empty());
}

struct DeadProbe(&'static str);

#[async_trait::async_trait]
impl ToolHealthProbe for DeadProbe {
    async fn probe(&self) -> ProbeResult {
        ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(std::borrow::Cow::Borrowed(self.0)),
            retry_after: None,
        }
    }
}

#[tokio::test]
async fn compute_runtime_state_blocks_surfaces_unhealthy_probes() {
    let registry = Arc::new(crate::tool_metadata::ToolCatalog::new());
    let cache = registry.health();
    cache.register_probe("alpha", Arc::new(DeadProbe("alpha offline")));
    cache.register_probe("beta", Arc::new(DeadProbe("beta offline")));
    // Force-populate cache entries (snapshots only surface cached entries).
    let _ = cache.refresh("alpha").await;
    let _ = cache.refresh("beta").await;

    let blocks = compute_runtime_state_blocks(Some(&registry));
    assert_eq!(blocks.len(), 2);
    let names: Vec<&str> = blocks.iter().map(|b| b.tool_name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
    for block in &blocks {
        assert!(
            matches!(&block.status, ToolStatus::Unavailable { .. }),
            "expected Unavailable"
        );
        if let ToolStatus::Unavailable { reason } = &block.status {
            assert!(reason.contains("offline"));
        }
    }
}

#[tokio::test]
async fn compute_runtime_state_blocks_coalesces_shared_reason() {
    // A single downed dependency that gates many tools (an MCP server with N
    // tools, or the whole `browser_*` family) must collapse to ONE hint
    // instead of one line per tool.
    let registry = Arc::new(crate::tool_metadata::ToolCatalog::new());
    let cache = registry.health();
    let probe: Arc<dyn ToolHealthProbe> = Arc::new(DeadProbe("no browser runtime"));
    for name in ["browser_open", "browser_click", "browser_screenshot"] {
        cache.register_probe(name, probe.clone());
        let _ = cache.refresh(name).await;
    }

    let blocks = compute_runtime_state_blocks(Some(&registry));
    assert_eq!(blocks.len(), 1, "shared-reason tools should coalesce");
    let block = &blocks[0];
    // Lexicographically smallest name leads; the rest are summarised.
    assert!(block.tool_name.starts_with("browser_click"));
    assert!(block.tool_name.contains("+2 more"));
    assert!(
        matches!(&block.status, ToolStatus::Unavailable { .. }),
        "expected Unavailable"
    );
    if let ToolStatus::Unavailable { reason } = &block.status {
        assert_eq!(reason, "no browser runtime");
    }
}
