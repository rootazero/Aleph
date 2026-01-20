//! Multi-Model Router Module
//!
//! This module provides intelligent routing of AI tasks to optimal models
//! based on task characteristics, model capabilities, and cost/latency preferences.
//!
//! # Architecture
//!
//! ```text
//! Task
//!   │
//!   ▼
//! ┌─────────────────┐
//! │  ModelMatcher   │ ◀── ModelProfiles + RoutingRules
//! └─────────────────┘
//!   │
//!   ▼
//! ModelProfile (selected)
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::dispatcher::model_router::{ModelMatcher, ModelProfile, Capability};
//!
//! // Create matcher with profiles
//! let matcher = ModelMatcher::new(profiles, rules);
//!
//! // Route task to optimal model
//! let profile = matcher.route(&task)?;
//! ```

mod collector;
mod context;
mod health;
mod health_manager;
mod intelligent_routing;
mod intent;
mod matcher;
mod metrics;
mod profiles;
mod rules;
mod scoring;
mod transition_engine;

pub use collector::{
    HybridMetricsCollector, InMemoryMetricsCollector, MetricsCollector, MetricsConfig,
    MetricsError, RingBuffer,
};
pub use context::TaskContextManager;
pub use intent::TaskIntent;
pub use matcher::{FallbackProvider, ModelMatcher, ModelRouter, RoutingError};
pub use metrics::{
    CallOutcome, CallRecord, CostStats, ErrorDistribution, IntentMetrics, LatencyStats,
    ModelMetrics, MultiWindowMetrics, RateLimitState, UserFeedback, WindowConfig,
};
pub use profiles::{Capability, CostTier, LatencyTier, ModelProfile};
pub use rules::{CostStrategy, ModelRoutingRules};
pub use scoring::{DynamicScorer, ScoreResult, ScoringConfig};
pub use health::{
    CallPermission, CircuitBreakerConfig, CircuitBreakerState, CircuitState, DegradationReason,
    ErrorType, HealthConfig, HealthError, HealthEvent, HealthStatus, ModelHealth,
    ModelHealthSummary, ProbeConfig, ProbeEndpoint, RateLimitInfo, UnhealthyReason,
};
pub use transition_engine::{CallResult, HealthTransitionEngine, TransitionResult};
pub use health_manager::{HealthManager, HealthStatistics};
pub use intelligent_routing::{IntelligentRouter, IntelligentRoutingConfig, IntelligentRoutingResult};

// Re-export StageResult from cowork_types module for backward compatibility
pub use super::cowork_types::StageResult;
