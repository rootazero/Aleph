// Aether/core/src/event/mod.rs
//! Event-driven architecture for Aether's agentic loop.
//!
//! This module provides:
//! - `EventBus`: Type-safe broadcast channel for component communication
//! - `AetherEvent`: Unified event enum for all system events
//! - `EventHandler`: Trait for components to subscribe and handle events
//! - `PermissionEvent`: Permission request/reply events
//! - `QuestionEvent`: Structured user interaction events

mod bus;
pub mod filter;
pub mod global_bus;
mod handler;
pub mod permission;
pub mod question;
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

// Permission system events
pub use permission::{
    PermissionAction, PermissionEvent, PermissionReply, PermissionRequest, ToolCallRef,
};

// Question system events
pub use question::{
    Answer, QuestionEvent, QuestionInfo, QuestionOption, QuestionReply, QuestionRequest,
};

// Event filtering for subscription-based routing
pub use filter::EventFilter;
pub use global_bus::{GlobalBus, GlobalEvent, Subscription, SubscriptionId};
