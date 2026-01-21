//! Advanced features (P3).
//!
//! This module provides advanced routing capabilities:
//! - A/B testing framework for experimentation
//! - Multi-model ensemble for quality improvement
//! - P3 router integrating all P3 features with P2

pub mod ab_testing;
pub mod ensemble;
pub mod p3_router;

pub use ab_testing::{
    ABTestingEngine, AssignmentStrategy, ExperimentConfig, ExperimentId, ExperimentOutcome,
    ExperimentReport, ExperimentStatus, ExperimentValidationError, MetricStats, MetricSummary,
    OutcomeTracker, SignificanceCalculator, SignificanceResult, TrackedMetric, TrafficSplitManager,
    VariantAssignment, VariantConfig, VariantId, VariantStats, VariantSummary,
};
pub use ensemble::{
    create_scorer, jaccard_similarity, ConfidenceMarkersScorer, EnsembleConfig, EnsembleDecision,
    EnsembleEngine, EnsembleEngineConfig, EnsembleExecutionError, EnsembleMode, EnsembleRequest,
    EnsembleResult, EnsembleStrategy, EnsembleValidationError, LengthAndStructureScorer,
    LengthScorer, ModelExecutionResult, ParallelExecutor, QualityMetric, QualityScorer,
    RelevanceScorer, ResponseAggregator, StructureScorer, TokenUsage,
};
pub use p3_router::{
    P3EnsembleDecision, P3IntelligentRouter, P3PreRouteResult, P3RouterConfig, P3RouterError,
    P3RouterEvent, P3RoutingDecision, UserIdMode,
};
