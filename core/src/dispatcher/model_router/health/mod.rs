//! Health monitoring and metrics collection.
//!
//! This module provides health tracking and observability:
//! - Health status and circuit breaker
//! - Health manager for model health tracking
//! - Metrics collection and storage
//! - Health state transitions

pub mod collector;
pub mod manager;
pub mod metrics;
pub mod status;
pub mod transition;

pub use collector::{
    HybridMetricsCollector, InMemoryMetricsCollector, MetricsCollector, MetricsConfig,
    MetricsError, RingBuffer,
};
pub use manager::{HealthManager, HealthStatistics};
pub use metrics::{
    CallOutcome, CallRecord, CostStats, ErrorDistribution, IntentMetrics, LatencyStats,
    ModelMetrics, MultiWindowMetrics, RateLimitState, UserFeedback, WindowConfig,
};
pub use status::{
    CallPermission, CircuitBreakerConfig, CircuitBreakerState, CircuitState, DegradationReason,
    ErrorType, HealthConfig, HealthError, HealthEvent, HealthStatus, ModelHealth,
    ModelHealthSummary, ProbeConfig, ProbeEndpoint, RateLimitInfo, UnhealthyReason,
};
pub use transition::{CallResult, HealthTransitionEngine, TransitionResult};
