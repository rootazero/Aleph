// Aether/core/src/event/filter.rs
//! Event filtering for subscription-based event routing.
//!
//! The `EventFilter` provides flexible filtering capabilities for the GlobalBus,
//! allowing subscribers to filter events based on:
//! - Session ID (single or multiple)
//! - Agent ID (single or multiple)
//! - Event types (required)
//!
//! # Example
//!
//! ```rust
//! use aether_core::event::filter::EventFilter;
//! use aether_core::event::EventType;
//!
//! // Filter for all tool events from a specific session
//! let filter = EventFilter::new(vec![
//!     EventType::ToolCallStarted,
//!     EventType::ToolCallCompleted,
//!     EventType::ToolCallFailed,
//! ])
//! .with_session("session-123");
//!
//! // Filter for all events from specific agents
//! let filter = EventFilter::all()
//!     .with_agent("agent-1")
//!     .with_agent("agent-2");
//! ```

use crate::event::types::{AetherEvent, EventType};
use std::collections::HashSet;

// =============================================================================
// GlobalEvent (Temporary Definition)
// =============================================================================

/// Global event wrapper for cross-session event routing.
///
/// TODO: This is a minimal definition for EventFilter testing.
/// Will be replaced with the full GlobalEvent implementation in the next task
/// (Task #2: Create GlobalEvent wrapper with source tracking).
#[derive(Debug, Clone)]
pub struct GlobalEvent {
    /// The source session that emitted this event
    pub source_session_id: String,
    /// The source agent that emitted this event (if applicable)
    pub source_agent_id: Option<String>,
    /// The actual event payload
    pub event: AetherEvent,
    /// Timestamp when the event was emitted (epoch millis)
    pub timestamp: i64,
    /// Monotonic sequence number for ordering
    pub sequence: u64,
}

impl GlobalEvent {
    /// Create a new GlobalEvent (for testing purposes)
    #[cfg(test)]
    pub fn new(
        source_session_id: impl Into<String>,
        source_agent_id: Option<String>,
        event: AetherEvent,
    ) -> Self {
        Self {
            source_session_id: source_session_id.into(),
            source_agent_id,
            event,
            timestamp: chrono::Utc::now().timestamp_millis(),
            sequence: 0,
        }
    }
}

// =============================================================================
// EventFilter
// =============================================================================

/// Filter for subscription-based event routing.
///
/// Allows filtering events by:
/// - Session IDs: `None` means all sessions, `Some(set)` means only those sessions
/// - Agent IDs: `None` means all agents, `Some(set)` means only those agents
/// - Event types: Required, events must match one of the specified types
///
/// All conditions must pass for an event to match (AND logic between categories,
/// OR logic within each category).
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Session IDs to filter on. `None` = all sessions.
    pub session_ids: Option<HashSet<String>>,
    /// Agent IDs to filter on. `None` = all agents.
    pub agent_ids: Option<HashSet<String>>,
    /// Event types to filter on. Must be non-empty.
    pub event_types: Vec<EventType>,
}

impl EventFilter {
    /// Create a new filter with specified event types.
    ///
    /// # Arguments
    ///
    /// * `event_types` - The event types to filter on. An empty vector means no events match.
    ///
    /// # Example
    ///
    /// ```rust
    /// use aether_core::event::filter::EventFilter;
    /// use aether_core::event::EventType;
    ///
    /// let filter = EventFilter::new(vec![EventType::ToolCallCompleted]);
    /// ```
    pub fn new(event_types: Vec<EventType>) -> Self {
        Self {
            session_ids: None,
            agent_ids: None,
            event_types,
        }
    }

    /// Create a filter that matches all events.
    ///
    /// Equivalent to `EventFilter::new(vec![EventType::All])`.
    pub fn all() -> Self {
        Self::new(vec![EventType::All])
    }

    /// Add a single session ID to filter on.
    ///
    /// If this is the first session added, creates a new filter set.
    /// Otherwise, adds to the existing set.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to include
    pub fn with_session(mut self, session_id: &str) -> Self {
        match &mut self.session_ids {
            Some(ids) => {
                ids.insert(session_id.to_string());
            }
            None => {
                let mut set = HashSet::new();
                set.insert(session_id.to_string());
                self.session_ids = Some(set);
            }
        }
        self
    }

    /// Set the session IDs to filter on, replacing any existing filter.
    ///
    /// # Arguments
    ///
    /// * `session_ids` - The set of session IDs to include
    pub fn with_sessions(mut self, session_ids: HashSet<String>) -> Self {
        self.session_ids = Some(session_ids);
        self
    }

    /// Add a single agent ID to filter on.
    ///
    /// If this is the first agent added, creates a new filter set.
    /// Otherwise, adds to the existing set.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - The agent ID to include
    pub fn with_agent(mut self, agent_id: &str) -> Self {
        match &mut self.agent_ids {
            Some(ids) => {
                ids.insert(agent_id.to_string());
            }
            None => {
                let mut set = HashSet::new();
                set.insert(agent_id.to_string());
                self.agent_ids = Some(set);
            }
        }
        self
    }

    /// Set the agent IDs to filter on, replacing any existing filter.
    ///
    /// # Arguments
    ///
    /// * `agent_ids` - The set of agent IDs to include
    pub fn with_agents(mut self, agent_ids: HashSet<String>) -> Self {
        self.agent_ids = Some(agent_ids);
        self
    }

    /// Check if an event matches this filter.
    ///
    /// Returns `true` if ALL of the following conditions are met:
    /// 1. Session filter passes (no filter OR event session is in the set)
    /// 2. Agent filter passes (no filter OR event agent is in the set OR event has no agent)
    /// 3. Event type filter passes (event_types contains EventType::All OR event type is in the list)
    ///
    /// # Arguments
    ///
    /// * `event` - The global event to check against this filter
    pub fn matches(&self, event: &GlobalEvent) -> bool {
        // Check session filter
        if let Some(ref session_ids) = self.session_ids {
            if !session_ids.contains(&event.source_session_id) {
                return false;
            }
        }

        // Check agent filter
        if let Some(ref agent_ids) = self.agent_ids {
            match &event.source_agent_id {
                Some(agent_id) => {
                    if !agent_ids.contains(agent_id) {
                        return false;
                    }
                }
                // If event has no agent_id but filter requires specific agents, it doesn't match
                None => return false,
            }
        }

        // Check event type filter
        if self.event_types.is_empty() {
            return false;
        }

        if self.event_types.contains(&EventType::All) {
            return true;
        }

        self.event_types.contains(&event.event.event_type())
    }

    /// Check if this filter has any session restrictions.
    pub fn has_session_filter(&self) -> bool {
        self.session_ids.is_some()
    }

    /// Check if this filter has any agent restrictions.
    pub fn has_agent_filter(&self) -> bool {
        self.agent_ids.is_some()
    }

    /// Check if this filter matches all event types.
    pub fn matches_all_types(&self) -> bool {
        self.event_types.contains(&EventType::All)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::types::{InputEvent, StopReason};

    fn make_input_event() -> AetherEvent {
        AetherEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        })
    }

    fn make_loop_stop_event() -> AetherEvent {
        AetherEvent::LoopStop(StopReason::Completed)
    }

    #[test]
    fn test_filter_no_session_agent_filter_matches_all() {
        // Filter with no session/agent restrictions should match all sessions/agents
        let filter = EventFilter::new(vec![EventType::InputReceived]);

        let event = GlobalEvent::new("session-1", Some("agent-1".to_string()), make_input_event());
        assert!(filter.matches(&event));

        let event = GlobalEvent::new("session-2", Some("agent-2".to_string()), make_input_event());
        assert!(filter.matches(&event));

        let event = GlobalEvent::new("any-session", None, make_input_event());
        assert!(filter.matches(&event));
    }

    #[test]
    fn test_filter_with_specific_session() {
        let filter = EventFilter::new(vec![EventType::InputReceived]).with_session("session-1");

        // Matching session
        let event = GlobalEvent::new("session-1", None, make_input_event());
        assert!(filter.matches(&event));

        // Non-matching session
        let event = GlobalEvent::new("session-2", None, make_input_event());
        assert!(!filter.matches(&event));
    }

    #[test]
    fn test_filter_with_multiple_sessions() {
        let mut sessions = HashSet::new();
        sessions.insert("session-1".to_string());
        sessions.insert("session-2".to_string());

        let filter = EventFilter::new(vec![EventType::InputReceived]).with_sessions(sessions);

        assert!(filter.matches(&GlobalEvent::new("session-1", None, make_input_event())));
        assert!(filter.matches(&GlobalEvent::new("session-2", None, make_input_event())));
        assert!(!filter.matches(&GlobalEvent::new("session-3", None, make_input_event())));
    }

    #[test]
    fn test_filter_with_specific_agent() {
        let filter = EventFilter::new(vec![EventType::InputReceived]).with_agent("agent-1");

        // Matching agent
        let event = GlobalEvent::new("session-1", Some("agent-1".to_string()), make_input_event());
        assert!(filter.matches(&event));

        // Non-matching agent
        let event = GlobalEvent::new("session-1", Some("agent-2".to_string()), make_input_event());
        assert!(!filter.matches(&event));

        // No agent specified in event (filter requires specific agent)
        let event = GlobalEvent::new("session-1", None, make_input_event());
        assert!(!filter.matches(&event));
    }

    #[test]
    fn test_filter_with_multiple_agents() {
        let mut agents = HashSet::new();
        agents.insert("agent-1".to_string());
        agents.insert("agent-2".to_string());

        let filter = EventFilter::new(vec![EventType::InputReceived]).with_agents(agents);

        assert!(filter.matches(&GlobalEvent::new(
            "session-1",
            Some("agent-1".to_string()),
            make_input_event()
        )));
        assert!(filter.matches(&GlobalEvent::new(
            "session-1",
            Some("agent-2".to_string()),
            make_input_event()
        )));
        assert!(!filter.matches(&GlobalEvent::new(
            "session-1",
            Some("agent-3".to_string()),
            make_input_event()
        )));
    }

    #[test]
    fn test_filter_event_type_matching() {
        let filter = EventFilter::new(vec![EventType::InputReceived, EventType::LoopStop]);

        // Matching event types
        assert!(filter.matches(&GlobalEvent::new("s1", None, make_input_event())));
        assert!(filter.matches(&GlobalEvent::new("s1", None, make_loop_stop_event())));

        // Non-matching event type
        let plan_event = AetherEvent::PlanRequested(crate::event::types::PlanRequest {
            input: InputEvent {
                text: "test".to_string(),
                topic_id: None,
                context: None,
                timestamp: 0,
            },
            intent_type: None,
            detected_steps: vec![],
        });
        assert!(!filter.matches(&GlobalEvent::new("s1", None, plan_event)));
    }

    #[test]
    fn test_filter_event_type_all_matches_everything() {
        let filter = EventFilter::all();

        assert!(filter.matches(&GlobalEvent::new("s1", None, make_input_event())));
        assert!(filter.matches(&GlobalEvent::new("s1", None, make_loop_stop_event())));

        let plan_event = AetherEvent::PlanRequested(crate::event::types::PlanRequest {
            input: InputEvent {
                text: "test".to_string(),
                topic_id: None,
                context: None,
                timestamp: 0,
            },
            intent_type: None,
            detected_steps: vec![],
        });
        assert!(filter.matches(&GlobalEvent::new("s1", None, plan_event)));
    }

    #[test]
    fn test_filter_empty_event_types_matches_nothing() {
        let filter = EventFilter::new(vec![]);

        assert!(!filter.matches(&GlobalEvent::new("s1", None, make_input_event())));
        assert!(!filter.matches(&GlobalEvent::new("s1", None, make_loop_stop_event())));
    }

    #[test]
    fn test_filter_combined_session_and_agent() {
        let filter = EventFilter::new(vec![EventType::InputReceived])
            .with_session("session-1")
            .with_agent("agent-1");

        // Both match
        assert!(filter.matches(&GlobalEvent::new(
            "session-1",
            Some("agent-1".to_string()),
            make_input_event()
        )));

        // Session matches, agent doesn't
        assert!(!filter.matches(&GlobalEvent::new(
            "session-1",
            Some("agent-2".to_string()),
            make_input_event()
        )));

        // Agent matches, session doesn't
        assert!(!filter.matches(&GlobalEvent::new(
            "session-2",
            Some("agent-1".to_string()),
            make_input_event()
        )));

        // Neither matches
        assert!(!filter.matches(&GlobalEvent::new(
            "session-2",
            Some("agent-2".to_string()),
            make_input_event()
        )));
    }

    #[test]
    fn test_filter_builder_chaining() {
        // Test that builder methods can be chained
        let filter = EventFilter::new(vec![EventType::All])
            .with_session("s1")
            .with_session("s2")
            .with_agent("a1")
            .with_agent("a2");

        assert!(filter.has_session_filter());
        assert!(filter.has_agent_filter());
        assert!(filter.matches_all_types());

        // Check that both sessions are in the filter
        let session_ids = filter.session_ids.as_ref().unwrap();
        assert!(session_ids.contains("s1"));
        assert!(session_ids.contains("s2"));

        // Check that both agents are in the filter
        let agent_ids = filter.agent_ids.as_ref().unwrap();
        assert!(agent_ids.contains("a1"));
        assert!(agent_ids.contains("a2"));
    }

    #[test]
    fn test_filter_helper_methods() {
        let filter = EventFilter::new(vec![EventType::InputReceived]);
        assert!(!filter.has_session_filter());
        assert!(!filter.has_agent_filter());
        assert!(!filter.matches_all_types());

        let filter = filter.with_session("s1");
        assert!(filter.has_session_filter());
        assert!(!filter.has_agent_filter());

        let filter = filter.with_agent("a1");
        assert!(filter.has_session_filter());
        assert!(filter.has_agent_filter());

        let all_filter = EventFilter::all();
        assert!(all_filter.matches_all_types());
    }

    #[test]
    fn test_filter_default() {
        let filter = EventFilter::default();

        // Default filter has no restrictions but also no event types
        assert!(!filter.has_session_filter());
        assert!(!filter.has_agent_filter());
        assert!(filter.event_types.is_empty());

        // Should not match anything since event_types is empty
        assert!(!filter.matches(&GlobalEvent::new("s1", None, make_input_event())));
    }

    #[test]
    fn test_filter_clone() {
        let filter = EventFilter::new(vec![EventType::InputReceived])
            .with_session("s1")
            .with_agent("a1");

        let cloned = filter.clone();

        assert_eq!(
            filter.session_ids.as_ref().unwrap(),
            cloned.session_ids.as_ref().unwrap()
        );
        assert_eq!(
            filter.agent_ids.as_ref().unwrap(),
            cloned.agent_ids.as_ref().unwrap()
        );
        assert_eq!(filter.event_types, cloned.event_types);
    }
}
