// Aleph/core/src/event/mod.rs
//! Event-driven architecture for Aleph's agentic loop.
//!
//! This module provides:
//! - `EventBus`: Type-safe broadcast channel for component communication
//! - `AlephEvent`: Unified event enum for all system events
//! - `EventHandler`: Trait for components to subscribe and handle events

mod bus;
pub mod filter;
pub mod global_bus;
mod handler;
mod types;

#[cfg(test)]
mod integration_test;
#[cfg(test)]
mod tests;

pub use bus::{EventBus, EventBusConfig, EventBusError, EventSubscriber};
pub use handler::{EventContext, EventHandler, EventHandlerRegistry, HandlerError};
pub use types::{
    // AI response
    AiResponse,
    AlephEvent,
    CompactionInfo,
    ErrorKind,
    EventType,
    InputContext,
    // Input events
    InputEvent,
    // Loop control
    LoopState,
    // Planning events
    PlanRequest,
    PlanStep,
    SessionDiff,
    // Session events
    SessionInfo,
    StepStatus,
    StopReason,
    // Sub-agent events
    SubAgentCompletionEvent,
    TaskPlan,
    TimestampedEvent,
    // Token usage
    TokenUsage,
    ToolCallError,
    // Tool events
    ToolCallRequest,
    ToolCallResult,
    ToolCallRetry,
    ToolCallStarted,
    // User interaction
    UserQuestion,
};

// Event filtering for subscription-based routing
pub use filter::EventFilter;
pub use global_bus::{GlobalBus, GlobalEvent, Subscription, SubscriptionId};
