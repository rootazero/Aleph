/// Performance metrics and timing instrumentation module
///
/// This module provides tools for measuring and logging performance metrics
/// across the Aleph pipeline. It is designed to have minimal overhead when
/// profiling is disabled and detailed instrumentation when enabled.
///
/// # Usage
///
/// ```rust,no_run
/// use alephcore::metrics::StageTimer;
///
/// // Simple timing
/// let _timer = StageTimer::start("clipboard_read");
/// // ... do work
/// // timer automatically logs on drop
///
/// // With metadata
/// let _timer = StageTimer::start("ai_request")
///     .with_meta("provider", "OpenAI")
///     .with_meta("model", "gpt-4");
/// // ... do work
/// ```
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use std::collections::BTreeMap;
use std::time::Instant;

/// Default warning multiplier applied when no policy is configured.
const DEFAULT_WARNING_MULTIPLIER: f64 = 2.0;

/// Live metrics knobs sourced from `[policies.metrics]` at config load.
///
/// `StageTimer` is created via ad-hoc static `start()` calls with no config in
/// scope, so the policy is bound process-wide once (write-once `OnceLock`,
/// mirroring `config::defaults_override`). Reads before init — early startup,
/// unit tests — fall back to the compiled defaults.
#[derive(Clone, Copy)]
struct MetricsRuntime {
    warning_multiplier: f64,
    enable_logging: bool,
    enable_warnings: bool,
}

impl Default for MetricsRuntime {
    fn default() -> Self {
        Self {
            warning_multiplier: DEFAULT_WARNING_MULTIPLIER,
            enable_logging: true,
            enable_warnings: true,
        }
    }
}

/// `IndistinguishableDefault`, derived from the single reader below:
/// [`metrics_runtime`] answers `.get().copied().unwrap_or_default()`, so an
/// uninstalled handle hands every `StageTimer` the compiled
/// `MetricsRuntime::default()` — warnings on, logging on, 2.0x threshold —
/// and an operator who set `[policies.metrics] enable_warnings = false` keeps
/// getting warnings with nothing anywhere saying why.
///
/// ⚠️ This handle is the reason the census is derived rather than grepped: it
/// was absent from the specification's hand-written roster because rustfmt put
/// its `.set(` on the line after the receiver. See
/// `capability::census::tests::the_writer_recogniser_reads_across_line_breaks`.
static METRICS_RUNTIME: CapabilitySlot<MetricsRuntime> = CapabilitySlot::new(
    "metrics/runtime",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "MetricsRuntime::default() -- warnings and stage logging on, \
                   2.0x warning threshold, whatever [policies.metrics] said",
    },
);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn metrics_runtime_slot() -> &'static dyn SlotStatus {
    &METRICS_RUNTIME
}

/// Bind the live metrics knobs from `[policies.metrics]`. Called once from
/// `Config::load` so `StageTimer` honours user-configured thresholds instead of
/// the compiled defaults. Idempotent: a later call (e.g. a config reload) is
/// ignored, matching the write-once semantics of `defaults_override`.
pub fn init_metrics_runtime(policy: &crate::config::MetricsPolicy) {
    let warning_multiplier =
        if policy.warning_multiplier.is_finite() && policy.warning_multiplier >= 0.0 {
            policy.warning_multiplier
        } else {
            DEFAULT_WARNING_MULTIPLIER
        };
    if !METRICS_RUNTIME.install(MetricsRuntime {
        warning_multiplier,
        enable_logging: policy.enable_logging,
        enable_warnings: policy.enable_warnings,
    }) {
        tracing::warn!(
            "metrics runtime already initialised; ignoring reload — the reloaded \
             [policies.metrics] values are silently inactive, restart the process to \
             pick them up"
        );
    }
}

fn metrics_runtime() -> MetricsRuntime {
    METRICS_RUNTIME.get().copied().unwrap_or_default()
}

impl MetricsRuntime {
    #[must_use]
    pub fn warning_threshold_ms(&self, target_ms: u64) -> u64 {
        (target_ms as f64 * self.warning_multiplier) as u64
    }
}

/// A timer for measuring the duration of a specific stage in the pipeline
///
/// The timer starts when created via `start()` and automatically logs
/// the elapsed time when dropped. This ensures timing data is always
/// captured, even if early returns or errors occur.
///
/// # Examples
///
/// ```rust,no_run
/// use alephcore::metrics::StageTimer;
///
/// {
///     let _timer = StageTimer::start("example_stage");
///     // ... do work
/// } // timer logs automatically here
/// ```
#[must_use]
pub struct StageTimer {
    name: String,
    start: Instant,
    metadata: Option<BTreeMap<String, String>>,
    target_ms: Option<u64>,
}

impl StageTimer {
    /// Start timing a new stage
    ///
    /// The timer begins immediately upon creation.
    ///
    /// # Arguments
    ///
    /// * `name` - Human-readable name for this stage
    ///
    /// # Returns
    ///
    /// A new `StageTimer` that will log on drop
    pub fn start(name: &str) -> Self {
        Self {
            name: name.to_string(),
            start: Instant::now(),
            metadata: None,
            target_ms: None,
        }
    }

    /// Add metadata to be included in the log output
    ///
    /// Metadata is useful for providing context about what happened
    /// during the timed stage (e.g., provider name, model, app).
    ///
    /// # Arguments
    ///
    /// * `key` - Metadata key
    /// * `value` - Metadata value
    ///
    /// # Returns
    ///
    /// Self for chaining
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use alephcore::metrics::StageTimer;
    ///
    /// let _timer = StageTimer::start("ai_request")
    ///     .with_meta("provider", "OpenAI")
    ///     .with_meta("model", "gpt-4");
    /// ```
    pub fn with_meta(mut self, key: &str, value: &str) -> Self {
        self.metadata
            .get_or_insert_with(BTreeMap::new)
            .insert(key.to_string(), value.to_string());
        self
    }

    /// Set a target latency for this stage
    ///
    /// If the stage takes longer than `target_ms * warning_multiplier`, a warning will be logged.
    /// Setting `target_ms` to `0` disables the warning for this timer.
    /// This is useful for detecting performance regressions.
    ///
    /// # Arguments
    ///
    /// * `target_ms` - Target latency in milliseconds
    ///
    /// # Returns
    ///
    /// Self for chaining
    pub const fn with_target(mut self, target_ms: u64) -> Self {
        self.target_ms = Some(target_ms);
        self
    }

    /// Get the elapsed time in whole milliseconds.
    ///
    /// Sub-millisecond precision is truncated (e.g., a 0.5 ms duration
    /// returns `0`). Use this for coarse-grained reporting only.
    ///
    /// This method does not stop the timer or trigger logging.
    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        self.start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        let elapsed_ms = self.elapsed_ms();
        // Live knobs from `[policies.metrics]` (compiled defaults before init).
        let rt = metrics_runtime();

        // Check if we exceeded the target (if set) and warnings are enabled
        if let Some(target_ms) = self.target_ms {
            if target_ms > 0 {
                let threshold_ms = rt.warning_threshold_ms(target_ms);
                if elapsed_ms > threshold_ms && rt.enable_warnings {
                    tracing::warn!(
                        stage = %self.name,
                        actual_ms = %elapsed_ms,
                        target_ms = %target_ms,
                        threshold_ms = %threshold_ms,
                        ratio = %(elapsed_ms as f64 / target_ms as f64),
                        metadata = ?self.metadata,
                        "Slow operation detected (exceeds threshold)"
                    );
                    return;
                }
            }
        }

        // Normal timing log (debug level) if logging is enabled
        if !rt.enable_logging {
            return;
        }

        let meta = self.metadata.as_ref().filter(|m| !m.is_empty());
        tracing::debug!(
            stage = %self.name,
            duration_ms = %elapsed_ms,
            metadata = ?meta,
            "Stage completed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_timer_creation() {
        let timer = StageTimer::start("test_stage");
        assert_eq!(timer.name, "test_stage");
        assert!(timer.metadata.is_none());
        assert!(timer.target_ms.is_none());
    }

    #[test]
    fn test_timer_with_metadata() {
        let timer = StageTimer::start("test_stage")
            .with_meta("key1", "value1")
            .with_meta("key2", "value2");

        assert_eq!(
            timer.metadata.as_ref().and_then(|m| m.get("key1")),
            Some(&"value1".to_string())
        );
        assert_eq!(
            timer.metadata.as_ref().and_then(|m| m.get("key2")),
            Some(&"value2".to_string())
        );
    }

    #[test]
    fn test_timer_with_target() {
        let timer = StageTimer::start("test_stage").with_target(100);
        assert_eq!(timer.target_ms, Some(100));
    }

    #[test]
    fn test_timer_elapsed() {
        let timer = StageTimer::start("test_stage");
        thread::sleep(Duration::from_millis(10));
        let elapsed = timer.elapsed_ms();
        assert!(elapsed >= 10, "Elapsed time should be at least 10ms");
        assert!(elapsed < 5000, "Elapsed time should be less than 5s");
    }

    #[test]
    fn test_timer_drop_logs() {
        // This test just ensures the drop doesn't panic
        {
            let _timer = StageTimer::start("test_stage").with_meta("test", "value");
        } // Timer drops here
    }

    #[test]
    fn test_timer_accuracy() {
        let timer = StageTimer::start("accuracy_test");
        thread::sleep(Duration::from_millis(100));
        let elapsed = timer.elapsed_ms();

        // The lower bound proves the timer actually accumulates the slept time;
        // the upper bound only guards against a unit error (e.g. reporting ns/us
        // as ms). It must stay generous: a loaded CI runner can let a 100ms
        // sleep overshoot well past any tight ceiling, which made this test
        // flaky at ±50ms. 5s is comfortably above any scheduler jitter.
        assert!(
            (50..=5_000).contains(&elapsed),
            "Timer accuracy: {}ms",
            elapsed
        );
    }

    #[test]
    fn test_multiple_metadata() {
        let timer = StageTimer::start("multi_meta")
            .with_meta("provider", "OpenAI")
            .with_meta("model", "gpt-4")
            .with_meta("app", "com.apple.Notes");

        assert_eq!(timer.metadata.as_ref().map_or(0, |m| m.len()), 3);
        assert_eq!(
            timer.metadata.as_ref().and_then(|m| m.get("provider")),
            Some(&"OpenAI".to_string())
        );
        assert_eq!(
            timer.metadata.as_ref().and_then(|m| m.get("model")),
            Some(&"gpt-4".to_string())
        );
        assert_eq!(
            timer.metadata.as_ref().and_then(|m| m.get("app")),
            Some(&"com.apple.Notes".to_string())
        );
    }

    #[test]
    fn test_chaining() {
        let timer = StageTimer::start("chain_test")
            .with_meta("key", "value")
            .with_target(200);

        assert_eq!(
            timer.metadata.as_ref().and_then(|m| m.get("key")),
            Some(&"value".to_string())
        );
        assert_eq!(timer.target_ms, Some(200));
    }

    #[test]
    fn test_timer_target_zero_no_warning() {
        let timer = StageTimer::start("zero_target").with_target(0);
        assert_eq!(timer.target_ms, Some(0));
    }

    #[test]
    fn test_timer_with_empty_metadata() {
        let timer = StageTimer::start("empty_meta_test");
        assert!(timer.metadata.is_none());
    }

    /// The `reads_as` sentence reaches an operator, so it is tied to the
    /// fallback `metrics_runtime()` really returns rather than spot-read.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = metrics_runtime_slot();
        assert_eq!(slot.id(), "metrics/runtime");
        let MissingSemantics::IndistinguishableDefault { reads_as } = slot.missing() else {
            panic!(
                "expected IndistinguishableDefault, got {:?}",
                slot.missing()
            );
        };
        // Tied to the CONSTANT, not to the literal "2.0x": change
        // DEFAULT_WARNING_MULTIPLIER and this names the stale sentence
        // instead of agreeing with it.
        assert!(
            reads_as.contains(&format!("{DEFAULT_WARNING_MULTIPLIER:.1}x")),
            "the sentence must name the real multiplier \
             ({DEFAULT_WARNING_MULTIPLIER}); got {reads_as:?}"
        );
        let d = MetricsRuntime::default();
        assert_eq!(d.warning_multiplier, DEFAULT_WARNING_MULTIPLIER);
        assert!(
            d.enable_logging && d.enable_warnings,
            "the sentence claims logging and warnings are ON when nothing is \
             installed; MetricsRuntime::default() no longer agrees"
        );
    }
}
