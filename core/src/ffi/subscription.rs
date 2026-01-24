//! FFI Subscription Interface for Swift Event Subscriptions
//!
//! This module provides UniFFI-compatible functions for Swift to dynamically
//! subscribe to GlobalBus events. It enables the Swift UI layer to receive
//! real-time event notifications from the Rust core.
//!
//! # Architecture
//!
//! ```text
//! Swift UI <-- EventSubscriptionHandler <-- GlobalBus
//! ```
//!
//! # Usage
//!
//! ```swift
//! // Swift side
//! class MyEventHandler: EventSubscriptionHandler {
//!     func onEvent(eventJson: String) {
//!         // Parse JSON and handle event
//!     }
//! }
//!
//! let handler = MyEventHandler()
//! let subId = subscribeEvents(
//!     sessionId: "session-123",
//!     eventTypes: ["ToolCallStarted", "LoopStop"],
//!     handler: handler
//! )
//!
//! // Later: unsubscribe
//! unsubscribeEvents(subscriptionId: subId)
//! ```

use crate::event::filter::EventFilter;
use crate::event::global_bus::GlobalBus;
use crate::event::EventType;
use std::sync::Arc;
use tracing::{debug, warn};

// =============================================================================
// Callback Interface
// =============================================================================

/// Subscription callback trait for Swift event handling.
///
/// Swift clients implement this trait to receive GlobalBus events.
/// Events are serialized to JSON for FFI boundary crossing.
#[uniffi::export(callback_interface)]
pub trait EventSubscriptionHandler: Send + Sync {
    /// Called when a matching event is received.
    ///
    /// # Arguments
    ///
    /// * `event_json` - JSON serialized GlobalEvent
    fn on_event(&self, event_json: String);
}

// =============================================================================
// Public Functions
// =============================================================================

/// Create a subscription to GlobalBus events.
///
/// Subscribes to events matching the specified filters and invokes the handler
/// callback when events are received.
///
/// # Arguments
///
/// * `session_id` - Optional session ID to filter events (None = all sessions)
/// * `event_types` - List of event type names to subscribe to (e.g., ["ToolCallStarted", "LoopStop"])
/// * `handler` - Callback handler implementing EventSubscriptionHandler
///
/// # Returns
///
/// A unique subscription ID that can be used to unsubscribe later.
///
/// # Event Type Names
///
/// Valid event type names:
/// - "InputReceived", "PlanRequested", "PlanCreated"
/// - "ToolCallRequested", "ToolCallStarted", "ToolCallCompleted", "ToolCallFailed", "ToolCallRetrying"
/// - "LoopContinue", "LoopStop"
/// - "SessionCreated", "SessionUpdated", "SessionResumed", "SessionCompacted"
/// - "SubAgentStarted", "SubAgentCompleted"
/// - "UserQuestionAsked", "UserResponseReceived"
/// - "PermissionAsked", "PermissionReplied"
/// - "QuestionAsked", "QuestionReplied", "QuestionRejected"
/// - "AiResponseGenerated"
/// - "PartAdded", "PartUpdated", "PartRemoved"
/// - "All" (matches all event types)
#[uniffi::export]
pub fn subscribe_events(
    session_id: Option<String>,
    event_types: Vec<String>,
    handler: Box<dyn EventSubscriptionHandler>,
) -> String {
    // Wrap in Arc for use in callback closure
    subscribe_events_internal(session_id, event_types, Arc::from(handler))
}

/// Internal subscription function that accepts Arc<dyn EventSubscriptionHandler>.
///
/// This is used by the public FFI function.
fn subscribe_events_internal(
    session_id: Option<String>,
    event_types: Vec<String>,
    handler: Arc<dyn EventSubscriptionHandler>,
) -> String {
    let (filter, callback) = build_filter_and_callback(session_id.clone(), event_types, handler);

    debug!(
        session_id = ?session_id,
        event_count = filter.event_types.len(),
        "Creating GlobalBus subscription"
    );

    // Subscribe to GlobalBus (sync version - uses blocking_write)
    let subscription_id = GlobalBus::global().subscribe(filter, callback);

    debug!(
        subscription_id = %subscription_id,
        "GlobalBus subscription created"
    );

    subscription_id
}

/// Async version of internal subscription function for use in async contexts.
///
/// This is used by async tests to avoid blocking_write issues.
#[cfg(test)]
async fn subscribe_events_async(
    session_id: Option<String>,
    event_types: Vec<String>,
    handler: Arc<dyn EventSubscriptionHandler>,
) -> String {
    let (filter, callback) = build_filter_and_callback(session_id.clone(), event_types, handler);

    debug!(
        session_id = ?session_id,
        event_count = filter.event_types.len(),
        "Creating GlobalBus subscription (async)"
    );

    // Subscribe to GlobalBus (async version)
    let subscription_id = GlobalBus::global().subscribe_async(filter, callback).await;

    debug!(
        subscription_id = %subscription_id,
        "GlobalBus subscription created (async)"
    );

    subscription_id
}

/// Build the filter and callback for subscription.
///
/// Helper function shared by sync and async subscription functions.
fn build_filter_and_callback(
    session_id: Option<String>,
    event_types: Vec<String>,
    handler: Arc<dyn EventSubscriptionHandler>,
) -> (EventFilter, impl Fn(crate::event::global_bus::GlobalEvent) + Send + Sync + 'static) {
    // Parse event types from strings
    let parsed_types: Vec<EventType> = event_types
        .iter()
        .filter_map(|s| parse_event_type(s))
        .collect();

    // If no valid event types, default to All
    let event_types = if parsed_types.is_empty() {
        warn!("No valid event types provided, defaulting to All");
        vec![EventType::All]
    } else {
        parsed_types
    };

    // Build filter
    let mut filter = EventFilter::new(event_types);

    // Apply session filter if provided
    if let Some(ref session) = session_id {
        filter = filter.with_session(session);
    }

    // Create callback wrapper that serializes events to JSON
    let callback = move |event: crate::event::global_bus::GlobalEvent| {
        match serde_json::to_string(&event) {
            Ok(json) => {
                handler.on_event(json);
            }
            Err(e) => {
                warn!(error = %e, "Failed to serialize GlobalEvent to JSON");
            }
        }
    };

    (filter, callback)
}

/// Cancel a subscription to GlobalBus events.
///
/// # Arguments
///
/// * `subscription_id` - The subscription ID returned from `subscribe_events`
#[uniffi::export]
pub fn unsubscribe_events(subscription_id: String) {
    debug!(
        subscription_id = %subscription_id,
        "Cancelling GlobalBus subscription"
    );

    // Get or create a tokio runtime for the async unsubscribe call
    // Use the async version of unsubscribe via a blocking call
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // In async context - spawn a task to avoid blocking
            let sub_id = subscription_id.clone();
            handle.spawn(async move {
                GlobalBus::global().unsubscribe(&sub_id).await;
            });
        }
        Err(_) => {
            // Not in async context, create a new runtime for this call
            if let Ok(rt) = tokio::runtime::Runtime::new() {
                rt.block_on(async {
                    GlobalBus::global().unsubscribe(&subscription_id).await;
                });
            } else {
                warn!("Failed to create tokio runtime for unsubscribe");
            }
        }
    }

    debug!(
        subscription_id = %subscription_id,
        "GlobalBus subscription cancelled"
    );
}

/// Async version of unsubscribe for use in async contexts.
#[cfg(test)]
async fn unsubscribe_events_async(subscription_id: String) {
    debug!(
        subscription_id = %subscription_id,
        "Cancelling GlobalBus subscription (async)"
    );

    GlobalBus::global().unsubscribe(&subscription_id).await;

    debug!(
        subscription_id = %subscription_id,
        "GlobalBus subscription cancelled (async)"
    );
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Parse an event type string to EventType enum.
///
/// # Arguments
///
/// * `s` - Event type name string
///
/// # Returns
///
/// Some(EventType) if valid, None otherwise
fn parse_event_type(s: &str) -> Option<EventType> {
    match s {
        // Input
        "InputReceived" => Some(EventType::InputReceived),

        // Planning
        "PlanRequested" => Some(EventType::PlanRequested),
        "PlanCreated" => Some(EventType::PlanCreated),

        // Tool execution
        "ToolCallRequested" => Some(EventType::ToolCallRequested),
        "ToolCallStarted" => Some(EventType::ToolCallStarted),
        "ToolCallCompleted" => Some(EventType::ToolCallCompleted),
        "ToolCallFailed" => Some(EventType::ToolCallFailed),
        "ToolCallRetrying" => Some(EventType::ToolCallRetrying),

        // Loop control
        "LoopContinue" => Some(EventType::LoopContinue),
        "LoopStop" => Some(EventType::LoopStop),

        // Session
        "SessionCreated" => Some(EventType::SessionCreated),
        "SessionUpdated" => Some(EventType::SessionUpdated),
        "SessionResumed" => Some(EventType::SessionResumed),
        "SessionCompacted" => Some(EventType::SessionCompacted),

        // Sub-agent
        "SubAgentStarted" => Some(EventType::SubAgentStarted),
        "SubAgentCompleted" => Some(EventType::SubAgentCompleted),

        // User interaction (legacy)
        "UserQuestionAsked" => Some(EventType::UserQuestionAsked),
        "UserResponseReceived" => Some(EventType::UserResponseReceived),

        // Permission system
        "PermissionAsked" => Some(EventType::PermissionAsked),
        "PermissionReplied" => Some(EventType::PermissionReplied),

        // Question system
        "QuestionAsked" => Some(EventType::QuestionAsked),
        "QuestionReplied" => Some(EventType::QuestionReplied),
        "QuestionRejected" => Some(EventType::QuestionRejected),

        // AI response
        "AiResponseGenerated" => Some(EventType::AiResponseGenerated),

        // Part updates
        "PartAdded" => Some(EventType::PartAdded),
        "PartUpdated" => Some(EventType::PartUpdated),
        "PartRemoved" => Some(EventType::PartRemoved),

        // Wildcard
        "All" => Some(EventType::All),

        _ => {
            warn!(event_type = s, "Unknown event type");
            None
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_event_type_valid() {
        assert_eq!(parse_event_type("ToolCallStarted"), Some(EventType::ToolCallStarted));
        assert_eq!(parse_event_type("LoopStop"), Some(EventType::LoopStop));
        assert_eq!(parse_event_type("All"), Some(EventType::All));
        assert_eq!(parse_event_type("SessionCompacted"), Some(EventType::SessionCompacted));
        assert_eq!(parse_event_type("PartAdded"), Some(EventType::PartAdded));
    }

    #[test]
    fn test_parse_event_type_invalid() {
        assert_eq!(parse_event_type("InvalidEvent"), None);
        assert_eq!(parse_event_type(""), None);
        assert_eq!(parse_event_type("toolcallstarted"), None); // Case sensitive
    }

    #[test]
    fn test_parse_all_event_types() {
        // Verify all EventType variants have string mappings
        let event_types = vec![
            "InputReceived",
            "PlanRequested",
            "PlanCreated",
            "ToolCallRequested",
            "ToolCallStarted",
            "ToolCallCompleted",
            "ToolCallFailed",
            "ToolCallRetrying",
            "LoopContinue",
            "LoopStop",
            "SessionCreated",
            "SessionUpdated",
            "SessionResumed",
            "SessionCompacted",
            "SubAgentStarted",
            "SubAgentCompleted",
            "UserQuestionAsked",
            "UserResponseReceived",
            "PermissionAsked",
            "PermissionReplied",
            "QuestionAsked",
            "QuestionReplied",
            "QuestionRejected",
            "AiResponseGenerated",
            "PartAdded",
            "PartUpdated",
            "PartRemoved",
            "All",
        ];

        for event_type in event_types {
            assert!(
                parse_event_type(event_type).is_some(),
                "Event type '{}' should be valid",
                event_type
            );
        }
    }

    // Mock handler for testing
    struct MockHandler {
        events: std::sync::Mutex<Vec<String>>,
    }

    impl EventSubscriptionHandler for MockHandler {
        fn on_event(&self, event_json: String) {
            self.events.lock().unwrap().push(event_json);
        }
    }

    #[tokio::test]
    async fn test_subscribe_and_unsubscribe() {
        let handler = Arc::new(MockHandler {
            events: std::sync::Mutex::new(Vec::new()),
        });

        // Subscribe using async version for testing
        let sub_id = subscribe_events_async(
            Some("test-session".to_string()),
            vec!["ToolCallStarted".to_string(), "LoopStop".to_string()],
            handler.clone(),
        )
        .await;

        assert!(!sub_id.is_empty());

        // Unsubscribe using async version
        unsubscribe_events_async(sub_id.clone()).await;

        // Verify no panic - subscription count check is informational
        let _count = GlobalBus::global().subscription_count().await;
    }

    #[test]
    fn test_subscribe_with_empty_event_types() {
        let handler = Arc::new(MockHandler {
            events: std::sync::Mutex::new(Vec::new()),
        });

        // Subscribe with empty event types - should default to All
        let sub_id = subscribe_events_internal(None, vec![], handler.clone());

        assert!(!sub_id.is_empty());

        // Cleanup
        unsubscribe_events(sub_id);
    }

    #[test]
    fn test_subscribe_with_invalid_event_types() {
        let handler = Arc::new(MockHandler {
            events: std::sync::Mutex::new(Vec::new()),
        });

        // Subscribe with invalid event types - should default to All
        let sub_id = subscribe_events_internal(
            None,
            vec!["InvalidEvent".to_string(), "AnotherInvalid".to_string()],
            handler.clone(),
        );

        assert!(!sub_id.is_empty());

        // Cleanup
        unsubscribe_events(sub_id);
    }

    #[tokio::test]
    async fn test_event_delivery() {
        use crate::event::global_bus::GlobalBus;
        use crate::event::{AetherEvent, StopReason};

        let handler = Arc::new(MockHandler {
            events: std::sync::Mutex::new(Vec::new()),
        });

        // Subscribe to LoopStop events using async version
        let sub_id = subscribe_events_async(
            Some("delivery-test-session".to_string()),
            vec!["LoopStop".to_string()],
            handler.clone(),
        )
        .await;

        // Broadcast an event
        GlobalBus::global()
            .broadcast(
                "test-agent",
                "delivery-test-session",
                AetherEvent::LoopStop(StopReason::Completed),
            )
            .await;

        // Small delay to allow async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Check that event was received
        let events = handler.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("LoopStop"));
        assert!(events[0].contains("Completed"));

        // Cleanup
        drop(events);
        unsubscribe_events_async(sub_id).await;
    }

    #[tokio::test]
    async fn test_session_filter() {
        use crate::event::global_bus::GlobalBus;
        use crate::event::{AetherEvent, StopReason};

        let handler = Arc::new(MockHandler {
            events: std::sync::Mutex::new(Vec::new()),
        });

        // Subscribe to events from specific session using async version
        let sub_id = subscribe_events_async(
            Some("filter-test-session".to_string()),
            vec!["LoopStop".to_string()],
            handler.clone(),
        )
        .await;

        // Broadcast from matching session
        GlobalBus::global()
            .broadcast(
                "test-agent",
                "filter-test-session",
                AetherEvent::LoopStop(StopReason::Completed),
            )
            .await;

        // Broadcast from different session (should not match)
        GlobalBus::global()
            .broadcast(
                "test-agent",
                "other-session",
                AetherEvent::LoopStop(StopReason::UserAborted),
            )
            .await;

        // Small delay
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Only one event should be received
        let events = handler.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("Completed"));

        // Cleanup
        drop(events);
        unsubscribe_events_async(sub_id).await;
    }
}
