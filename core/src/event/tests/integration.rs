// Aleph/core/src/event/tests/integration.rs
//! Integration tests for the enhanced event system.
//!
//! These tests verify that:
//! 1. GlobalBus receives events from multiple EventBus instances
//! 2. EventFilter correctly filters events by session, agent, and event type
//! 3. The complete event flow works with cross-agent communication

#[cfg(test)]
mod tests {
    use crate::event::bus::EventBus;
    use crate::event::filter::EventFilter;
    use crate::event::global_bus::GlobalBus;
    use crate::event::types::{AlephEvent, EventType, InputEvent, StopReason, TokenUsage, ToolCallResult};
    use crate::sync_primitives::{AtomicUsize, Ordering};
    use crate::sync_primitives::Arc;

    // =========================================================================
    // GlobalBus + Multiple EventBus Integration Tests
    // =========================================================================

    #[tokio::test]
    async fn test_global_bus_aggregates_from_multiple_agents() {
        // Create a dedicated GlobalBus for this test
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        // Track events received at GlobalBus
        let total_events = Arc::new(AtomicUsize::new(0));
        let agent1_events = Arc::new(AtomicUsize::new(0));
        let agent2_events = Arc::new(AtomicUsize::new(0));
        let agent3_events = Arc::new(AtomicUsize::new(0));

        let total_clone = total_events.clone();
        let a1_clone = agent1_events.clone();
        let a2_clone = agent2_events.clone();
        let a3_clone = agent3_events.clone();

        // Subscribe to all events on GlobalBus
        let _sub_id = global_bus
            .subscribe_async(EventFilter::all(), move |event| {
                total_clone.fetch_add(1, Ordering::SeqCst);
                match event.source_agent_id.as_str() {
                    "agent-1" => a1_clone.fetch_add(1, Ordering::SeqCst),
                    "agent-2" => a2_clone.fetch_add(1, Ordering::SeqCst),
                    "agent-3" => a3_clone.fetch_add(1, Ordering::SeqCst),
                    _ => 0,
                };
            })
            .await;

        // Create three EventBus instances connected to the same GlobalBus
        let bus1 = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-a")
            .with_global_bus(global_bus);

        let bus2 = EventBus::new()
            .with_agent_id("agent-2")
            .with_session_id("session-b")
            .with_global_bus(global_bus);

        let bus3 = EventBus::new()
            .with_agent_id("agent-3")
            .with_session_id("session-c")
            .with_global_bus(global_bus);

        // Publish events from each bus
        bus1.publish(AlephEvent::InputReceived(InputEvent {
            text: "Hello from agent 1".to_string(),
            topic_id: None,
            context: None,
            timestamp: 1000,
        }))
        .await;

        bus2.publish(AlephEvent::InputReceived(InputEvent {
            text: "Hello from agent 2".to_string(),
            topic_id: None,
            context: None,
            timestamp: 2000,
        }))
        .await;

        bus2.publish(AlephEvent::LoopStop(StopReason::Completed)).await;

        bus3.publish(AlephEvent::InputReceived(InputEvent {
            text: "Hello from agent 3".to_string(),
            topic_id: None,
            context: None,
            timestamp: 3000,
        }))
        .await;

        // Allow async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify all events were aggregated
        assert_eq!(total_events.load(Ordering::SeqCst), 4, "Should have 4 total events");
        assert_eq!(agent1_events.load(Ordering::SeqCst), 1, "Agent 1 should have 1 event");
        assert_eq!(agent2_events.load(Ordering::SeqCst), 2, "Agent 2 should have 2 events");
        assert_eq!(agent3_events.load(Ordering::SeqCst), 1, "Agent 3 should have 1 event");
    }

    #[tokio::test]
    async fn test_global_bus_filter_by_session() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let session_a_events = Arc::new(AtomicUsize::new(0));
        let session_b_events = Arc::new(AtomicUsize::new(0));

        let sa_clone = session_a_events.clone();
        let sb_clone = session_b_events.clone();

        // Subscribe to session-a events only
        let filter_a = EventFilter::all().with_session("session-a");
        let _sub_a = global_bus
            .subscribe_async(filter_a, move |_| {
                sa_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Subscribe to session-b events only
        let filter_b = EventFilter::all().with_session("session-b");
        let _sub_b = global_bus
            .subscribe_async(filter_b, move |_| {
                sb_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Create buses with different sessions
        let bus_a = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-a")
            .with_global_bus(global_bus);

        let bus_b = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-b")
            .with_global_bus(global_bus);

        // Publish events
        bus_a.publish(AlephEvent::LoopStop(StopReason::Completed)).await;
        bus_a.publish(AlephEvent::LoopStop(StopReason::Completed)).await;
        bus_b.publish(AlephEvent::LoopStop(StopReason::Completed)).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(session_a_events.load(Ordering::SeqCst), 2);
        assert_eq!(session_b_events.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_global_bus_filter_by_event_type() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let tool_events = Arc::new(AtomicUsize::new(0));
        let stop_events = Arc::new(AtomicUsize::new(0));

        let tool_clone = tool_events.clone();
        let stop_clone = stop_events.clone();

        // Subscribe to tool events
        let filter_tool = EventFilter::new(vec![EventType::ToolCallCompleted]);
        let _sub_tool = global_bus
            .subscribe_async(filter_tool, move |_| {
                tool_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Subscribe to stop events
        let filter_stop = EventFilter::new(vec![EventType::LoopStop]);
        let _sub_stop = global_bus
            .subscribe_async(filter_stop, move |_| {
                stop_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        let bus = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-1")
            .with_global_bus(global_bus);

        // Publish different event types
        bus.publish(AlephEvent::ToolCallCompleted(ToolCallResult {
            call_id: "call-1".to_string(),
            tool: "search".to_string(),
            input: serde_json::json!({}),
            output: "results".to_string(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: TokenUsage::default(),
            session_id: None,
        }))
        .await;

        bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;
        bus.publish(AlephEvent::LoopStop(StopReason::UserAborted)).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(tool_events.load(Ordering::SeqCst), 1);
        assert_eq!(stop_events.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_global_bus_combined_filters() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let matched_events = Arc::new(AtomicUsize::new(0));
        let matched_clone = matched_events.clone();

        // Subscribe to LoopStop events from agent-1 in session-1
        let filter = EventFilter::new(vec![EventType::LoopStop])
            .with_agent("agent-1")
            .with_session("session-1");

        let _sub = global_bus
            .subscribe_async(filter, move |_| {
                matched_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Create buses
        let bus_match = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-1")
            .with_global_bus(global_bus);

        let bus_wrong_agent = EventBus::new()
            .with_agent_id("agent-2")
            .with_session_id("session-1")
            .with_global_bus(global_bus);

        let bus_wrong_session = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-2")
            .with_global_bus(global_bus);

        // Publish events
        bus_match.publish(AlephEvent::LoopStop(StopReason::Completed)).await; // Should match
        bus_match.publish(AlephEvent::InputReceived(InputEvent { // Wrong event type
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        })).await;
        bus_wrong_agent.publish(AlephEvent::LoopStop(StopReason::Completed)).await; // Wrong agent
        bus_wrong_session.publish(AlephEvent::LoopStop(StopReason::Completed)).await; // Wrong session

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Only one event should match all criteria
        assert_eq!(matched_events.load(Ordering::SeqCst), 1);
    }

    // =========================================================================
    // Sub-Agent Event Flow Tests
    // =========================================================================

    #[tokio::test]
    async fn test_parent_subscribes_to_child_completion() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let child_completed = Arc::new(AtomicUsize::new(0));
        let child_completed_clone = child_completed.clone();

        // Parent subscribes to child's session LoopStop events
        let filter = EventFilter::new(vec![EventType::LoopStop])
            .with_session("child-session");

        let _sub = global_bus
            .subscribe_async(filter, move |event| {
                // Verify it's from the child
                assert_eq!(event.source_session_id, "child-session");
                child_completed_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Simulate child agent
        let child_bus = EventBus::new()
            .with_agent_id("child-agent")
            .with_session_id("child-session")
            .with_global_bus(global_bus);

        // Child completes its work
        child_bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(child_completed.load(Ordering::SeqCst), 1);
    }

    // =========================================================================
    // GlobalBus Agent Registration Tests
    // =========================================================================

    #[tokio::test]
    async fn test_agent_registration_and_cleanup() {
        let global_bus = GlobalBus::new();

        // Register some agents
        {
            let bus1 = Arc::new(EventBus::new());
            let bus2 = Arc::new(EventBus::new());

            global_bus.register_agent("agent-1", bus1.clone()).await;
            global_bus.register_agent("agent-2", bus2.clone()).await;

            assert_eq!(global_bus.agent_count().await, 2);

            // bus1 and bus2 are dropped here
        }

        // Cleanup should remove stale references
        global_bus.cleanup_stale_agents().await;
        assert_eq!(global_bus.agent_count().await, 0);
    }

    #[tokio::test]
    async fn test_unregister_agent() {
        let global_bus = GlobalBus::new();

        let bus = Arc::new(EventBus::new());
        global_bus.register_agent("agent-1", bus).await;
        assert_eq!(global_bus.agent_count().await, 1);

        global_bus.unregister_agent("agent-1").await;
        assert_eq!(global_bus.agent_count().await, 0);
    }

    // =========================================================================
    // Broadcast Receiver Tests
    // =========================================================================

    #[tokio::test]
    async fn test_broadcast_receiver_async_consumption() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let mut receiver = global_bus.subscribe_broadcast();

        let bus = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-1")
            .with_global_bus(global_bus);

        // Spawn a task to receive events
        let receive_task = tokio::spawn(async move {
            let result = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                receiver.recv(),
            )
            .await;

            result.is_ok() && result.unwrap().is_ok()
        });

        // Give receiver time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish event
        bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;

        // Verify receiver got the event
        let received = receive_task.await.unwrap();
        assert!(received, "Broadcast receiver should have received the event");
    }

    // =========================================================================
    // Event Sequence Tests
    // =========================================================================

    #[tokio::test]
    async fn test_global_bus_maintains_sequence_ordering() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let sequences = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seq_clone = sequences.clone();

        let _sub = global_bus
            .subscribe_async(EventFilter::all(), move |event| {
                seq_clone.lock().unwrap().push(event.sequence);
            })
            .await;

        let bus = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-1")
            .with_global_bus(global_bus);

        // Publish multiple events
        for _ in 0..5 {
            bus.publish(AlephEvent::LoopStop(StopReason::Completed)).await;
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let seqs = sequences.lock().unwrap();
        assert_eq!(seqs.len(), 5);

        // Verify sequences are monotonically increasing
        for window in seqs.windows(2) {
            assert!(window[1] > window[0], "Sequences should be monotonically increasing");
        }
    }

    // =========================================================================
    // Filter Edge Cases Tests
    // =========================================================================

    #[test]
    fn test_event_filter_multiple_sessions() {
        let filter = EventFilter::all()
            .with_session("session-1")
            .with_session("session-2");

        assert!(filter.has_session_filter());

        let session_ids = filter.session_ids.as_ref().unwrap();
        assert!(session_ids.contains("session-1"));
        assert!(session_ids.contains("session-2"));
    }

    #[test]
    fn test_event_filter_multiple_agents() {
        let filter = EventFilter::all()
            .with_agent("agent-1")
            .with_agent("agent-2");

        assert!(filter.has_agent_filter());

        let agent_ids = filter.agent_ids.as_ref().unwrap();
        assert!(agent_ids.contains("agent-1"));
        assert!(agent_ids.contains("agent-2"));
    }

    #[test]
    fn test_event_filter_multiple_event_types() {
        let filter = EventFilter::new(vec![
            EventType::InputReceived,
            EventType::ToolCallStarted,
            EventType::ToolCallCompleted,
            EventType::LoopStop,
        ]);

        assert!(!filter.matches_all_types());
        assert_eq!(filter.event_types.len(), 4);
    }

    #[test]
    fn test_event_filter_empty_matches_nothing() {
        let filter = EventFilter::default();
        assert!(filter.event_types.is_empty());

        // Empty filter should not match any event
        let event = crate::event::global_bus::GlobalEvent::new(
            "agent-1",
            "session-1",
            AlephEvent::LoopStop(StopReason::Completed),
            0,
        );

        assert!(!filter.matches(&event));
    }
}
