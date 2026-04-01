//! Shared context data sources for prompt building.

pub mod environment;
pub mod memory_context;
pub mod session_info;

pub use environment::EnvironmentInfo;
pub use memory_context::{ConversationSnippet, MemoryContext, MemoryFact};
pub use session_info::SessionInfo;
