//! Unified Routing Framework
//!
//! This module provides a unified multi-layer routing system that coordinates:
//!
//! - **L1 (Regex)**: Fast pattern matching (<10ms, confidence 1.0)
//! - **L2 (Semantic)**: Keyword and context matching (200-500ms, confidence 0.7)
//! - **L3 (Inference)**: AI-powered routing (>1s, confidence varies)
//! - **Default**: Fallback to general chat
//!
//! # Architecture
//!
//! ```text
//! User Input
//!      ↓
//! ┌─────────────────────────────────────────┐
//! │           UnifiedRouter                  │
//! │                                          │
//! │  ┌────────────────────────────────────┐ │
//! │  │ L1: Regex Layer                    │ │
//! │  │ - Explicit slash commands          │ │
//! │  │ - Config-based patterns            │ │
//! │  └────────────┬───────────────────────┘ │
//! │               ↓ (no match)              │
//! │  ┌────────────────────────────────────┐ │
//! │  │ L2: Semantic Layer                 │ │
//! │  │ - Keyword matching                 │ │
//! │  │ - Context inference                │ │
//! │  └────────────┬───────────────────────┘ │
//! │               ↓ (no match)              │
//! │  ┌────────────────────────────────────┐ │
//! │  │ L3: AI Inference Layer             │ │
//! │  │ - LLM-based tool selection         │ │
//! │  │ - Parameter extraction             │ │
//! │  └────────────┬───────────────────────┘ │
//! │               ↓ (no match)              │
//! │  ┌────────────────────────────────────┐ │
//! │  │ Default: General Chat              │ │
//! │  └────────────────────────────────────┘ │
//! └──────────────────────────────────────────┘
//!      ↓
//! RoutingResult { tool, confidence, layer, params }
//! ```
//!
//! # Intent Routing Pipeline
//!
//! The enhanced intent routing pipeline adds:
//!
//! - **Intent Cache**: Fast-path for repeated patterns
//! - **Confidence Calibration**: Tool-specific threshold adjustment
//! - **Intent Aggregation**: Combine signals from multiple layers
//! - **Clarification Flow**: Context-preserving parameter collection
//!
//! ```rust,ignore
//! use aethecore::routing::{IntentRoutingPipeline, PipelineConfig};
//!
//! let config = PipelineConfig::enabled();
//! let pipeline = IntentRoutingPipeline::new(config, ...);
//!
//! let result = pipeline.process(input, context).await?;
//!
//! match result {
//!     PipelineResult::Executed { .. } => { /* Tool executed */ }
//!     PipelineResult::PendingClarification(req) => { /* Need user input */ }
//!     PipelineResult::GeneralChat { .. } => { /* Fall back to chat */ }
//!     _ => {}
//! }
//! ```

// Core types
mod types;
mod unified;

// Intent routing pipeline types
mod aggregator;
mod cache;
mod calibrator;
mod clarification;
mod engine;
mod intent;
mod l1_regex;
mod l2_semantic;
mod l3_enhanced;
mod pipeline;
mod pipeline_config;
mod pipeline_result;

// Re-export core types
pub use types::{
    AppContextInfo, RoutingConfig, RoutingContext, RoutingLayerType, RoutingMatch, RoutingResult,
};
pub use unified::UnifiedRouter;

// Re-export intent types
pub use intent::{
    AggregatedIntent, CalibratedSignal, CalibrationFactor, IntentAction, IntentSignal,
    ParameterRequirement,
};

// Re-export pipeline config types
pub use pipeline_config::{
    ActionSuggestion, CacheConfig, ClarificationConfig, ConfidenceThresholds, ExecutionMode,
    LayerConfig, PipelineConfig, ToolConfidenceConfig,
};

// Re-export pipeline result types
pub use pipeline_result::{
    ClarificationError, ClarificationInputType, ClarificationRequest, PipelineResult, ResumeResult,
};

// Re-export cache types
pub use cache::{CacheMetrics, CachedIntent, IntentCache};

// Re-export calibrator types
pub use calibrator::{CalibrationHistory, ConfidenceCalibrator};

// Re-export layer execution types
pub use engine::{LayerExecutionEngine, LayerExecutionResult};
pub use l1_regex::L1RegexMatcher;
pub use l2_semantic::L2SemanticMatcher;
pub use l3_enhanced::EnhancedL3Router;

// Re-export aggregator types
pub use aggregator::IntentAggregator;

// Re-export clarification types
pub use clarification::{ClarificationIntegrator, PendingClarification};

// Re-export pipeline types
pub use pipeline::IntentRoutingPipeline;
