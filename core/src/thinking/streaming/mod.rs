//! Streaming thinking block processing.
//!
//! Provides:
//! - Stream event definitions
//! - Thinking tag detection and parsing
//! - Block reply chunking for TTS
//! - Callback-based stream subscription

pub mod events;
pub mod block_state;
pub mod block_reply_chunker;
pub mod subscriber;

pub use events::{StreamEvent, TokenUsage};
pub use block_state::{BlockState, ThinkingTagParser};
pub use block_reply_chunker::BlockReplyChunker;
pub use subscriber::StreamSubscriber;
