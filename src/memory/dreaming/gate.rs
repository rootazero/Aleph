//! DreamGate — 3-level cheap-to-expensive gate chain for DreamDaemon consolidation.
//!
//! Gates run in order:
//!   1. Time gate   (cheapest — atomic read)
//!   2. Count gate  (cheap   — caller supplies the count)
//!   3. Drift gate  (most expensive — caller supplies the pre-computed avg distance)
//!
//! The caller is responsible for computing `avg_drift` (e.g. by sampling embeddings)
//! only after the cheaper gates have passed.

use crate::sync_primitives::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the DreamGate chain.
#[derive(Debug, Clone)]
pub struct DreamGateConfig {
    /// Minimum hours that must have elapsed since the last successful
    /// consolidation before the gate will pass.  Default: 6.0
    pub min_hours: f64,

    /// Minimum number of pending facts required for consolidation to proceed.
    /// Default: 20
    pub min_pending_facts: usize,

    /// Minimum average cosine distance across pending facts required to proceed.
    /// Values below this indicate the memory landscape hasn't drifted enough.
    /// Default: 0.3
    pub drift_threshold: f32,

    /// Suggested polling interval for the background scheduler.  Default: 4 h
    pub background_interval: Duration,
}

impl Default for DreamGateConfig {
    fn default() -> Self {
        Self {
            min_hours: 6.0,
            min_pending_facts: 20,
            drift_threshold: 0.3,
            background_interval: Duration::from_secs(4 * 3600),
        }
    }
}

// ---------------------------------------------------------------------------
// GateResult / BlockReason
// ---------------------------------------------------------------------------

/// Outcome of a gate evaluation.
#[derive(Debug)]
pub enum GateResult {
    /// All gates passed — consolidation may proceed.
    Pass,
    /// A gate blocked consolidation; the reason is attached.
    Blocked(BlockReason),
}

impl GateResult {
    /// Returns `true` when the result is `Pass`.
    pub fn is_pass(&self) -> bool {
        matches!(self, GateResult::Pass)
    }
}

/// Reason a gate blocked consolidation.
#[derive(Debug)]
pub enum BlockReason {
    /// Not enough time has elapsed since the last run.
    TooRecent {
        /// Hours elapsed since the last successful consolidation.
        hours_since: f64,
    },
    /// Too few facts are pending — not worth running the pipeline.
    InsufficientFacts {
        /// Current pending fact count.
        count: usize,
    },
    /// The average embedding drift is below the threshold — memories haven't
    /// changed enough to warrant consolidation.
    LowDrift {
        /// Computed average cosine distance.
        avg_distance: f32,
    },
    /// A consolidation run is already in progress.
    AlreadyRunning,
}

// ---------------------------------------------------------------------------
// DreamGate
// ---------------------------------------------------------------------------

/// Thread-safe gate chain that decides whether to trigger the DreamDaemon
/// consolidation pipeline.
pub struct DreamGate {
    config: DreamGateConfig,
    /// Unix timestamp (seconds) of the last *successful* consolidation.
    /// Initialised to 0 so that the very first run always passes the time gate
    /// (unless `min_hours` is also 0).
    last_consolidation: AtomicI64,
    /// `true` while a consolidation run is in progress.
    is_running: AtomicBool,
}

impl DreamGate {
    /// Create a new gate with the given configuration.
    pub fn new(config: DreamGateConfig) -> Self {
        Self {
            config,
            last_consolidation: AtomicI64::new(0),
            is_running: AtomicBool::new(false),
        }
    }

    // ------------------------------------------------------------------
    // Individual gate checks
    // ------------------------------------------------------------------

    /// Gate 1 (cheapest): Ensure enough time has passed since the last run.
    pub fn check_time_gate(&self) -> GateResult {
        let last = self.last_consolidation.load(Ordering::Relaxed);
        let now = chrono::Utc::now().timestamp();
        let elapsed_secs = (now - last).max(0) as f64;
        let hours_since = elapsed_secs / 3600.0;

        if hours_since < self.config.min_hours {
            GateResult::Blocked(BlockReason::TooRecent { hours_since })
        } else {
            GateResult::Pass
        }
    }

    /// Gate 2: Ensure enough pending facts exist to justify a pipeline run.
    pub fn check_count_gate(&self, pending_facts: usize) -> GateResult {
        if pending_facts < self.config.min_pending_facts {
            GateResult::Blocked(BlockReason::InsufficientFacts {
                count: pending_facts,
            })
        } else {
            GateResult::Pass
        }
    }

    /// Gate 3 (most expensive): Ensure the average embedding drift is high
    /// enough to indicate meaningful memory change.
    pub fn check_drift_gate(&self, avg_distance: f32) -> GateResult {
        if avg_distance < self.config.drift_threshold {
            GateResult::Blocked(BlockReason::LowDrift { avg_distance })
        } else {
            GateResult::Pass
        }
    }

    // ------------------------------------------------------------------
    // Full evaluation
    // ------------------------------------------------------------------

    /// Evaluate the full 3-level gate chain in cheap→expensive order.
    ///
    /// Also checks whether a run is already in progress.  If all gates pass,
    /// `is_running` is atomically set to `true` before returning `Pass`.
    ///
    /// # Arguments
    ///
    /// * `pending_facts` — number of facts awaiting consolidation.
    /// * `avg_drift` — pre-computed average cosine distance across pending facts.
    pub fn evaluate(&self, pending_facts: usize, avg_drift: f32) -> GateResult {
        // Running check (effectively free).
        if self.is_running.load(Ordering::SeqCst) {
            return GateResult::Blocked(BlockReason::AlreadyRunning);
        }

        // Gate 1 — time
        if let GateResult::Blocked(reason) = self.check_time_gate() {
            return GateResult::Blocked(reason);
        }

        // Gate 2 — count
        if let GateResult::Blocked(reason) = self.check_count_gate(pending_facts) {
            return GateResult::Blocked(reason);
        }

        // Gate 3 — drift
        if let GateResult::Blocked(reason) = self.check_drift_gate(avg_drift) {
            return GateResult::Blocked(reason);
        }

        // All gates passed — claim the lock.
        match self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => GateResult::Pass,
            Err(_) => GateResult::Blocked(BlockReason::AlreadyRunning),
        }
    }

    // ------------------------------------------------------------------
    // Lifecycle helpers
    // ------------------------------------------------------------------

    /// Called when a consolidation run completes successfully.
    /// Records the completion timestamp and releases the running lock.
    pub fn mark_complete(&self) {
        self.record_consolidation();
        self.is_running.store(false, Ordering::SeqCst);
    }

    /// Called when a consolidation run fails.
    /// Releases the running lock but does *not* update the timestamp so that
    /// the next check can retry sooner.
    pub fn mark_failed(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }

    /// Record the current time as the last successful consolidation timestamp.
    pub fn record_consolidation(&self) {
        let now = chrono::Utc::now().timestamp();
        self.last_consolidation.store(now, Ordering::Relaxed);
    }

    /// Return a reference to the gate configuration.
    pub fn config(&self) -> &DreamGateConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn gate_with(min_hours: f64, min_facts: usize, drift_threshold: f32) -> DreamGate {
        DreamGate::new(DreamGateConfig {
            min_hours,
            min_pending_facts: min_facts,
            drift_threshold,
            background_interval: Duration::from_secs(3600),
        })
    }

    #[test]
    fn gate_blocks_when_too_recent() {
        let gate = gate_with(6.0, 20, 0.3);
        // Record consolidation right now — only 0 seconds have elapsed.
        gate.record_consolidation();
        let result = gate.check_time_gate();
        assert!(
            matches!(result, GateResult::Blocked(BlockReason::TooRecent { .. })),
            "expected TooRecent, got {:?}",
            result
        );
    }

    #[test]
    fn gate_passes_time_when_old_enough() {
        // With min_hours = 0.0, any elapsed time (including 0) satisfies the gate.
        let gate = gate_with(0.0, 20, 0.3);
        gate.record_consolidation();
        let result = gate.check_time_gate();
        assert!(
            matches!(result, GateResult::Pass),
            "expected Pass, got {:?}",
            result
        );
    }

    #[test]
    fn gate_blocks_insufficient_facts() {
        let gate = gate_with(0.0, 20, 0.3);
        let result = gate.check_count_gate(5);
        assert!(
            matches!(
                result,
                GateResult::Blocked(BlockReason::InsufficientFacts { count: 5 })
            ),
            "expected InsufficientFacts(5), got {:?}",
            result
        );
    }

    #[test]
    fn gate_passes_sufficient_facts() {
        let gate = gate_with(0.0, 20, 0.3);
        let result = gate.check_count_gate(25);
        assert!(
            matches!(result, GateResult::Pass),
            "expected Pass, got {:?}",
            result
        );
    }

    #[test]
    fn gate_blocks_low_drift() {
        let gate = gate_with(0.0, 20, 0.3);
        let result = gate.check_drift_gate(0.1);
        assert!(
            matches!(result, GateResult::Blocked(BlockReason::LowDrift { .. })),
            "expected LowDrift, got {:?}",
            result
        );
    }

    #[test]
    fn gate_passes_high_drift() {
        let gate = gate_with(0.0, 20, 0.3);
        let result = gate.check_drift_gate(0.5);
        assert!(
            matches!(result, GateResult::Pass),
            "expected Pass, got {:?}",
            result
        );
    }
}
