//! Model Health Monitoring FFI Types
//!
//! Contains Health Monitoring FFI types:
//! - ModelHealthStatusFFI: Health status enum
//! - ModelHealthSummaryFFI: Per-model health summary
//! - HealthStatisticsFFI: Overall health statistics

use crate::dispatcher::model_router::{HealthStatistics, HealthStatus, ModelHealthSummary};

// ============================================================================
// Health Status Enum
// ============================================================================

/// Health status of an AI model for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelHealthStatusFFI {
    Healthy,
    Degraded,
    Unhealthy,
    CircuitOpen,
    HalfOpen,
    Unknown,
}

impl From<HealthStatus> for ModelHealthStatusFFI {
    fn from(status: HealthStatus) -> Self {
        match status {
            HealthStatus::Healthy => ModelHealthStatusFFI::Healthy,
            HealthStatus::Degraded => ModelHealthStatusFFI::Degraded,
            HealthStatus::Unhealthy => ModelHealthStatusFFI::Unhealthy,
            HealthStatus::CircuitOpen => ModelHealthStatusFFI::CircuitOpen,
            HealthStatus::HalfOpen => ModelHealthStatusFFI::HalfOpen,
            HealthStatus::Unknown => ModelHealthStatusFFI::Unknown,
        }
    }
}

impl From<ModelHealthStatusFFI> for HealthStatus {
    fn from(status: ModelHealthStatusFFI) -> Self {
        match status {
            ModelHealthStatusFFI::Healthy => HealthStatus::Healthy,
            ModelHealthStatusFFI::Degraded => HealthStatus::Degraded,
            ModelHealthStatusFFI::Unhealthy => HealthStatus::Unhealthy,
            ModelHealthStatusFFI::CircuitOpen => HealthStatus::CircuitOpen,
            ModelHealthStatusFFI::HalfOpen => HealthStatus::HalfOpen,
            ModelHealthStatusFFI::Unknown => HealthStatus::Unknown,
        }
    }
}

// ============================================================================
// Health Summary Struct
// ============================================================================

/// Summarized health information for a single model (UI display)
#[derive(Debug, Clone)]
pub struct ModelHealthSummaryFFI {
    pub model_id: String,
    pub status: ModelHealthStatusFFI,
    pub status_text: String,
    pub status_emoji: String,
    pub reason: Option<String>,
    pub consecutive_successes: u32,
    pub consecutive_failures: u32,
}

impl From<ModelHealthSummary> for ModelHealthSummaryFFI {
    fn from(summary: ModelHealthSummary) -> Self {
        Self {
            model_id: summary.model_id,
            status: ModelHealthStatusFFI::from(summary.status),
            status_text: summary.status_text,
            status_emoji: summary.status_emoji,
            reason: summary.reason,
            consecutive_successes: summary.consecutive_successes,
            consecutive_failures: summary.consecutive_failures,
        }
    }
}

impl From<&ModelHealthSummary> for ModelHealthSummaryFFI {
    fn from(summary: &ModelHealthSummary) -> Self {
        Self {
            model_id: summary.model_id.clone(),
            status: ModelHealthStatusFFI::from(summary.status),
            status_text: summary.status_text.clone(),
            status_emoji: summary.status_emoji.clone(),
            reason: summary.reason.clone(),
            consecutive_successes: summary.consecutive_successes,
            consecutive_failures: summary.consecutive_failures,
        }
    }
}

// ============================================================================
// Health Statistics Struct
// ============================================================================

/// Overall health statistics for all tracked models
#[derive(Debug, Clone)]
pub struct HealthStatisticsFFI {
    pub total: u32,
    pub healthy: u32,
    pub degraded: u32,
    pub unhealthy: u32,
    pub circuit_open: u32,
    pub half_open: u32,
    pub unknown: u32,
    pub healthy_percent: f64,
}

impl From<HealthStatistics> for HealthStatisticsFFI {
    fn from(stats: HealthStatistics) -> Self {
        Self {
            total: stats.total as u32,
            healthy: stats.healthy as u32,
            degraded: stats.degraded as u32,
            unhealthy: stats.unhealthy as u32,
            circuit_open: stats.circuit_open as u32,
            half_open: stats.half_open as u32,
            unknown: stats.unknown as u32,
            healthy_percent: stats.healthy_percent(),
        }
    }
}

impl From<&HealthStatistics> for HealthStatisticsFFI {
    fn from(stats: &HealthStatistics) -> Self {
        Self {
            total: stats.total as u32,
            healthy: stats.healthy as u32,
            degraded: stats.degraded as u32,
            unhealthy: stats.unhealthy as u32,
            circuit_open: stats.circuit_open as u32,
            half_open: stats.half_open as u32,
            unknown: stats.unknown as u32,
            healthy_percent: stats.healthy_percent(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_health_status_ffi_conversion() {
        let statuses = [
            (HealthStatus::Healthy, ModelHealthStatusFFI::Healthy),
            (HealthStatus::Degraded, ModelHealthStatusFFI::Degraded),
            (HealthStatus::Unhealthy, ModelHealthStatusFFI::Unhealthy),
            (HealthStatus::CircuitOpen, ModelHealthStatusFFI::CircuitOpen),
            (HealthStatus::HalfOpen, ModelHealthStatusFFI::HalfOpen),
            (HealthStatus::Unknown, ModelHealthStatusFFI::Unknown),
        ];

        for (status, expected_ffi) in statuses {
            let ffi: ModelHealthStatusFFI = status.into();
            assert_eq!(ffi, expected_ffi);

            let back: HealthStatus = ffi.into();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn test_model_health_summary_ffi_conversion() {
        let summary = ModelHealthSummary {
            model_id: "claude-opus".to_string(),
            status: HealthStatus::Healthy,
            status_text: "Healthy".to_string(),
            status_emoji: "✅".to_string(),
            reason: None,
            consecutive_successes: 10,
            consecutive_failures: 0,
        };

        let ffi: ModelHealthSummaryFFI = summary.clone().into();
        assert_eq!(ffi.model_id, "claude-opus");
        assert_eq!(ffi.status, ModelHealthStatusFFI::Healthy);
        assert_eq!(ffi.status_text, "Healthy");
        assert_eq!(ffi.status_emoji, "✅");
        assert!(ffi.reason.is_none());
        assert_eq!(ffi.consecutive_successes, 10);
        assert_eq!(ffi.consecutive_failures, 0);

        // Test from reference
        let ffi_ref: ModelHealthSummaryFFI = (&summary).into();
        assert_eq!(ffi_ref.model_id, "claude-opus");
    }

    #[test]
    fn test_model_health_summary_ffi_with_reason() {
        let summary = ModelHealthSummary {
            model_id: "gpt-4o".to_string(),
            status: HealthStatus::Degraded,
            status_text: "Degraded".to_string(),
            status_emoji: "⚠️".to_string(),
            reason: Some("High latency: p95 2500ms (threshold: 1000ms)".to_string()),
            consecutive_successes: 3,
            consecutive_failures: 2,
        };

        let ffi: ModelHealthSummaryFFI = summary.into();
        assert_eq!(ffi.status, ModelHealthStatusFFI::Degraded);
        assert!(ffi.reason.is_some());
        assert!(ffi.reason.unwrap().contains("High latency"));
    }

    #[test]
    fn test_health_statistics_ffi_conversion() {
        let stats = HealthStatistics {
            total: 5,
            healthy: 3,
            degraded: 1,
            unhealthy: 0,
            circuit_open: 0,
            half_open: 0,
            unknown: 1,
        };

        let ffi: HealthStatisticsFFI = stats.clone().into();
        assert_eq!(ffi.total, 5);
        assert_eq!(ffi.healthy, 3);
        assert_eq!(ffi.degraded, 1);
        assert_eq!(ffi.unhealthy, 0);
        assert_eq!(ffi.circuit_open, 0);
        assert_eq!(ffi.half_open, 0);
        assert_eq!(ffi.unknown, 1);
        assert!((ffi.healthy_percent - 60.0).abs() < 0.01); // 3/5 = 60%

        // Test from reference
        let ffi_ref: HealthStatisticsFFI = (&stats).into();
        assert_eq!(ffi_ref.total, 5);
    }
}
