//! Resilience and fault tolerance.
//!
//! This module provides reliability features:
//! - Retry policies and backoff strategies
//! - Failover chain management
//! - Budget management and cost control
//! - Orchestrated execution with retry and failover

pub mod budget;
pub mod failover;
pub mod orchestrator;
pub mod retry;
pub mod router;

pub use budget::{
    BudgetCheckResult, BudgetEnforcement, BudgetEvent, BudgetLimit, BudgetManager, BudgetPeriod,
    BudgetScope, BudgetState, CostEstimate, CostEstimator, ModelPricing, PricingSource,
};
pub use failover::{FailoverChain, FailoverConfig, FailoverSelectionMode};
pub use orchestrator::{
    AttemptRecord, ExecutionError, ExecutionRequest, ExecutionResult, OrchestratorConfig,
    OrchestratorEvent, RetryOrchestrator,
};
pub use retry::{BackoffStrategy, RetryPolicy, RetryableOutcome};
pub use router::{
    BudgetCheckResultSummary, OrchestratedRouter, OrchestratedRouterConfig, RouterEvent,
    RoutingExecutionError, RoutingRequest, RoutingResult,
};
