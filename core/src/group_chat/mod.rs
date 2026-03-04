//! Multi-Agent Group Chat
//!
//! Channel-agnostic orchestration for multi-persona collaborative discussions.

pub mod coordinator;
pub mod orchestrator;
pub mod persona;
pub mod protocol;
pub mod session;

pub use orchestrator::GroupChatOrchestrator;
pub use persona::PersonaRegistry;
pub use protocol::*;
pub use session::GroupChatSession;
