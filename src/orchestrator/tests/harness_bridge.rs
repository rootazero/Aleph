//! Bridge-level smoke tests. Full end-to-end coverage lives in
//! tests/orchestrator_e2e.rs (Task 13).

use crate::orchestrator::harness_bridge::AgentHarnessRunner;

#[test]
fn agent_harness_runner_is_send_sync() {
    fn _requires_send_sync<T: Send + Sync>() {}
    _requires_send_sync::<AgentHarnessRunner>();
}
