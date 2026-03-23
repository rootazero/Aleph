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

