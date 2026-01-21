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
//! # Module Structure
//!
//! - **core/**: Core routing (matcher, profiles, rules, scoring)
//! - **health/**: Health monitoring (status, manager, metrics, collector)
//! - **resilience/**: Fault tolerance (retry, failover, budget, orchestrator)
//! - **intelligent/**: Smart routing P2 (prompt analyzer, semantic cache)
//! - **advanced/**: Advanced features P3 (A/B testing, ensemble)
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

// Submodules
pub mod advanced;
pub mod core;
pub mod health;
pub mod intelligent;
pub mod resilience;

// Re-export from core
pub use core::{
    Capability, CostStrategy, CostTier, DynamicScorer, FallbackProvider, LatencyTier,
    ModelMatcher, ModelProfile, ModelRouter, ModelRoutingRules, RoutingError, ScoreResult,
    ScoringConfig, TaskContextManager, TaskIntent,
};

// Re-export from health
pub use health::{
    CallOutcome, CallPermission, CallRecord, CallResult, CircuitBreakerConfig, CircuitBreakerState,
    CircuitState, CostStats, DegradationReason, ErrorDistribution, ErrorType, HealthConfig,
    HealthError, HealthEvent, HealthManager, HealthStatistics, HealthStatus,
    HealthTransitionEngine, HybridMetricsCollector, InMemoryMetricsCollector, IntentMetrics,
    LatencyStats, MetricsCollector, MetricsConfig, MetricsError, ModelHealth, ModelHealthSummary,
    ModelMetrics, MultiWindowMetrics, ProbeConfig, ProbeEndpoint, RateLimitInfo, RateLimitState,
    RingBuffer, TransitionResult, UnhealthyReason, UserFeedback, WindowConfig,
};

// Re-export from resilience (P1)
pub use resilience::{
    AttemptRecord, BackoffStrategy, BudgetCheckResult, BudgetCheckResultSummary, BudgetEnforcement,
    BudgetEvent, BudgetLimit, BudgetManager, BudgetPeriod, BudgetScope, BudgetState, CostEstimate,
    CostEstimator, ExecutionError, ExecutionRequest, ExecutionResult, FailoverChain,
    FailoverConfig, FailoverSelectionMode, ModelPricing, OrchestratedRouter,
    OrchestratedRouterConfig, OrchestratorConfig, OrchestratorEvent, PricingSource, RetryOrchestrator,
    RetryPolicy, RetryableOutcome, RouterEvent, RoutingExecutionError, RoutingRequest,
    RoutingResult,
};

// Re-export from intelligent (P2)
pub use intelligent::{
    CacheEntry, CacheHit, CacheHitType, CacheMetadata, CacheStats, CachedResponse,
    ComplexityWeights, ContextSize, Domain, EmbeddingError, EvictionPolicy, FastEmbedEmbedder,
    InMemoryVectorStore, IntelligentRouter, IntelligentRoutingConfig, IntelligentRoutingResult,
    Language, P2IntelligentRouter, P2RouterConfig, P2RouterError, PreRouteResult, PromptAnalysisError,
    PromptAnalyzer, PromptAnalyzerConfig, PromptFeatures, ReasoningLevel, RoutingDecision,
    SemanticCacheConfig, SemanticCacheError, SemanticCacheManager, TechnicalDomain, TextEmbedder,
};

// Re-export from advanced (P3)
pub use advanced::{
    ABTestingEngine, AssignmentStrategy, ConfidenceMarkersScorer, EnsembleConfig, EnsembleDecision,
    EnsembleEngine, EnsembleEngineConfig, EnsembleExecutionError, EnsembleMode, EnsembleRequest,
    EnsembleResult, EnsembleStrategy, EnsembleValidationError, ExperimentConfig, ExperimentId,
    ExperimentOutcome, ExperimentReport, ExperimentStatus, ExperimentValidationError,
    LengthAndStructureScorer, LengthScorer, MetricStats, MetricSummary, ModelExecutionResult,
    OutcomeTracker, P3EnsembleDecision, P3IntelligentRouter, P3PreRouteResult, P3RouterConfig,
    P3RouterError, P3RouterEvent, P3RoutingDecision, ParallelExecutor, QualityMetric, QualityScorer,
    RelevanceScorer, ResponseAggregator, SignificanceCalculator, SignificanceResult,
    StructureScorer, TokenUsage, TrackedMetric, TrafficSplitManager, UserIdMode, VariantAssignment,
    VariantConfig, VariantId, VariantStats, VariantSummary, create_scorer, jaccard_similarity,
};

// Re-export StageResult from agent_types module for backward compatibility
pub use super::agent_types::StageResult;
