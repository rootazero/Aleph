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
//!   │
//!   ▼
//! ┌─────────────────┐
//! │ PipelineExecutor│ ◀── TaskContextManager
//! └─────────────────┘
//!   │
//!   ▼
//! StageResult
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::cowork::model_router::{ModelMatcher, ModelProfile, Capability};
//!
//! // Create matcher with profiles
//! let matcher = ModelMatcher::new(profiles, rules);
//!
//! // Route task to optimal model
//! let profile = matcher.route(&task)?;
//! ```

mod context;
mod intent;
mod matcher;
mod pipeline;
mod profiles;
mod rules;

pub use context::{
    ContextError, ContextSummary, StoredTaskResult, TaskContext, TaskContextManager,
    TaskResultMetadata,
};
pub use intent::TaskIntent;
pub use matcher::{FallbackProvider, ModelMatcher, ModelRouter, RoutingError};
pub use pipeline::{
    ExecutionResult, PipelineContext, PipelineError, PipelineEvent, PipelineExecutor,
    PipelineProgressHandler, PipelineStage, PipelineState, PipelineSummary, ProviderAdapter,
    StageResult,
};
pub use profiles::{Capability, CostTier, LatencyTier, ModelProfile};
pub use rules::{CostStrategy, ModelRoutingRules};
