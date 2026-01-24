//! Session compactor component - manages token limits via compaction.
//!
//! Subscribes to: LoopContinue, ToolCallCompleted
//! Publishes: SessionCompacted
//!
//! This component monitors session token usage and compacts the session
//! when approaching model context limits. Compaction involves:
//! 1. Pruning old tool outputs (keeping recent ones)
//! 2. Generating a summary of earlier session parts
//! 3. Replacing old parts with the summary

// Submodules
pub mod config;
pub mod model_limits;
pub mod token_usage;
mod compactor;
mod event_handler;

#[cfg(test)]
mod tests;

// Re-export public types
pub use config::{CompactionConfig, LlmCallback, PruneInfo, compaction_prompt};
pub use model_limits::{ModelLimit, TokenTracker};
pub use token_usage::EnhancedTokenUsage;
pub use compactor::SessionCompactor;
