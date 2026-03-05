//! Multi-Agent Group Chat
//!
//! Channel-agnostic orchestration for multi-persona collaborative discussions.

pub mod channel;
pub mod coordinator;
pub mod executor;
pub mod orchestrator;
pub mod persona;
pub mod protocol;
pub mod session;

pub use channel::{GroupChatCommandParser, GroupChatRenderer};
pub use executor::GroupChatExecutor;
pub use orchestrator::{GroupChatOrchestrator, SharedSession};
pub use persona::PersonaRegistry;
pub use protocol::*;
pub use session::GroupChatSession;
