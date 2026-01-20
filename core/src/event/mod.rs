// Aether/core/src/event/mod.rs
//! Event-driven architecture for Aether's agentic loop.
//!
//! This module provides:
//! - `EventBus`: Type-safe broadcast channel for component communication
//! - `AetherEvent`: Unified event enum for all system events
//! - `EventHandler`: Trait for components to subscribe and handle events

mod bus;
mod handler;
mod types;

#[cfg(test)]
mod integration_test;

pub use bus::{EventBus, EventBusConfig, EventBusError, EventSubscriber};
pub use handler::{EventContext, EventHandler, EventHandlerRegistry, HandlerError};
pub use types::{
    AetherEvent,
    // AI response
    AiResponse,
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
    SubAgentRequest,
    SubAgentResult,
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
    UserResponse,
};
