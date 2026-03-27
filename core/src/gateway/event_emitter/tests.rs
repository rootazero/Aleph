//! Tests for event emitter subsystem

use super::*;
use crate::sync_primitives::Arc;

#[tokio::test]
async fn test_collecting_emitter() {
    let emitter = CollectingEventEmitter::new();

    emitter.emit_reasoning("run-1", "Thinking...", false).await;
    emitter.emit_reasoning("run-1", "Done thinking", true).await;

    let events = emitter.events().await;
    assert_eq!(events.len(), 2);

    match &events[0] {
        StreamEvent::Reasoning { content, is_complete, .. } => {
            assert_eq!(content, "Thinking...");
            assert!(!is_complete);
        }
        _ => panic!("Expected Reasoning event"),
    }
}

#[tokio::test]
async fn test_sequence_numbers() {
    let emitter = CollectingEventEmitter::new();

    emitter.emit_reasoning("run-1", "First", false).await;
    emitter.emit_reasoning("run-1", "Second", false).await;
    emitter.emit_reasoning("run-1", "Third", true).await;

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

    emitter
        .emit_tool_start("run-1", "read_file", "tool-1", serde_json::json!({"path": "/tmp/test"}))
        .await;
    emitter
        .emit_tool_update("run-1", "tool-1", "Reading file...")
        .await;
    emitter
        .emit_tool_end("run-1", "tool-1", ToolResult::success("file contents"), 100)
        .await;

    let events = emitter.events().await;
    assert_eq!(events.len(), 3);

    assert!(matches!(&events[0], StreamEvent::ToolStart { .. }));
    assert!(matches!(&events[1], StreamEvent::ToolUpdate { .. }));
    assert!(matches!(&events[2], StreamEvent::ToolEnd { .. }));
}

#[test]
fn test_event_method_names() {
    let event = StreamEvent::Reasoning {
        run_id: "".to_string(),
        seq: 0,
        content: "".to_string(),
        is_complete: false,
    };
    assert_eq!(event_method(&event), "stream.reasoning");

    let event = StreamEvent::ToolStart {
        run_id: "".to_string(),
        seq: 0,
        tool_name: "".to_string(),
        tool_id: "".to_string(),
        params: serde_json::json!({}),
    };
    assert_eq!(event_method(&event), "stream.tool_start");
}

#[test]
fn test_reasoning_block_serialization() {
    let event = StreamEvent::reasoning_block(
        "run-123",
        1,
        ReasoningStepType::Analysis,
        "Analyzing options",
        "Comparing Redis vs in-memory cache",
        false,
    );

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("reasoning_block"));
    assert!(json.contains("analysis"));
    assert!(json.contains("Analyzing options"));
}

#[test]
fn test_reasoning_block_with_confidence() {
    let event = StreamEvent::reasoning_block_with_confidence(
        "run-123",
        2,
        ReasoningStepType::Decision,
        "Final decision",
        "Will use Redis",
        ConfidenceLevel::High,
        true,
    );

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("decision"));
    assert!(json.contains("high"));
    assert!(json.contains("is_final"));
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
    assert!(UncertaintyAction::ProceedWithCaution.description().contains("caution"));
    assert!(UncertaintyAction::AskForClarification.description().contains("clarification"));
}

#[test]
fn test_deserialize_reasoning_block() {
    let json = r#"{"type":"reasoning_block","run_id":"r1","seq":1,"step_type":"observation","label":"Look","content":"Seeing the code","confidence":null,"is_final":false}"#;
    let event: StreamEvent = serde_json::from_str(json).unwrap();

    if let StreamEvent::ReasoningBlock { step_type, label, .. } = event {
        assert_eq!(step_type, ReasoningStepType::Observation);
        assert_eq!(label, "Look");
    } else {
        panic!("Wrong event type");
    }
}

#[test]
fn test_output_mode_from_config() {
    assert_eq!(OutputMode::from_config("typewriter"), OutputMode::Typewriter);
    assert_eq!(OutputMode::from_config("instant"), OutputMode::Instant);
    assert_eq!(OutputMode::from_config("unknown"), OutputMode::Typewriter);
    assert_eq!(OutputMode::from_config(""), OutputMode::Typewriter);
}

#[tokio::test]
async fn test_instant_mode_buffers_non_final_chunks() {
    use crate::gateway::event_bus::GatewayEventBus;

    let event_bus = Arc::new(GatewayEventBus::new());
    let emitter = GatewayEventEmitter::with_output_mode(
        event_bus,
        OutputMode::Instant,
    );

    // Non-final chunks should be buffered, not emitted
    let _ = emitter
        .emit(StreamEvent::ResponseChunk {
            run_id: "run-1".to_string(),
            seq: 0,
            content: "Hello ".to_string(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        })
        .await;

    let _ = emitter
        .emit(StreamEvent::ResponseChunk {
            run_id: "run-1".to_string(),
            seq: 1,
            content: "World".to_string(),
            chunk_index: 1,
            is_final: false,
            is_intermediate: false,
        })
        .await;

    // Check that content is buffered in instant_buffer
    let buffer = emitter.instant_buffer.lock().await;
    assert_eq!(*buffer, "Hello World");
    drop(buffer);

    // Final chunk should flush everything
    let _ = emitter
        .emit(StreamEvent::ResponseChunk {
            run_id: "run-1".to_string(),
            seq: 2,
            content: "!".to_string(),
            chunk_index: 2,
            is_final: true,
            is_intermediate: false,
        })
        .await;

    // Buffer should be empty after final
    let buffer = emitter.instant_buffer.lock().await;
    assert!(buffer.is_empty(), "Instant buffer should be empty after final chunk");
}

#[tokio::test]
async fn test_instant_mode_passes_non_chunk_events() {
    use crate::gateway::event_bus::GatewayEventBus;

    let event_bus = Arc::new(GatewayEventBus::new());
    let emitter = GatewayEventEmitter::with_output_mode(
        event_bus.clone(),
        OutputMode::Instant,
    );

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
    assert!(msg.is_ok(), "Non-ResponseChunk events should be emitted immediately in instant mode");
}

#[test]
fn test_default_output_mode_is_typewriter() {
    use crate::gateway::event_bus::GatewayEventBus;

    let event_bus = Arc::new(GatewayEventBus::new());
    let emitter = GatewayEventEmitter::new(event_bus);
    assert_eq!(*emitter.output_mode(), OutputMode::Typewriter);
}
