//! Bridge-level smoke tests. Full end-to-end coverage lives in
//! tests/orchestrator_e2e.rs (Task 13).

use std::sync::Arc;

use crate::orchestrator::harness_bridge::{compute_runtime_state_blocks, AgentHarnessRunner};
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
        match &block.status {
            ToolStatus::Unavailable { reason } => {
                assert!(reason.contains("offline"));
            }
            ToolStatus::Available => panic!("expected Unavailable"),
        }
    }
}
