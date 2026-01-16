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
mod profiles;
mod rules;

pub use context::TaskContextManager;
pub use intent::TaskIntent;
pub use matcher::{FallbackProvider, ModelMatcher, ModelRouter, RoutingError};
pub use profiles::{Capability, CostTier, LatencyTier, ModelProfile};
pub use rules::{CostStrategy, ModelRoutingRules};

// Re-export StageResult from types module for backward compatibility
pub use super::types::StageResult;
