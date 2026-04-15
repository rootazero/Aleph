//! Discord Channel Handlers
//!
//! Reusable handler components for Discord channel features.

pub mod approval;
pub mod interaction;
pub mod streaming;
pub mod thread;

pub use approval::{ApprovalError, ApprovalQueue, ApprovalStatus, PendingExec};
pub use interaction::{InteractionError, InteractionHandler, InteractionResult};
pub use streaming::{StreamingError, StreamingHandler, StreamingPreview};
pub use thread::{AgentId, ThreadBindingError, ThreadBindingHandler, ThreadInfo};
