//! Multi-Agent Group Chat
//!
//! Channel-agnostic orchestration for multi-persona collaborative discussions.

pub mod channel;
pub mod coordinator;
pub mod orchestrator;
pub mod persona;
pub mod protocol;
pub mod session;

pub use channel::{GroupChatCommandParser, GroupChatRenderer};
pub use orchestrator::GroupChatOrchestrator;
pub use persona::PersonaRegistry;
pub use protocol::*;
pub use session::GroupChatSession;
