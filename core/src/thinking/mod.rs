//! Thinking level and streaming processing module.
//!
//! Provides:
//! - Six thinking levels (off/minimal/low/medium/high/xhigh)
//! - Streaming event definitions
//! - Thinking tag detection and parsing
//! - Block reply chunking for TTS
//! - Callback-based stream subscription

pub mod streaming;

// Re-export streaming types
pub use streaming::{
    BlockReplyChunker, BlockState, StreamEvent, StreamSubscriber, TokenUsage,
    ThinkingTagParser,
};
