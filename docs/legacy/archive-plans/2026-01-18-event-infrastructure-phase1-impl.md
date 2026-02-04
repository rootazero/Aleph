# Phase 1: Event Infrastructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the event bus core infrastructure with type-safe broadcast channels, event types, and handler traits.

**Architecture:** Create a new `event/` module with EventBus using tokio broadcast channels, AlephEvent enum for type-safe events, and EventHandler trait for component subscription.

**Tech Stack:** tokio (broadcast channel), serde (serialization), chrono (timestamps), uuid (event IDs)

---

## Task 1: Create Event Module Structure

**Files:**
- Create: `Aleph/core/src/event/mod.rs`
- Create: `Aleph/core/src/event/types.rs`
- Create: `Aleph/core/src/event/bus.rs`
- Create: `Aleph/core/src/event/handler.rs`
- Modify: `Aleph/core/src/lib.rs` (add module declaration)

**Step 1: Create event module directory and mod.rs**

```rust
// Aleph/core/src/event/mod.rs
//! Event-driven architecture for Aleph's agentic loop.
//!
//! This module provides:
//! - `EventBus`: Type-safe broadcast channel for component communication
//! - `AlephEvent`: Unified event enum for all system events
//! - `EventHandler`: Trait for components to subscribe and handle events

mod bus;
mod handler;
mod types;

pub use bus::{EventBus, EventSubscriber};
pub use handler::{EventContext, EventHandler, EventHandlerRegistry};
pub use types::{
    AlephEvent, EventType, TimestampedEvent,
    // Input events
    InputEvent,
    // Planning events
    PlanRequest, TaskPlan, PlanStep, StepStatus,
    // Tool events
    ToolCallRequest, ToolCallStarted, ToolCallResult, ToolCallError, ToolCallRetry,
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
```

**Step 2: Run syntax check**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo check 2>&1 | head -20`
Expected: Error about missing files (bus.rs, handler.rs, types.rs)

**Step 3: Commit module structure placeholder**

```bash
git add Aleph/core/src/event/mod.rs
git commit -m "feat(event): add event module structure placeholder"
```

---

## Task 2: Define Core Event Types

**Files:**
- Create: `Aleph/core/src/event/types.rs`

**Step 1: Write event type definitions**

```rust
// Aleph/core/src/event/types.rs
//! Event type definitions for the event-driven architecture.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

// Global sequence counter for events
static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Generate next event sequence number
fn next_sequence() -> u64 {
    EVENT_SEQUENCE.fetch_add(1, Ordering::SeqCst)
}

// ============================================================================
// Core Event Types
// ============================================================================

/// Timestamped event wrapper for history tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampedEvent {
    pub event: AlephEvent,
    pub timestamp: i64,
    pub sequence: u64,
}

impl TimestampedEvent {
    pub fn new(event: AlephEvent) -> Self {
        Self {
            event,
            timestamp: chrono::Utc::now().timestamp_millis(),
            sequence: next_sequence(),
        }
    }
}

/// Event type discriminant for subscription filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    // Input
    InputReceived,

    // Planning
    PlanRequested,
    PlanCreated,

    // Tool execution
    ToolCallRequested,
    ToolCallStarted,
    ToolCallCompleted,
    ToolCallFailed,
    ToolCallRetrying,

    // Loop control
    LoopContinue,
    LoopStop,

    // Session
    SessionCreated,
    SessionUpdated,
    SessionResumed,
    SessionCompacted,

    // Sub-agent
    SubAgentStarted,
    SubAgentCompleted,

    // User interaction
    UserQuestionAsked,
    UserResponseReceived,

    // AI response
    AiResponseGenerated,

    // Wildcard for components that want all events
    All,
}

/// Unified event enum - all events in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AlephEvent {
    // Input events
    InputReceived(InputEvent),

    // Planning events
    PlanRequested(PlanRequest),
    PlanCreated(TaskPlan),

    // Tool execution events
    ToolCallRequested(ToolCallRequest),
    ToolCallStarted(ToolCallStarted),
    ToolCallCompleted(ToolCallResult),
    ToolCallFailed(ToolCallError),
    ToolCallRetrying(ToolCallRetry),

    // Loop control events
    LoopContinue(LoopState),
    LoopStop(StopReason),

    // Session events
    SessionCreated(SessionInfo),
    SessionUpdated(SessionDiff),
    SessionResumed(SessionInfo),
    SessionCompacted(CompactionInfo),

    // Sub-agent events
    SubAgentStarted(SubAgentRequest),
    SubAgentCompleted(SubAgentResult),

    // User interaction events
    UserQuestionAsked(UserQuestion),
    UserResponseReceived(UserResponse),

    // AI response events
    AiResponseGenerated(AiResponse),
}

impl AlephEvent {
    /// Get the event type discriminant
    pub fn event_type(&self) -> EventType {
        match self {
            Self::InputReceived(_) => EventType::InputReceived,
            Self::PlanRequested(_) => EventType::PlanRequested,
            Self::PlanCreated(_) => EventType::PlanCreated,
            Self::ToolCallRequested(_) => EventType::ToolCallRequested,
            Self::ToolCallStarted(_) => EventType::ToolCallStarted,
            Self::ToolCallCompleted(_) => EventType::ToolCallCompleted,
            Self::ToolCallFailed(_) => EventType::ToolCallFailed,
            Self::ToolCallRetrying(_) => EventType::ToolCallRetrying,
            Self::LoopContinue(_) => EventType::LoopContinue,
            Self::LoopStop(_) => EventType::LoopStop,
            Self::SessionCreated(_) => EventType::SessionCreated,
            Self::SessionUpdated(_) => EventType::SessionUpdated,
            Self::SessionResumed(_) => EventType::SessionResumed,
            Self::SessionCompacted(_) => EventType::SessionCompacted,
            Self::SubAgentStarted(_) => EventType::SubAgentStarted,
            Self::SubAgentCompleted(_) => EventType::SubAgentCompleted,
            Self::UserQuestionAsked(_) => EventType::UserQuestionAsked,
            Self::UserResponseReceived(_) => EventType::UserResponseReceived,
            Self::AiResponseGenerated(_) => EventType::AiResponseGenerated,
        }
    }

    /// Get a human-readable name for the event
    pub fn name(&self) -> &'static str {
        match self {
            Self::InputReceived(_) => "InputReceived",
            Self::PlanRequested(_) => "PlanRequested",
            Self::PlanCreated(_) => "PlanCreated",
            Self::ToolCallRequested(_) => "ToolCallRequested",
            Self::ToolCallStarted(_) => "ToolCallStarted",
            Self::ToolCallCompleted(_) => "ToolCallCompleted",
            Self::ToolCallFailed(_) => "ToolCallFailed",
            Self::ToolCallRetrying(_) => "ToolCallRetrying",
            Self::LoopContinue(_) => "LoopContinue",
            Self::LoopStop(_) => "LoopStop",
            Self::SessionCreated(_) => "SessionCreated",
            Self::SessionUpdated(_) => "SessionUpdated",
            Self::SessionResumed(_) => "SessionResumed",
            Self::SessionCompacted(_) => "SessionCompacted",
            Self::SubAgentStarted(_) => "SubAgentStarted",
            Self::SubAgentCompleted(_) => "SubAgentCompleted",
            Self::UserQuestionAsked(_) => "UserQuestionAsked",
            Self::UserResponseReceived(_) => "UserResponseReceived",
            Self::AiResponseGenerated(_) => "AiResponseGenerated",
        }
    }
}

// ============================================================================
// Input Event Types
// ============================================================================

/// User input event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputEvent {
    pub text: String,
    pub topic_id: Option<String>,
    pub context: Option<InputContext>,
    pub timestamp: i64,
}

/// Context captured with user input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputContext {
    pub app_name: Option<String>,
    pub app_bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub selected_text: Option<String>,
}

// ============================================================================
// Planning Event Types
// ============================================================================

/// Request to create a task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRequest {
    pub input: InputEvent,
    pub intent_type: Option<String>,
    pub detected_steps: Vec<String>,
}

/// Generated task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub id: String,
    pub steps: Vec<PlanStep>,
    pub parallel_groups: Vec<Vec<String>>,
    pub current_step_index: usize,
}

/// Single step in a task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub tool: String,
    pub parameters: Value,
    pub depends_on: Vec<String>,
    pub status: StepStatus,
}

/// Status of a plan step
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Skipped,
}

// ============================================================================
// Tool Execution Event Types
// ============================================================================

/// Request to call a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub tool: String,
    pub parameters: Value,
    pub plan_step_id: Option<String>,
}

/// Tool call has started
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallStarted {
    pub call_id: String,
    pub tool: String,
    pub input: Value,
    pub timestamp: i64,
}

/// Tool call completed successfully
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: String,
    pub tool: String,
    pub input: Value,
    pub output: String,
    pub started_at: i64,
    pub completed_at: i64,
    pub token_usage: TokenUsage,
}

/// Tool call failed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallError {
    pub call_id: String,
    pub tool: String,
    pub error: String,
    pub error_kind: ErrorKind,
    pub is_retryable: bool,
    pub attempts: u32,
}

/// Error classification for retry logic
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    InvalidInput,
    PermissionDenied,
    Timeout,
    RateLimit,
    ServiceUnavailable,
    ExecutionFailed,
    Aborted,
}

/// Tool call is being retried
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRetry {
    pub call_id: String,
    pub attempt: u32,
    pub delay_ms: u64,
    pub reason: Option<String>,
}

/// Token usage tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

// ============================================================================
// Loop Control Event Types
// ============================================================================

/// Current state of the agentic loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopState {
    pub session_id: String,
    pub iteration: u32,
    pub total_tokens: u64,
    pub last_tool: Option<String>,
}

/// Reason for stopping the loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StopReason {
    /// Task completed normally
    Completed,
    /// Hit iteration limit
    MaxIterationsReached,
    /// Detected infinite loop
    DoomLoopDetected,
    /// Context overflow
    TokenLimitReached,
    /// User cancelled
    UserAborted,
    /// Unrecoverable error
    Error(String),
    /// No steps to execute
    EmptyPlan,
}

// ============================================================================
// Session Event Types
// ============================================================================

/// Session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub model: String,
    pub created_at: i64,
}

/// Session state diff for incremental updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDiff {
    pub session_id: String,
    pub iteration_count: Option<u32>,
    pub total_tokens: Option<u64>,
    pub status: Option<String>,
}

/// Session compaction information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionInfo {
    pub session_id: String,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub timestamp: i64,
}

// ============================================================================
// Sub-agent Event Types
// ============================================================================

/// Request to start a sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRequest {
    pub agent_id: String,
    pub prompt: String,
    pub parent_session_id: String,
    pub child_session_id: String,
}

/// Sub-agent completed its task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub agent_id: String,
    pub child_session_id: String,
    pub summary: String,
    pub success: bool,
    pub error: Option<String>,
}

// ============================================================================
// User Interaction Event Types
// ============================================================================

/// Question asked to user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserQuestion {
    pub question_id: String,
    pub question: String,
    pub options: Option<Vec<String>>,
}

/// User's response to a question
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserResponse {
    pub question_id: String,
    pub response: String,
}

// ============================================================================
// AI Response Event Types
// ============================================================================

/// AI generated response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    pub content: String,
    pub reasoning: Option<String>,
    pub is_final: bool,
    pub timestamp: i64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_mapping() {
        let event = AlephEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        });

        assert_eq!(event.event_type(), EventType::InputReceived);
        assert_eq!(event.name(), "InputReceived");
    }

    #[test]
    fn test_timestamped_event_sequence() {
        let e1 = TimestampedEvent::new(AlephEvent::LoopStop(StopReason::Completed));
        let e2 = TimestampedEvent::new(AlephEvent::LoopStop(StopReason::Completed));

        assert!(e2.sequence > e1.sequence);
    }

    #[test]
    fn test_event_serialization() {
        let event = AlephEvent::ToolCallCompleted(ToolCallResult {
            call_id: "123".to_string(),
            tool: "search".to_string(),
            input: serde_json::json!({"query": "test"}),
            output: "results".to_string(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: TokenUsage::default(),
        });

        let json = serde_json::to_string(&event).unwrap();
        let parsed: AlephEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.event_type(), EventType::ToolCallCompleted);
    }
}
```

**Step 2: Run cargo check**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo check 2>&1 | head -30`
Expected: Still errors about missing bus.rs and handler.rs

**Step 3: Run tests for types module**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test event::types --no-fail-fast 2>&1 | tail -20`
Expected: Tests should pass once all files are created

**Step 4: Commit types module**

```bash
git add Aleph/core/src/event/types.rs
git commit -m "feat(event): add event type definitions

- Add AlephEvent enum with all event variants
- Add EventType discriminant for subscription filtering
- Add TimestampedEvent wrapper with sequence numbers
- Add all event payload structs (InputEvent, ToolCallResult, etc.)
- Include unit tests for serialization and type mapping"
```

---

## Task 3: Implement EventBus

**Files:**
- Create: `Aleph/core/src/event/bus.rs`

**Step 1: Write EventBus implementation**

```rust
// Aleph/core/src/event/bus.rs
//! Event bus implementation using tokio broadcast channels.

use crate::event::types::{AlephEvent, EventType, TimestampedEvent};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, trace, warn};

/// Default buffer size for the broadcast channel
const DEFAULT_BUFFER_SIZE: usize = 1024;

/// Maximum history size to keep
const MAX_HISTORY_SIZE: usize = 10000;

/// Event bus for component communication
///
/// Uses tokio broadcast channel for multi-subscriber support.
/// Events are type-safe and all subscribers receive all events.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<TimestampedEvent>,
    history: Arc<RwLock<Vec<TimestampedEvent>>>,
    config: EventBusConfig,
}

/// Configuration for the event bus
#[derive(Debug, Clone)]
pub struct EventBusConfig {
    /// Buffer size for the broadcast channel
    pub buffer_size: usize,
    /// Whether to keep event history
    pub enable_history: bool,
    /// Maximum history entries to keep
    pub max_history_size: usize,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            buffer_size: DEFAULT_BUFFER_SIZE,
            enable_history: true,
            max_history_size: MAX_HISTORY_SIZE,
        }
    }
}

/// Subscriber handle for receiving events
pub struct EventSubscriber {
    receiver: broadcast::Receiver<TimestampedEvent>,
    filter: Vec<EventType>,
}

impl EventBus {
    /// Create a new event bus with default configuration
    pub fn new() -> Self {
        Self::with_config(EventBusConfig::default())
    }

    /// Create a new event bus with custom buffer size
    pub fn with_buffer_size(buffer_size: usize) -> Self {
        Self::with_config(EventBusConfig {
            buffer_size,
            ..Default::default()
        })
    }

    /// Create a new event bus with custom configuration
    pub fn with_config(config: EventBusConfig) -> Self {
        let (sender, _) = broadcast::channel(config.buffer_size);
        Self {
            sender,
            history: Arc::new(RwLock::new(Vec::new())),
            config,
        }
    }

    /// Publish an event to all subscribers
    ///
    /// Returns the number of active subscribers that received the event.
    pub async fn publish(&self, event: AlephEvent) -> usize {
        let timestamped = TimestampedEvent::new(event);

        trace!(
            event_type = ?timestamped.event.event_type(),
            sequence = timestamped.sequence,
            "Publishing event"
        );

        // Store in history if enabled
        if self.config.enable_history {
            let mut history = self.history.write().await;
            history.push(timestamped.clone());

            // Trim history if too large
            if history.len() > self.config.max_history_size {
                let drain_count = history.len() - self.config.max_history_size;
                history.drain(0..drain_count);
                debug!(
                    drain_count,
                    "Trimmed event history"
                );
            }
        }

        // Send to subscribers
        match self.sender.send(timestamped) {
            Ok(count) => {
                trace!(subscriber_count = count, "Event delivered");
                count
            }
            Err(_) => {
                // No subscribers - this is not an error
                trace!("No subscribers for event");
                0
            }
        }
    }

    /// Subscribe to all events
    pub fn subscribe(&self) -> EventSubscriber {
        EventSubscriber {
            receiver: self.sender.subscribe(),
            filter: vec![EventType::All],
        }
    }

    /// Subscribe to specific event types
    pub fn subscribe_filtered(&self, event_types: Vec<EventType>) -> EventSubscriber {
        EventSubscriber {
            receiver: self.sender.subscribe(),
            filter: event_types,
        }
    }

    /// Get the current number of subscribers
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Get event history
    pub async fn history(&self) -> Vec<TimestampedEvent> {
        self.history.read().await.clone()
    }

    /// Get history since a specific sequence number
    pub async fn history_since(&self, since_sequence: u64) -> Vec<TimestampedEvent> {
        self.history
            .read()
            .await
            .iter()
            .filter(|e| e.sequence > since_sequence)
            .cloned()
            .collect()
    }

    /// Clear event history
    pub async fn clear_history(&self) {
        self.history.write().await.clear();
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSubscriber {
    /// Receive the next event
    ///
    /// Blocks until an event is available or the channel is closed.
    /// If filtering is enabled, only matching events are returned.
    pub async fn recv(&mut self) -> Result<TimestampedEvent, EventBusError> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    if self.matches(&event) {
                        return Ok(event);
                    }
                    // Event doesn't match filter, continue waiting
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(EventBusError::ChannelClosed);
                }
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    warn!(
                        lagged_count = count,
                        "Subscriber lagged behind, some events were missed"
                    );
                    // Continue receiving
                }
            }
        }
    }

    /// Try to receive an event without blocking
    pub fn try_recv(&mut self) -> Result<Option<TimestampedEvent>, EventBusError> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => {
                    if self.matches(&event) {
                        return Ok(Some(event));
                    }
                    // Event doesn't match filter, try next
                }
                Err(broadcast::error::TryRecvError::Empty) => {
                    return Ok(None);
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    return Err(EventBusError::ChannelClosed);
                }
                Err(broadcast::error::TryRecvError::Lagged(count)) => {
                    warn!(
                        lagged_count = count,
                        "Subscriber lagged behind, some events were missed"
                    );
                    // Continue receiving
                }
            }
        }
    }

    /// Check if event matches the filter
    fn matches(&self, event: &TimestampedEvent) -> bool {
        if self.filter.contains(&EventType::All) {
            return true;
        }
        self.filter.contains(&event.event.event_type())
    }
}

/// Error type for event bus operations
#[derive(Debug, thiserror::Error)]
pub enum EventBusError {
    #[error("Event channel is closed")]
    ChannelClosed,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::types::{InputEvent, StopReason};

    #[tokio::test]
    async fn test_publish_and_subscribe() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe();

        let event = AlephEvent::InputReceived(InputEvent {
            text: "hello".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        });

        // Publish in a separate task
        let bus_clone = bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            bus_clone.publish(event).await;
        });

        // Receive
        let received = subscriber.recv().await.unwrap();
        assert_eq!(received.event.event_type(), EventType::InputReceived);
    }

    #[tokio::test]
    async fn test_filtered_subscription() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe_filtered(vec![EventType::LoopStop]);

        // Publish non-matching event first
        bus.publish(AlephEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        })).await;

        // Publish matching event
        bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;

        // Should only receive LoopStop
        let received = subscriber.try_recv().unwrap();
        assert!(received.is_some());
        assert_eq!(received.unwrap().event.event_type(), EventType::LoopStop);
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::new();
        let mut sub1 = bus.subscribe();
        let mut sub2 = bus.subscribe();

        assert_eq!(bus.subscriber_count(), 2);

        bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;

        // Both should receive
        let r1 = sub1.try_recv().unwrap();
        let r2 = sub2.try_recv().unwrap();

        assert!(r1.is_some());
        assert!(r2.is_some());
    }

    #[tokio::test]
    async fn test_event_history() {
        let bus = EventBus::new();

        bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;
        bus.publish(AlephEvent::LoopStop(StopReason::UserAborted)).await;

        let history = bus.history().await;
        assert_eq!(history.len(), 2);

        let since = bus.history_since(history[0].sequence).await;
        assert_eq!(since.len(), 1);
    }

    #[tokio::test]
    async fn test_history_trimming() {
        let bus = EventBus::with_config(EventBusConfig {
            buffer_size: 16,
            enable_history: true,
            max_history_size: 5,
        });

        for _ in 0..10 {
            bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;
        }

        let history = bus.history().await;
        assert_eq!(history.len(), 5);
    }
}
```

**Step 2: Run cargo check**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo check 2>&1 | head -30`
Expected: Still error about missing handler.rs

**Step 3: Run bus tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test event::bus --no-fail-fast 2>&1 | tail -30`
Expected: Tests should pass once handler.rs is created

**Step 4: Commit bus module**

```bash
git add Aleph/core/src/event/bus.rs
git commit -m "feat(event): implement EventBus with broadcast channels

- Add EventBus using tokio broadcast for multi-subscriber support
- Add EventSubscriber with filtered subscription capability
- Add event history tracking with configurable limits
- Include comprehensive unit tests"
```

---

## Task 4: Implement EventHandler Trait

**Files:**
- Create: `Aleph/core/src/event/handler.rs`

**Step 1: Write EventHandler trait and registry**

```rust
// Aleph/core/src/event/handler.rs
//! Event handler trait and registry for component subscriptions.

use crate::event::bus::EventBus;
use crate::event::types::{AlephEvent, EventType};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace};

/// Context provided to event handlers
#[derive(Clone)]
pub struct EventContext {
    /// Event bus for publishing new events
    pub bus: EventBus,
    /// Abort signal for graceful shutdown
    pub abort_signal: Arc<AtomicBool>,
    /// Session ID for the current execution
    pub session_id: Arc<RwLock<Option<String>>>,
}

impl EventContext {
    /// Create a new event context
    pub fn new(bus: EventBus) -> Self {
        Self {
            bus,
            abort_signal: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(RwLock::new(None)),
        }
    }

    /// Check if abort has been signaled
    pub fn is_aborted(&self) -> bool {
        self.abort_signal.load(Ordering::Relaxed)
    }

    /// Signal abort
    pub fn abort(&self) {
        self.abort_signal.store(true, Ordering::Relaxed);
    }

    /// Reset abort signal
    pub fn reset_abort(&self) {
        self.abort_signal.store(false, Ordering::Relaxed);
    }

    /// Set current session ID
    pub async fn set_session_id(&self, session_id: String) {
        *self.session_id.write().await = Some(session_id);
    }

    /// Get current session ID
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }
}

/// Trait for event handlers
///
/// Components implement this trait to receive and process events.
/// Each handler declares which events it subscribes to and how to handle them.
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Get the handler's unique name (for logging/debugging)
    fn name(&self) -> &'static str;

    /// Get the list of event types this handler subscribes to
    fn subscriptions(&self) -> Vec<EventType>;

    /// Handle an event
    ///
    /// Returns a list of new events to publish (can be empty).
    /// Errors are logged but don't stop the event loop.
    async fn handle(
        &self,
        event: &AlephEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError>;
}

/// Error type for event handlers
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("Handler error: {message}")]
    Generic { message: String },

    #[error("Aborted by user")]
    Aborted,

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Registry for managing event handlers
pub struct EventHandlerRegistry {
    handlers: Vec<Arc<dyn EventHandler>>,
    running: AtomicBool,
}

impl EventHandlerRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
            running: AtomicBool::new(false),
        }
    }

    /// Register a handler
    pub fn register(&mut self, handler: Arc<dyn EventHandler>) {
        info!(
            handler_name = handler.name(),
            subscriptions = ?handler.subscriptions(),
            "Registering event handler"
        );
        self.handlers.push(handler);
    }

    /// Get the number of registered handlers
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the registry is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Start all handlers listening for events
    ///
    /// This spawns a tokio task for each handler that listens for events
    /// and dispatches them to the handler.
    pub async fn start(&self, ctx: EventContext) -> Vec<tokio::task::JoinHandle<()>> {
        if self.running.swap(true, Ordering::SeqCst) {
            debug!("Registry already running");
            return vec![];
        }

        info!(
            handler_count = self.handlers.len(),
            "Starting event handler registry"
        );

        let mut handles = Vec::new();

        for handler in &self.handlers {
            let handler = Arc::clone(handler);
            let ctx = ctx.clone();
            let subscriptions = handler.subscriptions();

            let mut subscriber = if subscriptions.contains(&EventType::All) {
                ctx.bus.subscribe()
            } else {
                ctx.bus.subscribe_filtered(subscriptions)
            };

            let handle = tokio::spawn(async move {
                let handler_name = handler.name();
                debug!(handler_name, "Handler event loop started");

                loop {
                    // Check abort signal
                    if ctx.is_aborted() {
                        debug!(handler_name, "Handler received abort signal");
                        break;
                    }

                    match subscriber.recv().await {
                        Ok(timestamped_event) => {
                            trace!(
                                handler_name,
                                event_type = ?timestamped_event.event.event_type(),
                                "Handler received event"
                            );

                            // Handle the event
                            match handler.handle(&timestamped_event.event, &ctx).await {
                                Ok(new_events) => {
                                    // Publish any new events
                                    for new_event in new_events {
                                        trace!(
                                            handler_name,
                                            new_event_type = ?new_event.event_type(),
                                            "Handler publishing new event"
                                        );
                                        ctx.bus.publish(new_event).await;
                                    }
                                }
                                Err(HandlerError::Aborted) => {
                                    debug!(handler_name, "Handler aborted");
                                    break;
                                }
                                Err(e) => {
                                    error!(
                                        handler_name,
                                        error = %e,
                                        "Handler error"
                                    );
                                    // Continue processing other events
                                }
                            }
                        }
                        Err(e) => {
                            error!(
                                handler_name,
                                error = %e,
                                "Handler receive error, stopping"
                            );
                            break;
                        }
                    }
                }

                debug!(handler_name, "Handler event loop ended");
            });

            handles.push(handle);
        }

        handles
    }

    /// Stop all handlers
    pub fn stop(&self, ctx: &EventContext) {
        info!("Stopping event handler registry");
        ctx.abort();
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Default for EventHandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::types::{InputEvent, StopReason};
    use std::sync::atomic::AtomicUsize;

    /// Test handler that counts events
    struct CountingHandler {
        count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EventHandler for CountingHandler {
        fn name(&self) -> &'static str {
            "CountingHandler"
        }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::All]
        }

        async fn handle(
            &self,
            _event: &AlephEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AlephEvent>, HandlerError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        }
    }

    /// Test handler that produces new events
    struct ProducingHandler;

    #[async_trait]
    impl EventHandler for ProducingHandler {
        fn name(&self) -> &'static str {
            "ProducingHandler"
        }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::InputReceived]
        }

        async fn handle(
            &self,
            _event: &AlephEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AlephEvent>, HandlerError> {
            // Produce a LoopStop event for each input
            Ok(vec![AlephEvent::LoopStop(StopReason::Completed)])
        }
    }

    #[tokio::test]
    async fn test_handler_registration() {
        let mut registry = EventHandlerRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));

        registry.register(Arc::new(CountingHandler { count: counter.clone() }));

        assert_eq!(registry.handler_count(), 1);
    }

    #[tokio::test]
    async fn test_handler_receives_events() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        let mut registry = EventHandlerRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));

        registry.register(Arc::new(CountingHandler { count: counter.clone() }));

        let handles = registry.start(ctx.clone()).await;

        // Give handlers time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish event
        bus.publish(AlephEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        })).await;

        // Give handler time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Stop and wait
        registry.stop(&ctx);
        for handle in handles {
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                handle
            ).await;
        }

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_handler_produces_events() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        let mut registry = EventHandlerRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));

        // Register producing handler first, then counting handler
        registry.register(Arc::new(ProducingHandler));
        registry.register(Arc::new(CountingHandler { count: counter.clone() }));

        let handles = registry.start(ctx.clone()).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish input event
        bus.publish(AlephEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        })).await;

        // Give handlers time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        registry.stop(&ctx);
        for handle in handles {
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                handle
            ).await;
        }

        // CountingHandler should have received: InputReceived + LoopStop
        assert!(counter.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_event_context_abort() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        assert!(!ctx.is_aborted());

        ctx.abort();

        assert!(ctx.is_aborted());

        ctx.reset_abort();

        assert!(!ctx.is_aborted());
    }

    #[tokio::test]
    async fn test_event_context_session_id() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        assert!(ctx.get_session_id().await.is_none());

        ctx.set_session_id("session-123".to_string()).await;

        assert_eq!(ctx.get_session_id().await, Some("session-123".to_string()));
    }
}
```

**Step 2: Run cargo check**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo check 2>&1 | head -30`
Expected: Should compile (may have warnings)

**Step 3: Run all event module tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test event:: --no-fail-fast 2>&1`
Expected: All tests pass

**Step 4: Commit handler module**

```bash
git add Aleph/core/src/event/handler.rs
git commit -m "feat(event): implement EventHandler trait and registry

- Add EventHandler trait for component subscriptions
- Add EventContext for shared state (abort signal, session ID)
- Add EventHandlerRegistry for managing multiple handlers
- Add HandlerError for error handling
- Include comprehensive unit tests for handler lifecycle"
```

---

## Task 5: Integrate Event Module into lib.rs

**Files:**
- Modify: `Aleph/core/src/lib.rs`

**Step 1: Add event module declaration**

Add to `lib.rs` after line 67 (after `mod error;`):

```rust
pub mod event; // NEW: Event-driven architecture for agentic loop
```

**Step 2: Add re-exports**

Add after the existing re-exports section (around line 280):

```rust
// Event system exports (event-driven agentic loop)
pub use crate::event::{
    AlephEvent, EventBus, EventBusConfig, EventContext, EventHandler,
    EventHandlerRegistry, EventSubscriber, EventType, HandlerError, TimestampedEvent,
    // Event payload types
    AiResponse, CompactionInfo, ErrorKind, InputContext, InputEvent, LoopState,
    PlanRequest, PlanStep, SessionDiff, SessionInfo, StepStatus, StopReason,
    SubAgentRequest, SubAgentResult, TaskPlan, TokenUsage, ToolCallError,
    ToolCallRequest, ToolCallResult, ToolCallRetry, ToolCallStarted,
    UserQuestion, UserResponse,
};
```

**Step 3: Run cargo check**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo check 2>&1 | head -20`
Expected: Clean compile

**Step 4: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test event:: 2>&1`
Expected: All tests pass

**Step 5: Commit integration**

```bash
git add Aleph/core/src/lib.rs Aleph/core/src/event/mod.rs
git commit -m "feat(event): integrate event module into library

- Add event module declaration to lib.rs
- Export all event types and traits
- Complete Phase 1 event infrastructure"
```

---

## Task 6: Add Integration Test

**Files:**
- Create: `Aleph/core/src/event/integration_test.rs`
- Modify: `Aleph/core/src/event/mod.rs`

**Step 1: Create integration test file**

```rust
// Aleph/core/src/event/integration_test.rs
//! Integration tests for the event system.

#[cfg(test)]
mod tests {
    use crate::event::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use async_trait::async_trait;

    /// Simulates IntentAnalyzer: receives InputReceived, publishes ToolCallRequested
    struct MockIntentAnalyzer;

    #[async_trait]
    impl EventHandler for MockIntentAnalyzer {
        fn name(&self) -> &'static str { "MockIntentAnalyzer" }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::InputReceived]
        }

        async fn handle(
            &self,
            event: &AlephEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AlephEvent>, HandlerError> {
            if let AlephEvent::InputReceived(input) = event {
                // Simulate: detect intent and request tool call
                Ok(vec![AlephEvent::ToolCallRequested(ToolCallRequest {
                    tool: "search".to_string(),
                    parameters: serde_json::json!({"query": input.text}),
                    plan_step_id: None,
                })])
            } else {
                Ok(vec![])
            }
        }
    }

    /// Simulates ToolExecutor: receives ToolCallRequested, publishes ToolCallCompleted
    struct MockToolExecutor;

    #[async_trait]
    impl EventHandler for MockToolExecutor {
        fn name(&self) -> &'static str { "MockToolExecutor" }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::ToolCallRequested]
        }

        async fn handle(
            &self,
            event: &AlephEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AlephEvent>, HandlerError> {
            if let AlephEvent::ToolCallRequested(req) = event {
                Ok(vec![AlephEvent::ToolCallCompleted(ToolCallResult {
                    call_id: uuid::Uuid::new_v4().to_string(),
                    tool: req.tool.clone(),
                    input: req.parameters.clone(),
                    output: "search results".to_string(),
                    started_at: chrono::Utc::now().timestamp_millis(),
                    completed_at: chrono::Utc::now().timestamp_millis(),
                    token_usage: TokenUsage::default(),
                })])
            } else {
                Ok(vec![])
            }
        }
    }

    /// Simulates LoopController: receives ToolCallCompleted, publishes LoopStop
    struct MockLoopController {
        iterations: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EventHandler for MockLoopController {
        fn name(&self) -> &'static str { "MockLoopController" }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::ToolCallCompleted]
        }

        async fn handle(
            &self,
            _event: &AlephEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AlephEvent>, HandlerError> {
            let count = self.iterations.fetch_add(1, Ordering::SeqCst);

            // Stop after first iteration
            if count >= 1 {
                Ok(vec![AlephEvent::LoopStop(StopReason::Completed)])
            } else {
                Ok(vec![AlephEvent::LoopContinue(LoopState {
                    session_id: "test-session".to_string(),
                    iteration: count as u32,
                    total_tokens: 0,
                    last_tool: Some("search".to_string()),
                })])
            }
        }
    }

    /// Test the complete event flow
    #[tokio::test]
    async fn test_complete_event_flow() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        let iterations = Arc::new(AtomicUsize::new(0));

        let mut registry = EventHandlerRegistry::new();
        registry.register(Arc::new(MockIntentAnalyzer));
        registry.register(Arc::new(MockToolExecutor));
        registry.register(Arc::new(MockLoopController {
            iterations: iterations.clone()
        }));

        // Subscribe to watch for LoopStop
        let mut watcher = bus.subscribe_filtered(vec![EventType::LoopStop]);

        // Start handlers
        let handles = registry.start(ctx.clone()).await;

        // Give handlers time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        // Trigger the flow with an input event
        bus.publish(AlephEvent::InputReceived(InputEvent {
            text: "search for rust async".to_string(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        })).await;

        // Wait for LoopStop with timeout
        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            watcher.recv()
        ).await;

        assert!(result.is_ok(), "Should receive LoopStop event");
        let event = result.unwrap().unwrap();
        assert_eq!(event.event.event_type(), EventType::LoopStop);

        // Verify the flow executed
        assert!(iterations.load(Ordering::SeqCst) >= 1);

        // Check history
        let history = bus.history().await;
        assert!(history.len() >= 4, "Should have at least 4 events in history");

        // Verify event sequence: Input -> ToolCallRequested -> ToolCallCompleted -> LoopStop
        let event_types: Vec<_> = history.iter()
            .map(|e| e.event.event_type())
            .collect();

        assert!(event_types.contains(&EventType::InputReceived));
        assert!(event_types.contains(&EventType::ToolCallRequested));
        assert!(event_types.contains(&EventType::ToolCallCompleted));

        // Cleanup
        registry.stop(&ctx);
        for handle in handles {
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                handle
            ).await;
        }
    }

    /// Test abort signal propagation
    #[tokio::test]
    async fn test_abort_stops_handlers() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        let counter = Arc::new(AtomicUsize::new(0));

        struct SlowHandler {
            counter: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl EventHandler for SlowHandler {
            fn name(&self) -> &'static str { "SlowHandler" }

            fn subscriptions(&self) -> Vec<EventType> {
                vec![EventType::All]
            }

            async fn handle(
                &self,
                _event: &AlephEvent,
                ctx: &EventContext,
            ) -> Result<Vec<AlephEvent>, HandlerError> {
                if ctx.is_aborted() {
                    return Err(HandlerError::Aborted);
                }
                self.counter.fetch_add(1, Ordering::SeqCst);
                Ok(vec![])
            }
        }

        let mut registry = EventHandlerRegistry::new();
        registry.register(Arc::new(SlowHandler { counter: counter.clone() }));

        let handles = registry.start(ctx.clone()).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish one event
        bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let count_before = counter.load(Ordering::SeqCst);

        // Abort
        registry.stop(&ctx);

        // Wait for handlers to stop
        for handle in handles {
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(200),
                handle
            ).await;
        }

        // Verify handler processed at least one event before abort
        assert!(count_before >= 1);
    }
}
```

**Step 2: Add integration test module to mod.rs**

Add to `event/mod.rs`:

```rust
#[cfg(test)]
mod integration_test;
```

**Step 3: Run integration tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test event::integration_test --no-fail-fast 2>&1`
Expected: All tests pass

**Step 4: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test 2>&1 | tail -30`
Expected: All tests pass

**Step 5: Commit integration test**

```bash
git add Aleph/core/src/event/integration_test.rs Aleph/core/src/event/mod.rs
git commit -m "test(event): add integration tests for event flow

- Add MockIntentAnalyzer, MockToolExecutor, MockLoopController
- Test complete event flow: Input -> Tool -> Complete
- Test abort signal propagation
- Verify event history tracking"
```

---

## Task 7: Final Verification and Documentation

**Step 1: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test 2>&1 | tail -50`
Expected: All tests pass

**Step 2: Run clippy**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo clippy --all-targets 2>&1 | grep -E "(warning|error)" | head -20`
Expected: No new warnings in event module

**Step 3: Build release to verify**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo build --release 2>&1 | tail -10`
Expected: Build succeeds

**Step 4: Verify library exports**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo doc --no-deps 2>&1 | tail -10`
Expected: Documentation builds

**Step 5: Final commit**

```bash
git add -A
git commit -m "docs(event): Phase 1 complete - event infrastructure ready

Event-driven architecture foundation:
- EventBus: Type-safe broadcast channels with history
- AlephEvent: Unified event enum with 19 variants
- EventHandler: Trait for component subscriptions
- EventHandlerRegistry: Lifecycle management

Ready for Phase 2: Core components implementation"
```

---

## Summary

| Task | Files | Description |
|------|-------|-------------|
| 1 | `event/mod.rs` | Module structure |
| 2 | `event/types.rs` | Event type definitions |
| 3 | `event/bus.rs` | EventBus with broadcast |
| 4 | `event/handler.rs` | EventHandler trait |
| 5 | `lib.rs` | Library integration |
| 6 | `event/integration_test.rs` | Integration tests |
| 7 | - | Final verification |

**Total estimated time:** 1-2 hours for implementation + testing
