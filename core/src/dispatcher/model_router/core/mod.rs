//! Core routing components.
//!
//! This module provides the fundamental building blocks for model routing:
//! - Model profiles and capabilities
//! - Routing rules and strategies
//! - Model matching and selection
//! - Dynamic scoring
//! - Task context management

pub mod context;
pub mod intent;
pub mod matcher;
pub mod profiles;
pub mod rules;
pub mod scoring;

pub use context::TaskContextManager;
pub use intent::TaskIntent;
pub use matcher::{FallbackProvider, ModelMatcher, ModelRouter, RoutingError};
pub use profiles::{Capability, CostTier, LatencyTier, ModelProfile};
pub use rules::{CostStrategy, ModelRoutingRules};
pub use scoring::{DynamicScorer, ScoreResult, ScoringConfig};
