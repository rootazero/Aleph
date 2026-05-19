//! Swarm task coordination module.
//!
//! Provides the agent message bus and the coordination task DAG store that
//! back the team subsystem:
//! - [`AgentMessageBus`]: in-process agent-to-agent event channel
//! - `tasks`: the `CoordTaskStore` DAG used by the team dispatcher and tools

pub mod bus;
pub mod events;
pub mod tasks;

pub use bus::AgentMessageBus;
pub use events::{AgentEvent, CriticalEvent, EventTier, FileOperation, ImportantEvent, InfoEvent};
