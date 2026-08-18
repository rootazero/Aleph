// Aleph/core/src/event/mod.rs
//! Event-driven architecture for Aleph's agentic loop.
//!
//! This module provides:
//! - `EventBus`: Local per-instance broadcast channel (used as a context bus
//!   for `EventHandler::handle`)
//! - `AlephEvent`: Unified event enum — only variants with both a live
//!   producer and a live subscriber are kept
//! - `EventHandler`: Trait for components to subscribe and handle events
//! - `GlobalBus`: Singleton event aggregator for cross-agent event routing

pub mod filter;
pub mod global_bus;
mod handler;
mod types;

#[cfg(test)]
mod tests;

pub use handler::{EventContext, EventHandler, HandlerError};
pub use types::{AlephEvent, EventType, ProcessCompletionEvent, SubAgentCompletionEvent};

// Event filtering for subscription-based routing
pub use filter::EventFilter;
pub use global_bus::{GlobalBus, GlobalEvent, Subscription, SubscriptionId};
