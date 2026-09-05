// Aleph/core/src/event/mod.rs
//! Event-driven architecture for Aleph's agentic loop.
//!
//! This module provides:
//! - `GlobalBus`: Singleton event aggregator for cross-agent event routing.
//!   Producers broadcast through `GlobalBus::global()`; subscribers attach
//!   via `subscribe_async` with an `EventFilter`. (The per-instance
//!   `EventBus` was removed in the 2026-08 severed-wire audits.)
//! - `AlephEvent`: Unified event enum — only variants with both a live
//!   producer and a live subscriber are kept
//! - `EventHandler`: Trait for components to subscribe and handle events
//! - `EventFilter`: Subscription-side filtering by event type, session, agent

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
pub use global_bus::{GlobalBus, GlobalEvent, SubscriptionId};
