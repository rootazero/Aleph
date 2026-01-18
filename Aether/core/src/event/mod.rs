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
    AetherEvent, EventType, TimestampedEvent,
    // Input events
    InputEvent, InputContext,
    // Planning events
    PlanRequest, TaskPlan, PlanStep, StepStatus,
    // Tool events
    ToolCallRequest, ToolCallStarted, ToolCallResult, ToolCallError, ToolCallRetry, ErrorKind,
    // Loop control
    LoopState, StopReason,
    // Session events
    SessionInfo, SessionDiff, CompactionInfo,
    // Sub-agent events
    SubAgentRequest, SubAgentResult,
    // User interaction
    UserQuestion, UserResponse,
    // AI response
    AiResponse,
    // Token usage
    TokenUsage,
};
