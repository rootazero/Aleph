//! Cortex telemetry for experience capture
//!
//! Captures task execution context at agent loop exit points
//! for experience replay and skill distillation.

use crate::error::AlephError;
use crate::memory::cortex::EnvironmentContext;
use crate::memory::database::VectorDatabase;
use std::sync::Arc;
use std::time::Instant;

/// Telemetry data captured during agent loop execution
#[derive(Debug, Clone)]
pub struct ExecutionTelemetry {
    /// User's original intent/request
    pub user_intent: String,

    /// Tool sequence executed (JSON)
    pub tool_sequence_json: String,

    /// Execution start time
    pub start_time: Instant,

    /// Execution end time
    pub end_time: Instant,

    /// Total tokens consumed
    pub total_tokens: u64,

    /// Number of steps/iterations
    pub step_count: u32,

    /// Whether execution succeeded
    pub success: bool,

    /// Error recovery occurred (tool A failed → tool B succeeded)
    pub had_error_recovery: bool,

    /// Environment context
    pub environment: EnvironmentContext,
}

impl ExecutionTelemetry {
    /// Calculate execution latency in milliseconds
    pub fn latency_ms(&self) -> i64 {
        self.end_time.duration_since(self.start_time).as_millis() as i64
    }

    /// Check if this execution meets realtime distillation criteria
    pub fn should_distill_realtime(&self) -> bool {
        // Realtime distillation criteria (from design doc):
        // 1. Execution time > 30 seconds
        // 2. Tool chain length > 5 steps
        // 3. Had error recovery (tool A failed → tool B succeeded)
        // 4. User explicit feedback (handled separately)

        let latency_sec = self.latency_ms() / 1000;

        latency_sec > 30 || self.step_count > 5 || self.had_error_recovery
    }
}

/// Cortex telemetry collector
pub struct CortexTelemetry {
    db: Arc<VectorDatabase>,
    enabled: bool,
}

impl CortexTelemetry {
    /// Create a new telemetry collector
    pub fn new(db: Arc<VectorDatabase>) -> Self {
        Self { db, enabled: true }
    }

    /// Create a disabled telemetry collector (no-op)
    pub fn disabled() -> Self {
        Self {
            db: Arc::new(VectorDatabase::new(std::path::PathBuf::from(":memory:")).unwrap()),
            enabled: false,
        }
    }

    /// Capture execution telemetry at loop exit
    pub async fn capture(&self, telemetry: ExecutionTelemetry) -> Result<(), AlephError> {
        if !self.enabled {
            return Ok(());
        }

        tracing::debug!(
            user_intent = %telemetry.user_intent,
            latency_ms = telemetry.latency_ms(),
            step_count = telemetry.step_count,
            success = telemetry.success,
            had_error_recovery = telemetry.had_error_recovery,
            "Cortex telemetry captured"
        );

        // Check if should trigger realtime distillation
        if telemetry.should_distill_realtime() {
            tracing::info!(
                user_intent = %telemetry.user_intent,
                latency_ms = telemetry.latency_ms(),
                step_count = telemetry.step_count,
                "Realtime distillation triggered"
            );

            // TODO: Trigger distillation service
            // For now, just log the event
            // In Month 2, we'll implement the actual distillation pipeline
        }

        // Store telemetry for batch processing
        // TODO: Store in a telemetry buffer for Dreaming process
        // For now, just log
        tracing::debug!(
            "Telemetry stored for batch processing (Month 2 implementation)"
        );

        Ok(())
    }
}

/// Optional telemetry wrapper (similar to OptionalCompactionTrigger)
pub struct OptionalCortexTelemetry {
    telemetry: Option<Arc<CortexTelemetry>>,
}

impl OptionalCortexTelemetry {
    /// Create with telemetry enabled
    pub fn new(telemetry: Option<Arc<CortexTelemetry>>) -> Self {
        Self { telemetry }
    }

    /// Capture telemetry if enabled
    pub async fn capture(&self, telemetry: ExecutionTelemetry) {
        if let Some(ref t) = self.telemetry {
            if let Err(e) = t.capture(telemetry).await {
                tracing::warn!("Failed to capture telemetry: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_distill_realtime_latency() {
        let telemetry = ExecutionTelemetry {
            user_intent: "test".to_string(),
            tool_sequence_json: "{}".to_string(),
            start_time: Instant::now() - std::time::Duration::from_secs(35),
            end_time: Instant::now(),
            total_tokens: 1000,
            step_count: 3,
            success: true,
            had_error_recovery: false,
            environment: EnvironmentContext {
                working_directory: "/test".to_string(),
                platform: "macos".to_string(),
                permissions: vec![],
                metadata: std::collections::HashMap::new(),
            },
        };

        assert!(telemetry.should_distill_realtime());
    }

    #[test]
    fn test_should_distill_realtime_step_count() {
        let telemetry = ExecutionTelemetry {
            user_intent: "test".to_string(),
            tool_sequence_json: "{}".to_string(),
            start_time: Instant::now() - std::time::Duration::from_secs(10),
            end_time: Instant::now(),
            total_tokens: 1000,
            step_count: 6,
            success: true,
            had_error_recovery: false,
            environment: EnvironmentContext {
                working_directory: "/test".to_string(),
                platform: "macos".to_string(),
                permissions: vec![],
                metadata: std::collections::HashMap::new(),
            },
        };

        assert!(telemetry.should_distill_realtime());
    }

    #[test]
    fn test_should_distill_realtime_error_recovery() {
        let telemetry = ExecutionTelemetry {
            user_intent: "test".to_string(),
            tool_sequence_json: "{}".to_string(),
            start_time: Instant::now() - std::time::Duration::from_secs(5),
            end_time: Instant::now(),
            total_tokens: 1000,
            step_count: 2,
            success: true,
            had_error_recovery: true,
            environment: EnvironmentContext {
                working_directory: "/test".to_string(),
                platform: "macos".to_string(),
                permissions: vec![],
                metadata: std::collections::HashMap::new(),
            },
        };

        assert!(telemetry.should_distill_realtime());
    }

    #[test]
    fn test_should_not_distill_realtime() {
        let telemetry = ExecutionTelemetry {
            user_intent: "test".to_string(),
            tool_sequence_json: "{}".to_string(),
            start_time: Instant::now() - std::time::Duration::from_secs(10),
            end_time: Instant::now(),
            total_tokens: 1000,
            step_count: 3,
            success: true,
            had_error_recovery: false,
            environment: EnvironmentContext {
                working_directory: "/test".to_string(),
                platform: "macos".to_string(),
                permissions: vec![],
                metadata: std::collections::HashMap::new(),
            },
        };

        assert!(!telemetry.should_distill_realtime());
    }
}
