//! Context Compressor (stubbed)
//!
//! The compressor was designed for the old OTAF agent loop and has been
//! stubbed out. The types are preserved for backward compatibility but
//! the implementation is a no-op.

pub mod context_stats;
pub mod smart_compactor;
pub mod smart_strategy;
pub mod strategy;
pub mod tool_truncator;
pub mod turn_protector;

pub use context_stats::{CompressionFocus, WarningLevel};
pub use smart_compactor::{CompactionResult, SmartCompactor};
pub use smart_strategy::{CompactionAction, SmartCompactionStrategy};
pub use strategy::CompressionPrompt;
pub use tool_truncator::{ToolTruncator, TruncatedOutput};
pub use turn_protector::TurnProtector;

/// Generates the system prompt injected before context compaction
/// to give the agent a chance to persist important information to long-term memory.
pub struct PreCompactionPrompt;

impl PreCompactionPrompt {
    /// Build the flush prompt that instructs the agent to save key facts
    /// before context compaction discards older turns.
    pub fn build() -> String {
        concat!(
            "SYSTEM: Your conversation context is about to be compacted to save space. ",
            "Before this happens, review the recent conversation and use the memory_store tool ",
            "to persist any important information that should be remembered long-term. ",
            "Focus on: user preferences, decisions made, key facts learned, and task progress. ",
            "Do NOT store trivial greetings or small talk. ",
            "After storing, respond with only: [memory_flush_complete]"
        )
        .to_string()
    }
}
