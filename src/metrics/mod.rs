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
use crate::config::MetricsPolicy;
use std::collections::HashMap;
use std::time::Instant;

/// Default warning multiplier applied when no policy is configured.
const DEFAULT_WARNING_MULTIPLIER: f64 = 2.0;

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
pub struct StageTimer {
    name: String,
    start: Instant,
    metadata: Option<HashMap<String, String>>,
    target_ms: Option<u64>,
    /// Warning multiplier (default: 2.0, can be set from policy)
    warning_multiplier: f64,
    /// Whether logging is enabled (from policy)
    enable_logging: bool,
    /// Whether warnings are enabled (from policy)
    enable_warnings: bool,
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
            warning_multiplier: DEFAULT_WARNING_MULTIPLIER,
            enable_logging: true,
            enable_warnings: true,
        }
    }

    /// Create a StageTimer with policy configuration
    ///
    /// Uses the policy's warning multiplier and logging settings.
    pub fn start_with_policy(name: &str, policy: &MetricsPolicy) -> Self {
        Self {
            name: name.to_string(),
            start: Instant::now(),
            metadata: None,
            target_ms: None,
            warning_multiplier: policy.warning_multiplier,
            enable_logging: policy.enable_logging,
            enable_warnings: policy.enable_warnings,
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
            .get_or_insert_with(HashMap::new)
            .insert(key.to_string(), value.to_string());
        self
    }

    /// Set a target latency for this stage
    ///
    /// If the stage takes longer than 2x the target, a warning will be logged.
    /// This is useful for detecting performance regressions.
    ///
    /// # Arguments
    ///
    /// * `target_ms` - Target latency in milliseconds
    ///
    /// # Returns
    ///
    /// Self for chaining
    pub fn with_target(mut self, target_ms: u64) -> Self {
        self.target_ms = Some(target_ms);
        self
    }

    /// Stop the timer, return the elapsed time, and suppress Drop logging.
    ///
    /// Unlike letting the timer go out of scope, this prevents the
    /// automatic debug/warn log from firing when the value is dropped.
    /// Useful when the caller wants to handle timing data manually.
    pub fn stop(mut self) -> u64 {
        self.enable_logging = false;
        self.enable_warnings = false;
        self.elapsed_ms()
    }

    /// Get the elapsed time in whole milliseconds.
    ///
    /// Sub-millisecond precision is truncated (e.g., a 0.5 ms duration
    /// returns `0`). Use this for coarse-grained reporting only.
    ///
    /// This method does not stop the timer or trigger logging.
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        let elapsed_ms = self.start.elapsed().as_millis() as u64;

        // Check if we exceeded the target (if set) and warnings are enabled
        if let Some(target_ms) = self.target_ms {
            if target_ms > 0 {
                let threshold_ms = (target_ms as f64 * self.warning_multiplier) as u64;
                if elapsed_ms > threshold_ms && self.enable_warnings {
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
        if !self.enable_logging {
            return;
        }

        if self.metadata.as_ref().is_none_or(|m| m.is_empty()) {
            tracing::debug!(
                stage = %self.name,
                duration_ms = %elapsed_ms,
                "Stage completed"
            );
        } else {
            tracing::debug!(
                stage = %self.name,
                duration_ms = %elapsed_ms,
                metadata = ?self.metadata,
                "Stage completed"
            );
        }
    }
}

/// Macro for convenient timing with automatic target setting
///
/// This macro creates a StageTimer with a predefined target based on
/// the stage name. It's a convenience wrapper around StageTimer::start().
///
/// # Examples
///
/// ```rust,ignore
/// use alephcore::time_stage;
///
/// {
///     let _timer = time_stage!("clipboard_read");
///     // ... do work
/// }
/// ```
#[macro_export]
macro_rules! time_stage {
    ($name:expr) => {{
        use $crate::metrics::StageTimer;
        StageTimer::start($name)
    }};

    ($name:expr, target: $target:expr) => {{
        use $crate::metrics::StageTimer;
        StageTimer::start($name).with_target($target)
    }};
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
        assert_eq!(timer.warning_multiplier, DEFAULT_WARNING_MULTIPLIER);
        assert!(timer.enable_logging);
        assert!(timer.enable_warnings);
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
        assert!(elapsed < 50, "Elapsed time should be less than 50ms");
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

        // Allow ±10% tolerance
        assert!(
            (90..=110).contains(&elapsed),
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
    fn test_timer_start_with_policy() {
        let mut policy = MetricsPolicy::default();
        policy.warning_multiplier = 3.0;
        policy.enable_logging = false;
        policy.enable_warnings = false;
        let timer = StageTimer::start_with_policy("policy_test", &policy);
        assert_eq!(timer.name, "policy_test");
        assert_eq!(timer.warning_multiplier, 3.0);
        assert!(!timer.enable_logging);
        assert!(!timer.enable_warnings);
    }

    #[test]
    fn test_timer_stop() {
        let timer = StageTimer::start("stop_test").with_meta("key", "value");
        let elapsed = timer.stop();
        assert!(elapsed < 10, "stop() should return elapsed time");
    }
}
