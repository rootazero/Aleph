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

/// Census tests for the runtime wiring (the database and the provider
/// registry). Test-only; the module exists purely to hold the guards.
#[cfg(test)]
mod wiring_census;

pub use channel::DefaultGroupChatCommandParser;
pub use executor::GroupChatExecutor;
pub use orchestrator::{GroupChatOrchestrator, SharedSession};
pub use persona::PersonaRegistry;
pub use protocol::*;
pub use session::GroupChatSession;
