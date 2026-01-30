//! Streaming thinking block processing.
//!
//! Provides:
//! - Stream event definitions
//! - Thinking tag detection and parsing
//! - Block reply chunking for TTS
//! - Block coalescing for message batching
//! - Callback-based stream subscription

pub mod events;
pub mod block_state;
pub mod block_reply_chunker;
pub mod block_coalescer;
pub mod subscriber;

pub use events::{StreamEvent, TokenUsage};
pub use block_state::{BlockState, ThinkingTagParser};
pub use block_reply_chunker::{BlockReplyChunker, ChunkerConfig};
pub use block_coalescer::{BlockCoalescer, CoalescingConfig, AsyncBlockCoalescer};
pub use subscriber::StreamSubscriber;
