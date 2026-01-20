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

mod budget;
mod collector;
mod context;
mod failover;
mod health;
mod health_manager;
mod intelligent_routing;
mod intent;
mod matcher;
mod metrics;
mod orchestrator;
mod profiles;
mod retry;
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

// Retry and Failover (P1)
pub use retry::{BackoffStrategy, RetryPolicy, RetryableOutcome};
pub use failover::{FailoverChain, FailoverConfig, FailoverSelectionMode};

// Budget Management (P1)
pub use budget::{
    BudgetCheckResult, BudgetEnforcement, BudgetEvent, BudgetLimit, BudgetManager,
    BudgetPeriod, BudgetScope, BudgetState, CostEstimate, CostEstimator, ModelPricing,
    PricingSource,
};

// Retry Orchestrator (P1)
pub use orchestrator::{
    AttemptRecord, ExecutionError, ExecutionRequest, ExecutionResult, OrchestratorConfig,
    OrchestratorEvent, RetryOrchestrator,
};

// Re-export StageResult from cowork_types module for backward compatibility
pub use super::cowork_types::StageResult;
