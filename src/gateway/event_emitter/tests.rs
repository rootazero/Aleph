//! Tests for event emitter subsystem

use super::*;
use crate::sync_primitives::Arc;

#[tokio::test]
async fn test_collecting_emitter() {
    let emitter = CollectingEventEmitter::new();

    let seq = emitter.next_seq();
    emitter
        .emit(StreamEvent::Reasoning {
            run_id: "run-1".to_string(),
            seq,
            content: "Thinking...".to_string(),
            is_complete: false,
        })
        .await
        .unwrap();

    let seq = emitter.next_seq();
    emitter
        .emit(StreamEvent::Reasoning {
            run_id: "run-1".to_string(),
            seq,
            content: "Done thinking".to_string(),
            is_complete: true,
        })
        .await
        .unwrap();

    let events = emitter.events().await;
    assert_eq!(events.len(), 2);

    match &events[0] {
        StreamEvent::Reasoning {
            content,
            is_complete,
            ..
        } => {
            assert_eq!(content, "Thinking...");
            assert!(!is_complete);
        }
        _ => panic!("Expected Reasoning event"),
    }
}

#[tokio::test]
async fn test_sequence_numbers() {
    let emitter = CollectingEventEmitter::new();

    let first = emitter.next_seq();
    let second = emitter.next_seq();
    let third = emitter.next_seq();
    assert_eq!((first, second, third), (0, 1, 2));

    for (index, seq) in [first, second, third].iter().enumerate() {
        emitter
            .emit(StreamEvent::Reasoning {
                run_id: "run-1".to_string(),
                seq: *seq,
                content: format!("chunk-{index}"),
                is_complete: false,
            })
            .await
            .unwrap();
    }

    let events = emitter.events().await;
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Reasoning { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();

    assert_eq!(seqs, vec![0, 1, 2]);
}

#[tokio::test]
async fn test_tool_lifecycle() {
    let emitter = CollectingEventEmitter::new();

    let seq = emitter.next_seq();
    emitter
        .emit(StreamEvent::ToolStart {
            run_id: "run-1".to_string(),
            seq,
            tool_name: "read_file".to_string(),
            tool_id: "tool-1".to_string(),
            params: serde_json::json!({"path": "/tmp/test"}),
        })
        .await
        .unwrap();

    let seq = emitter.next_seq();
    emitter
        .emit(StreamEvent::ToolUpdate {
            run_id: "run-1".to_string(),
            seq,
            tool_id: "tool-1".to_string(),
            progress: "Reading file...".to_string(),
        })
        .await
        .unwrap();

    let seq = emitter.next_seq();
    emitter
        .emit(StreamEvent::ToolEnd {
            run_id: "run-1".to_string(),
            seq,
            tool_id: "tool-1".to_string(),
            result: ToolResult::success("file contents"),
            duration_ms: 100,
        })
        .await
        .unwrap();

    let events = emitter.events().await;
    assert_eq!(events.len(), 3);

    assert!(matches!(&events[0], StreamEvent::ToolStart { .. }));
    assert!(matches!(&events[1], StreamEvent::ToolUpdate { .. }));
    assert!(matches!(&events[2], StreamEvent::ToolEnd { .. }));
}

#[test]
fn test_uncertainty_signal() {
    let event = StreamEvent::uncertainty_signal(
        "run-123",
        3,
        "Not sure about the caching strategy",
        UncertaintyAction::AskForClarification,
    );

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("uncertainty_signal"));
    assert!(json.contains("ask_for_clarification"));
}

#[test]
fn test_uncertainty_action_description() {
    assert!(UncertaintyAction::ProceedWithCaution
        .description()
        .contains("caution"));
    assert!(UncertaintyAction::AskForClarification
        .description()
        .contains("clarification"));
}

#[test]
fn test_agent_trace_serialization() {
    let event = StreamEvent::AgentTrace {
        run_id: "run-123".into(),
        seq: 7,
        event: crate::harness::trace::LoopTraceEvent::TurnCompleted {
            iteration: 2,
            outcome: crate::harness::trace::LoopTraceTurnOutcome::Continue,
            metrics: crate::harness::trace::LoopTraceTurnMetrics {
                requested_tool_calls: 1,
                executed_tool_calls: 1,
                productive: true,
                total_tokens: 42,
            },
        }
        .into(),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("agent_trace"));
    assert!(json.contains("turn_completed"));
    assert!(json.contains("\"productive\":true"));
}

#[test]
fn test_output_mode_from_config() {
    assert_eq!(
        OutputMode::from_config("typewriter"),
        OutputMode::Typewriter
    );
    assert_eq!(OutputMode::from_config("instant"), OutputMode::Instant);
    assert_eq!(OutputMode::from_config("unknown"), OutputMode::Typewriter);
    assert_eq!(OutputMode::from_config(""), OutputMode::Typewriter);
}

#[tokio::test]
async fn test_instant_mode_buffers_non_final_chunks() {
    use crate::gateway::event_bus::GatewayEventBus;

    let event_bus = Arc::new(GatewayEventBus::new());
    let emitter = GatewayEventEmitter::with_output_mode(event_bus, OutputMode::Instant);

    // Non-final chunks should be buffered, not emitted
    let _ = emitter
        .emit(StreamEvent::ResponseChunk {
            run_id: "run-1".to_string(),
            seq: 0,
            delta: "Hello ".to_string(),
            full_text: String::new(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        })
        .await;

    let _ = emitter
        .emit(StreamEvent::ResponseChunk {
            run_id: "run-1".to_string(),
            seq: 1,
            delta: "World".to_string(),
            full_text: String::new(),
            chunk_index: 1,
            is_final: false,
            is_intermediate: false,
        })
        .await;

    // Check that content is buffered in instant_buffer
    let state = emitter.instant_buffer.lock().await;
    assert_eq!(state.buffer, "Hello World");
    drop(state);

    // Final chunk should flush everything
    let _ = emitter
        .emit(StreamEvent::ResponseChunk {
            run_id: "run-1".to_string(),
            seq: 2,
            delta: "!".to_string(),
            full_text: String::new(),
            chunk_index: 2,
            is_final: true,
            is_intermediate: false,
        })
        .await;

    // Buffer should be empty after final (and the final-emitted marker set,
    // so the RunComplete summary fallback cannot double-deliver).
    let state = emitter.instant_buffer.lock().await;
    assert!(
        state.buffer.is_empty(),
        "Instant buffer should be empty after final chunk"
    );
    assert!(state.final_emitted, "final chunk must set the marker");
}

#[tokio::test]
async fn test_instant_mode_passes_non_chunk_events() {
    use crate::gateway::event_bus::GatewayEventBus;

    let event_bus = Arc::new(GatewayEventBus::new());
    let emitter = GatewayEventEmitter::with_output_mode(event_bus.clone(), OutputMode::Instant);

    // Subscribe to verify events are published
    let mut rx = event_bus.subscribe();

    // Reasoning events should still be emitted immediately in instant mode
    let _ = emitter
        .emit(StreamEvent::Reasoning {
            run_id: "run-1".to_string(),
            seq: 0,
            content: "Thinking...".to_string(),
            is_complete: false,
        })
        .await;

    // Should receive the event
    let msg = rx.try_recv();
    assert!(
        msg.is_ok(),
        "Non-ResponseChunk events should be emitted immediately in instant mode"
    );
}

#[test]
fn test_default_output_mode_is_typewriter() {
    use crate::gateway::event_bus::GatewayEventBus;

    let event_bus = Arc::new(GatewayEventBus::new());
    let emitter = GatewayEventEmitter::new(event_bus);
    assert_eq!(*emitter.output_mode(), OutputMode::Typewriter);
}
