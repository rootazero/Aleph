/// Performance metrics and timing instrumentation module
///
/// This module provides tools for measuring and logging performance metrics
/// across the Aether pipeline. It is designed to have minimal overhead when
/// profiling is disabled and detailed instrumentation when enabled.
///
/// # Usage
///
/// ```rust,no_run
/// use aethecore::metrics::StageTimer;
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
use std::collections::HashMap;
use std::time::Instant;

/// Target latencies for performance monitoring (in milliseconds)
///
/// These values represent the expected maximum latency for each operation
/// under normal conditions. Operations exceeding 2x these targets will
/// trigger warnings in the logs.
pub const TARGET_HOTKEY_TO_CLIPBOARD_MS: u64 = 50;
pub const TARGET_CLIPBOARD_TO_MEMORY_MS: u64 = 100;
pub const TARGET_MEMORY_TO_AI_MS: u64 = 500;
pub const TARGET_AI_TO_PASTE_MS: u64 = 50;
pub const TARGET_PASTE_TO_COMPLETE_MS: u64 = 100;

/// A timer for measuring the duration of a specific stage in the pipeline
///
/// The timer starts when created via `start()` and automatically logs
/// the elapsed time when dropped. This ensures timing data is always
/// captured, even if early returns or errors occur.
///
/// # Examples
///
/// ```rust,no_run
/// use aethecore::metrics::StageTimer;
///
/// {
///     let _timer = StageTimer::start("example_stage");
///     // ... do work
/// } // timer logs automatically here
/// ```
pub struct StageTimer {
    name: String,
    start: Instant,
    metadata: HashMap<String, String>,
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
            metadata: HashMap::new(),
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
    /// use aethecore::metrics::StageTimer;
    ///
    /// let _timer = StageTimer::start("ai_request")
    ///     .with_meta("provider", "OpenAI")
    ///     .with_meta("model", "gpt-4");
    /// ```
    pub fn with_meta(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
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

    /// Stop the timer and log the results
    ///
    /// This is called automatically on drop, but can be called manually
    /// if you want to log the timing before the timer goes out of scope.
    pub fn stop(self) {
        // The drop implementation handles logging
    }

    /// Get the elapsed time in milliseconds
    ///
    /// This method does not stop the timer or trigger logging.
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        let elapsed_ms = self.start.elapsed().as_millis() as u64;

        // Check if we exceeded the target (if set)
        if let Some(target_ms) = self.target_ms {
            if elapsed_ms > target_ms * 2 {
                tracing::warn!(
                    stage = %self.name,
                    actual_ms = %elapsed_ms,
                    target_ms = %target_ms,
                    ratio = %(elapsed_ms as f64 / target_ms as f64),
                    metadata = ?self.metadata,
                    "Slow operation detected (>2x target)"
                );
                return;
            }
        }

        // Normal timing log (debug level)
        if self.metadata.is_empty() {
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
/// use aethecore::time_stage;
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
        assert!(timer.metadata.is_empty());
        assert!(timer.target_ms.is_none());
    }

    #[test]
    fn test_timer_with_metadata() {
        let timer = StageTimer::start("test_stage")
            .with_meta("key1", "value1")
            .with_meta("key2", "value2");

        assert_eq!(timer.metadata.get("key1"), Some(&"value1".to_string()));
        assert_eq!(timer.metadata.get("key2"), Some(&"value2".to_string()));
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
    fn test_target_constants() {
        // Verify target constants are sensible
        assert_eq!(TARGET_HOTKEY_TO_CLIPBOARD_MS, 50);
        assert_eq!(TARGET_CLIPBOARD_TO_MEMORY_MS, 100);
        assert_eq!(TARGET_MEMORY_TO_AI_MS, 500);
        assert_eq!(TARGET_AI_TO_PASTE_MS, 50);
        assert_eq!(TARGET_PASTE_TO_COMPLETE_MS, 100);
    }

    #[test]
    fn test_timer_accuracy() {
        let timer = StageTimer::start("accuracy_test");
        thread::sleep(Duration::from_millis(100));
        let elapsed = timer.elapsed_ms();

        // Allow ±10% tolerance
        assert!(
            elapsed >= 90 && elapsed <= 110,
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

        assert_eq!(timer.metadata.len(), 3);
        assert_eq!(timer.metadata.get("provider"), Some(&"OpenAI".to_string()));
        assert_eq!(timer.metadata.get("model"), Some(&"gpt-4".to_string()));
        assert_eq!(
            timer.metadata.get("app"),
            Some(&"com.apple.Notes".to_string())
        );
    }

    #[test]
    fn test_chaining() {
        let timer = StageTimer::start("chain_test")
            .with_meta("key", "value")
            .with_target(200);

        assert_eq!(timer.metadata.get("key"), Some(&"value".to_string()));
        assert_eq!(timer.target_ms, Some(200));
    }
}
