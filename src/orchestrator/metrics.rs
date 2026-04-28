//! Orchestrator metrics for monitoring flow execution, retry rates, and stall detection.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Metrics collected by the orchestrator during flow execution.
#[derive(Debug, Default)]
pub struct OrchestratorMetrics {
    pub flows_started: AtomicU64,
    pub flows_completed: AtomicU64,
    pub flows_failed: AtomicU64,
    pub flows_cancelled: AtomicU64,
    pub flows_stalled: AtomicU64,
    pub total_retries: AtomicU64,
    pub total_turns: AtomicU64,
    pub active_flows: AtomicU64,
}

impl OrchestratorMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_flow_started(&self) {
        // Relaxed is sufficient: these are independent counters where atomicity
        // is the only requirement, not sequential consistency across threads.
        self.flows_started.fetch_add(1, Ordering::Relaxed);
        self.active_flows.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_flow_completed(&self) {
        self.flows_completed.fetch_add(1, Ordering::Relaxed);
        self.active_flows.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn record_flow_failed(&self) {
        self.flows_failed.fetch_add(1, Ordering::Relaxed);
        self.active_flows.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn record_flow_cancelled(&self) {
        self.flows_cancelled.fetch_add(1, Ordering::Relaxed);
        self.active_flows.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn record_flow_stalled(&self) {
        self.flows_stalled.fetch_add(1, Ordering::Relaxed);
        self.active_flows.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn record_retry(&self) {
        self.total_retries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_turn(&self) {
        self.total_turns.fetch_add(1, Ordering::Relaxed);
    }

    pub fn active_flows(&self) -> u64 {
        self.active_flows.load(Ordering::Relaxed)
    }

    pub fn flows_started(&self) -> u64 {
        self.flows_started.load(Ordering::Relaxed)
    }

    pub fn completion_rate(&self) -> f64 {
        let completed = self.flows_completed.load(Ordering::Relaxed);
        let failed = self.flows_failed.load(Ordering::Relaxed);
        let cancelled = self.flows_cancelled.load(Ordering::Relaxed);
        let stalled = self.flows_stalled.load(Ordering::Relaxed);
        let total = completed + failed + cancelled + stalled;
        if total == 0 {
            0.0
        } else {
            completed as f64 / total as f64
        }
    }

    pub fn stall_rate(&self) -> f64 {
        let completed = self.flows_completed.load(Ordering::Relaxed);
        let failed = self.flows_failed.load(Ordering::Relaxed);
        let cancelled = self.flows_cancelled.load(Ordering::Relaxed);
        let stalled = self.flows_stalled.load(Ordering::Relaxed);
        let total = completed + failed + cancelled + stalled;
        if total == 0 {
            0.0
        } else {
            stalled as f64 / total as f64
        }
    }
}

/// Per-flow metrics tracker.
#[derive(Debug)]
pub struct FlowMetrics {
    pub flow_id: Arc<str>,
    pub started_at: Instant,
    pub turns: AtomicU64,
    pub retries: AtomicU64,
    pub last_activity: AtomicU64,
}

impl FlowMetrics {
    pub fn new(flow_id: Arc<str>) -> Self {
        Self {
            flow_id,
            started_at: Instant::now(),
            turns: AtomicU64::new(0),
            retries: AtomicU64::new(0),
            last_activity: AtomicU64::new(0),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn record_turn(&self) {
        self.turns.fetch_add(1, Ordering::SeqCst);
        self.last_activity.store(
            std::time::UNIX_EPOCH
                .elapsed()
                .unwrap_or_default()
                .as_secs(),
            Ordering::SeqCst,
        );
    }

    pub fn record_retry(&self) {
        self.retries.fetch_add(1, Ordering::SeqCst);
    }

    pub fn turns(&self) -> u64 {
        self.turns.load(Ordering::SeqCst)
    }

    pub fn retries(&self) -> u64 {
        self.retries.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_counters() {
        let metrics = OrchestratorMetrics::new();

        metrics.record_flow_started();
        metrics.record_flow_started();
        assert_eq!(metrics.flows_started(), 2);
        assert_eq!(metrics.active_flows(), 2);

        metrics.record_flow_completed();
        assert_eq!(metrics.active_flows(), 1);
        assert_eq!(metrics.completion_rate(), 0.5);
    }

    #[test]
    fn test_stall_rate() {
        let metrics = OrchestratorMetrics::new();

        for _ in 0..3 {
            metrics.record_flow_started();
        }
        metrics.record_flow_stalled();
        metrics.record_flow_completed();
        metrics.record_flow_failed();

        assert_eq!(metrics.stall_rate(), 1.0 / 3.0);
    }

    #[test]
    fn test_flow_metrics() {
        let metrics = FlowMetrics::new(Arc::from("test-flow"));

        metrics.record_turn();
        metrics.record_turn();
        metrics.record_retry();

        assert_eq!(metrics.turns(), 2);
        assert_eq!(metrics.retries(), 1);
    }
}
