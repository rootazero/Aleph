//! Support functionality for intent classification.
//!
//! This module provides caching, rollback support, and legacy prompt templates.

pub mod agent_prompt;
pub mod cache;
pub mod rollback;

pub use agent_prompt::{AgentModePrompt, GenerationModelInfo, ToolDescription};
pub use cache::{CacheConfig, CacheMetrics, CachedIntent, IntentCache};
pub use rollback::{
    RollbackCapable, RollbackConfig, RollbackEntry, RollbackManager, RollbackResult,
};
