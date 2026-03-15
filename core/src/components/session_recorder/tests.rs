//! Tests for SessionRecorder.

use super::*;
use crate::components::{AiResponsePart, SessionPart, UserInputPart};
use crate::event::{
    AlephEvent, AiResponse, ErrorKind, EventBus, EventContext, EventType, InputContext, InputEvent,
    PlanStep, StepStatus, TaskPlan, TokenUsage, ToolCallError, ToolCallResult,
};

// ========================================================================
// Construction Tests
// ========================================================================

#[test]
fn test_create_in_memory() {
    let recorder = SessionRecorder::new_in_memory();
    assert!(recorder.is_ok());
}

#[test]
fn test_create_file_based() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test_sessions.db");

    let recorder = SessionRecorder::new(&db_path);
    assert!(recorder.is_ok());
    assert!(db_path.exists());
}

// ========================================================================
// Session Management Tests
// ========================================================================

#[test]
fn test_create_session() {
    let recorder = SessionRecorder::new_in_memory().unwrap();

    let result = recorder.create_session("session-001", "gpt-4");
    assert!(result.is_ok());

    // Verify session was created
    let session = recorder.get_session("session-001").unwrap();
    assert!(session.is_some());

    let session = session.unwrap();
    assert_eq!(session.id, "session-001");
    assert_eq!(session.model, "gpt-4");
    assert_eq!(session.agent_id, "main");
    assert_eq!(session.status, "running");
    assert_eq!(session.iteration_count, 0);
    assert_eq!(session.total_tokens, 0);
}

#[test]
fn test_create_session_with_parent() {
    let recorder = SessionRecorder::new_in_memory().unwrap();

    // Create parent session
    recorder.create_session("parent-001", "gpt-4").unwrap();

    // Create child session
    let result = recorder.create_session_with_options(
        "child-001",
        "gpt-4",
        Some("parent-001"),
        "sub-agent",
    );
    assert!(result.is_ok());

    let session = recorder.get_session("child-001").unwrap().unwrap();
    assert_eq!(session.parent_id, Some("parent-001".to_string()));
    assert_eq!(session.agent_id, "sub-agent");
}

#[test]
fn test_update_session() {
    let recorder = SessionRecorder::new_in_memory().unwrap();

    recorder.create_session("session-001", "gpt-4").unwrap();

    // Update session
    let result = recorder.update_session("session-001");
    assert!(result.is_ok());

    // Verify iteration count increased
    let session = recorder.get_session("session-001").unwrap().unwrap();
    assert_eq!(session.iteration_count, 1);

    // Update again
    recorder.update_session("session-001").unwrap();
    let session = recorder.get_session("session-001").unwrap().unwrap();
    assert_eq!(session.iteration_count, 2);
}

#[test]
fn test_update_session_full() {
    let recorder = SessionRecorder::new_in_memory().unwrap();

    recorder.create_session("session-001", "gpt-4").unwrap();

    // Update with all fields
    let result =
        recorder.update_session_full("session-001", Some("completed"), Some(5), Some(1000));
    assert!(result.is_ok());

    let session = recorder.get_session("session-001").unwrap().unwrap();
    assert_eq!(session.status, "completed");
    assert_eq!(session.iteration_count, 5);
    assert_eq!(session.total_tokens, 1000);
}

// ========================================================================
// Part Persistence Tests
// ========================================================================

#[test]
fn test_append_part() {
    let recorder = SessionRecorder::new_in_memory().unwrap();

    recorder.create_session("session-001", "gpt-4").unwrap();

    let part = SessionPart::UserInput(UserInputPart {
        text: "Hello, world!".to_string(),
        context: None,
        timestamp: chrono::Utc::now().timestamp(),
    });

    let result = recorder.append_part("session-001", &part);
    assert!(result.is_ok());

    // Verify part was stored
    let parts = recorder.get_session_parts("session-001").unwrap();
    assert_eq!(parts.len(), 1);

    if let SessionPart::UserInput(input) = &parts[0] {
        assert_eq!(input.text, "Hello, world!");
    } else {
        panic!("Expected UserInput part");
    }
}

#[test]
fn test_append_multiple_parts() {
    let recorder = SessionRecorder::new_in_memory().unwrap();

    recorder.create_session("session-001", "gpt-4").unwrap();

    // Add multiple parts
    let parts_to_add = vec![
        SessionPart::UserInput(UserInputPart {
            text: "First message".to_string(),
            context: None,
            timestamp: 1000,
        }),
        SessionPart::AiResponse(AiResponsePart {
            content: "First response".to_string(),
            reasoning: None,
            timestamp: 1001,
        }),
        SessionPart::UserInput(UserInputPart {
            text: "Second message".to_string(),
            context: None,
            timestamp: 1002,
        }),
    ];

    for part in &parts_to_add {
        recorder.append_part("session-001", part).unwrap();
    }

    // Verify parts are in order
    let parts = recorder.get_session_parts("session-001").unwrap();
    assert_eq!(parts.len(), 3);

    // Check sequence
    if let SessionPart::UserInput(input) = &parts[0] {
        assert_eq!(input.text, "First message");
    }
    if let SessionPart::AiResponse(response) = &parts[1] {
        assert_eq!(response.content, "First response");
    }
    if let SessionPart::UserInput(input) = &parts[2] {
        assert_eq!(input.text, "Second message");
    }
}

// ========================================================================
// Event Conversion Tests
// ========================================================================

#[test]
fn test_event_to_part_input() {
    let event = AlephEvent::InputReceived(InputEvent {
        text: "Hello".to_string(),
        session_id: Some("topic-1".to_string()),
        context: Some(InputContext {
            app_name: Some("Terminal".to_string()),
            window_title: Some("bash".to_string()),
            selected_text: None,
        }),
        timestamp: 1234567890,
    });

    let part = SessionRecorder::event_to_part(&event);
    assert!(part.is_some());

    if let Some(SessionPart::UserInput(input)) = part {
        assert_eq!(input.text, "Hello");
        assert_eq!(input.timestamp, 1234567890);
        assert!(input.context.is_some());
    } else {
        panic!("Expected UserInput part");
    }
}

#[test]
fn test_event_to_part_tool_completed() {
    use crate::components::ToolCallStatus;

    let event = AlephEvent::ToolCallCompleted(ToolCallResult {
        call_id: "call-001".to_string(),
        tool: "web_search".to_string(),
        input: serde_json::json!({"query": "rust programming"}),
        output: "Search results...".to_string(),
        started_at: 1000,
        completed_at: 2000,
        token_usage: TokenUsage::default(),
        session_id: None,
    });

    let part = SessionRecorder::event_to_part(&event);
    assert!(part.is_some());

    if let Some(SessionPart::ToolCall(tool_call)) = part {
        assert_eq!(tool_call.id, "call-001");
        assert_eq!(tool_call.tool_name, "web_search");
        assert_eq!(tool_call.status, ToolCallStatus::Completed);
        assert_eq!(tool_call.output, Some("Search results...".to_string()));
        assert!(tool_call.error.is_none());
    } else {
        panic!("Expected ToolCall part");
    }
}

#[test]
fn test_event_to_part_tool_failed() {
    use crate::components::ToolCallStatus;

    let event = AlephEvent::ToolCallFailed(ToolCallError {
        call_id: "call-002".to_string(),
        tool: "file_read".to_string(),
        error: "File not found".to_string(),
        error_kind: ErrorKind::NotFound,
        is_retryable: false,
        attempts: 1,
        session_id: None,
    });

    let part = SessionRecorder::event_to_part(&event);
    assert!(part.is_some());

    if let Some(SessionPart::ToolCall(tool_call)) = part {
        assert_eq!(tool_call.id, "call-002");
        assert_eq!(tool_call.status, ToolCallStatus::Failed);
        assert_eq!(tool_call.error, Some("File not found".to_string()));
    } else {
        panic!("Expected ToolCall part");
    }
}

#[test]
fn test_event_to_part_ai_response() {
    let event = AlephEvent::AiResponseGenerated(AiResponse {
        content: "Here is my response".to_string(),
        reasoning: Some("I thought about it carefully".to_string()),
        is_final: true,
        timestamp: 1234567890,
    });

    let part = SessionRecorder::event_to_part(&event);
    assert!(part.is_some());

    if let Some(SessionPart::AiResponse(response)) = part {
        assert_eq!(response.content, "Here is my response");
        assert_eq!(
            response.reasoning,
            Some("I thought about it carefully".to_string())
        );
        assert_eq!(response.timestamp, 1234567890);
    } else {
        panic!("Expected AiResponse part");
    }
}

#[test]
fn test_event_to_part_plan_created() {
    let event = AlephEvent::PlanCreated(TaskPlan {
        id: "plan-001".to_string(),
        steps: vec![
            PlanStep {
                id: "step-1".to_string(),
                description: "First step".to_string(),
                tool: "search".to_string(),
                parameters: serde_json::json!({}),
                depends_on: vec![],
                status: StepStatus::Pending,
            },
            PlanStep {
                id: "step-2".to_string(),
                description: "Second step".to_string(),
                tool: "process".to_string(),
                parameters: serde_json::json!({}),
                depends_on: vec!["step-1".to_string()],
                status: StepStatus::Pending,
            },
        ],
        parallel_groups: vec![],
        current_step_index: 0,
    });

    let part = SessionRecorder::event_to_part(&event);
    assert!(part.is_some());

    if let Some(SessionPart::PlanCreated(plan)) = part {
        assert_eq!(plan.plan_id, "plan-001");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].description, "First step");
        assert_eq!(plan.steps[1].description, "Second step");
    } else {
        panic!("Expected PlanCreated part");
    }
}

#[test]
fn test_event_to_part_returns_none_for_internal_events() {
    // ToolCallRequested should not create a part
    let event = AlephEvent::ToolCallRequested(crate::event::ToolCallRequest {
        tool: "test".to_string(),
        parameters: serde_json::json!({}),
        plan_step_id: None,
    });
    assert!(SessionRecorder::event_to_part(&event).is_none());

    // LoopContinue should not create a part
    let event = AlephEvent::LoopContinue(crate::event::LoopState {
        session_id: "test".to_string(),
        iteration: 1,
        total_tokens: 100,
        last_tool: None,
        model: "gpt-4-turbo".to_string(),
    });
    assert!(SessionRecorder::event_to_part(&event).is_none());
}

// ========================================================================
// EventHandler Implementation Tests
// ========================================================================

#[test]
fn test_handler_name() {
    use crate::event::EventHandler;
    let recorder = SessionRecorder::new_in_memory().unwrap();
    assert_eq!(recorder.name(), "SessionRecorder");
}

#[test]
fn test_handler_subscriptions() {
    use crate::event::EventHandler;
    let recorder = SessionRecorder::new_in_memory().unwrap();
    let subs = recorder.subscriptions();

    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0], EventType::All);
}

#[tokio::test]
async fn test_handler_creates_session_on_session_created() {
    use crate::event::EventHandler;
    let recorder = SessionRecorder::new_in_memory().unwrap();
    let bus = EventBus::new();
    let ctx = EventContext::new(bus);

    let event = AlephEvent::SessionCreated(crate::event::SessionInfo {
        id: "session-from-event".to_string(),
        parent_id: None,
        agent_id: "main".to_string(),
        model: "claude-3".to_string(),
        created_at: chrono::Utc::now().timestamp(),
    });

    let result = recorder.handle(&event, &ctx).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty()); // Should not publish any events

    // Verify session was created
    let session = recorder.get_session("session-from-event").unwrap();
    assert!(session.is_some());
    assert_eq!(session.unwrap().model, "claude-3");
}

#[tokio::test]
async fn test_handler_persists_input_event() {
    use crate::event::EventHandler;
    let recorder = SessionRecorder::new_in_memory().unwrap();
    let bus = EventBus::new();
    let ctx = EventContext::new(bus);

    // Create session first
    recorder.create_session("test-session", "gpt-4").unwrap();
    ctx.set_session_id("test-session".to_string()).await;

    // Handle input event
    let event = AlephEvent::InputReceived(InputEvent {
        text: "Test input".to_string(),
        session_id: None,
        context: None,
        timestamp: chrono::Utc::now().timestamp(),
    });

    let result = recorder.handle(&event, &ctx).await;
    assert!(result.is_ok());

    // Verify part was persisted
    let parts = recorder.get_session_parts("test-session").unwrap();
    assert_eq!(parts.len(), 1);
}

#[tokio::test]
async fn test_handler_updates_session_on_loop_continue() {
    use crate::event::EventHandler;
    let recorder = SessionRecorder::new_in_memory().unwrap();
    let bus = EventBus::new();
    let ctx = EventContext::new(bus);

    // Create session
    recorder.create_session("test-session", "gpt-4").unwrap();
    ctx.set_session_id("test-session".to_string()).await;

    // Handle loop continue event
    let event = AlephEvent::LoopContinue(crate::event::LoopState {
        session_id: "test-session".to_string(),
        iteration: 1,
        total_tokens: 100,
        last_tool: None,
        model: "gpt-4-turbo".to_string(),
    });

    recorder.handle(&event, &ctx).await.unwrap();

    // Verify iteration count increased
    let session = recorder.get_session("test-session").unwrap().unwrap();
    assert_eq!(session.iteration_count, 1);
}
