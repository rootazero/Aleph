// Aleph/core/src/event/tests/integration.rs
//! Integration tests for the global event bus.

#[cfg(test)]
mod tests {
    use crate::event::filter::EventFilter;
    use crate::event::global_bus::GlobalBus;
    use crate::event::types::{
        AlephEvent, EventType, ProcessCompletionEvent, SubAgentCompletionEvent,
    };
    use crate::sync_primitives::Arc;
    use crate::sync_primitives::{AtomicUsize, Ordering};

    // =========================================================================
    // GlobalBus + Multiple EventBus Integration Tests
    // =========================================================================

    fn make_subagent_event() -> AlephEvent {
        AlephEvent::SubAgentCompleted(SubAgentCompletionEvent {
            agent_id: "a".into(),
            child_session_id: "s".into(),
            summary: "done".into(),
            success: true,
            error: None,
            request_id: None,
        })
    }

    fn make_process_event() -> AlephEvent {
        AlephEvent::ProcessCompleted(ProcessCompletionEvent {
            process_id: 1,
            command: "echo".into(),
            exit_code: 0,
            success: true,
            output_tail: "ok".into(),
            output_truncated: false,
        })
    }

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

        // Broadcast events from three different agents
        global_bus
            .broadcast("agent-1", "session-a", make_subagent_event())
            .await;
        global_bus
            .broadcast("agent-2", "session-b", make_subagent_event())
            .await;
        global_bus
            .broadcast("agent-2", "session-b", make_process_event())
            .await;
        global_bus
            .broadcast("agent-3", "session-c", make_subagent_event())
            .await;

        // Allow async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify all events were aggregated
        assert_eq!(
            total_events.load(Ordering::SeqCst),
            4,
            "Should have 4 total events"
        );
        assert_eq!(
            agent1_events.load(Ordering::SeqCst),
            1,
            "Agent 1 should have 1 event"
        );
        assert_eq!(
            agent2_events.load(Ordering::SeqCst),
            2,
            "Agent 2 should have 2 events"
        );
        assert_eq!(
            agent3_events.load(Ordering::SeqCst),
            1,
            "Agent 3 should have 1 event"
        );
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

        // Broadcast events to different sessions
        global_bus
            .broadcast("agent-1", "session-a", make_process_event())
            .await;
        global_bus
            .broadcast("agent-1", "session-a", make_process_event())
            .await;
        global_bus
            .broadcast("agent-1", "session-b", make_process_event())
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(session_a_events.load(Ordering::SeqCst), 2);
        assert_eq!(session_b_events.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_global_bus_filter_by_event_type() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let subagent_events = Arc::new(AtomicUsize::new(0));
        let process_events = Arc::new(AtomicUsize::new(0));

        let subagent_clone = subagent_events.clone();
        let process_clone = process_events.clone();

        // Subscribe to subagent events
        let filter_subagent = EventFilter::new(vec![EventType::SubAgentCompleted]);
        let _sub_subagent = global_bus
            .subscribe_async(filter_subagent, move |_| {
                subagent_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Subscribe to process events
        let filter_process = EventFilter::new(vec![EventType::ProcessCompleted]);
        let _sub_process = global_bus
            .subscribe_async(filter_process, move |_| {
                process_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Broadcast different event types
        global_bus
            .broadcast("agent-1", "session-1", make_subagent_event())
            .await;
        global_bus
            .broadcast("agent-1", "session-1", make_process_event())
            .await;
        global_bus
            .broadcast("agent-1", "session-1", make_process_event())
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(subagent_events.load(Ordering::SeqCst), 1);
        assert_eq!(process_events.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_global_bus_combined_filters() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let matched_events = Arc::new(AtomicUsize::new(0));
        let matched_clone = matched_events.clone();

        // Subscribe to SubAgentCompleted events from agent-1 in session-1
        let filter = EventFilter::new(vec![EventType::SubAgentCompleted])
            .with_agent("agent-1")
            .with_session("session-1");

        let _sub = global_bus
            .subscribe_async(filter, move |_| {
                matched_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Broadcast matching event
        global_bus
            .broadcast("agent-1", "session-1", make_subagent_event())
            .await; // Should match

        // Broadcast wrong event type
        global_bus
            .broadcast("agent-1", "session-1", make_process_event())
            .await;

        // Broadcast wrong agent
        global_bus
            .broadcast("agent-2", "session-1", make_subagent_event())
            .await;

        // Broadcast wrong session
        global_bus
            .broadcast("agent-1", "session-2", make_subagent_event())
            .await;

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

        // Parent subscribes to child's session SubAgentCompleted events
        let filter =
            EventFilter::new(vec![EventType::SubAgentCompleted]).with_session("child-session");

        let _sub = global_bus
            .subscribe_async(filter, move |event| {
                // Verify it's from the child
                assert_eq!(event.source_session_id, "child-session");
                child_completed_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Simulate child agent
        global_bus
            .broadcast("child-agent", "child-session", make_subagent_event())
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(child_completed.load(Ordering::SeqCst), 1);
    }

    // =========================================================================
    // Broadcast Receiver Tests
    // =========================================================================

    #[tokio::test]
    async fn test_broadcast_receiver_async_consumption() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let mut receiver = global_bus.subscribe_broadcast();

        // Spawn a task to receive events
        let receive_task = tokio::spawn(async move {
            let result =
                tokio::time::timeout(tokio::time::Duration::from_millis(100), receiver.recv())
                    .await;

            result.is_ok() && result.unwrap().is_ok()
        });

        // Give receiver time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Broadcast event
        global_bus
            .broadcast("agent-1", "session-1", make_subagent_event())
            .await;

        // Verify receiver got the event
        let received = receive_task.await.unwrap();
        assert!(
            received,
            "Broadcast receiver should have received the event"
        );
    }

    // =========================================================================
    // Event Sequence Tests
    // =========================================================================

    #[tokio::test]
    async fn test_global_bus_maintains_sequence_ordering() {
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let sequences = Arc::new(crate::sync_primitives::Mutex::new(Vec::new()));
        let seq_clone = sequences.clone();

        let _sub = global_bus
            .subscribe_async(EventFilter::all(), move |event| {
                seq_clone
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(event.sequence);
            })
            .await;

        // Publish multiple events
        for _ in 0..5 {
            global_bus
                .broadcast("agent-1", "session-1", make_subagent_event())
                .await;
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let seqs = sequences.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(seqs.len(), 5);

        // Verify sequences are monotonically increasing
        for window in seqs.windows(2) {
            assert!(
                window[1] > window[0],
                "Sequences should be monotonically increasing"
            );
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

        let session_ids = filter.session_ids.as_ref().unwrap();
        assert!(session_ids.contains("session-1"));
        assert!(session_ids.contains("session-2"));
    }

    #[test]
    fn test_event_filter_multiple_agents() {
        let filter = EventFilter::all()
            .with_agent("agent-1")
            .with_agent("agent-2");

        let agent_ids = filter.agent_ids.as_ref().unwrap();
        assert!(agent_ids.contains("agent-1"));
        assert!(agent_ids.contains("agent-2"));
    }

    #[test]
    fn test_event_filter_multiple_event_types() {
        let filter = EventFilter::new(vec![
            EventType::SubAgentCompleted,
            EventType::SubAgentTreeUpdate,
            EventType::ProcessCompleted,
            EventType::TeamTaskAssigned,
        ]);

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
            make_subagent_event(),
            0,
        );

        assert!(!filter.matches(&event));
    }
}
