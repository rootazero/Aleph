//! Streaming thinking block processing.
//!
//! Provides:
//! - Stream event definitions
//! - Thinking tag detection and parsing
//! - Block reply chunking for TTS
//! - Block coalescing for message batching
//! - Callback-based stream subscription

pub mod block_coalescer;
pub mod block_reply_chunker;
pub mod block_state;
pub mod events;
pub mod subscriber;

pub use block_coalescer::{AsyncBlockCoalescer, BlockCoalescer, CoalescingConfig};
pub use block_reply_chunker::{BlockReplyChunker, ChunkerConfig};
pub use block_state::{BlockState, ThinkingTagParser};
pub use events::{StreamEvent, TokenUsage};
pub use subscriber::StreamSubscriber;
