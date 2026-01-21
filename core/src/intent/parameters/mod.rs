//! Parameter management for intent classification.
//!
//! This module provides parameter types, defaults resolution, preset scenarios,
//! and matching context for intelligent intent routing.

pub mod context;
pub mod defaults;
pub mod presets;
pub mod types;

pub use context::{
    AppContext, ConversationContext, InputFeatures, MatchingContext, MatchingContextBuilder,
    PendingParam, TimeContext,
};
pub use defaults::DefaultsResolver;
pub use presets::{PresetRegistry, ScenarioPreset};
pub use types::{ConflictResolution, OrganizeMethod, ParameterSource, TaskParameters};
