//! Shared test helpers for session compactor tests

use crate::components::types::{
    AiResponsePart, ExecutionSession, SessionPart, ToolCallPart, ToolCallStatus,
    UserInputPart,
};
use serde_json::json;

pub(super) fn create_test_session() -> ExecutionSession {
    let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

    // Add user input
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Please help me analyze this code".to_string(),
        context: None,
        timestamp: 1000,
    }));

    // Add multiple tool calls
    for i in 0..15 {
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: format!("tool-{}", i),
            tool_name: format!("tool_{}", i),
            input: json!({"param": i}),
            status: ToolCallStatus::Completed,
            output: Some(format!("Output for tool {} with some content here", i)),
            error: None,
            started_at: 1000 + i * 100,
            completed_at: Some(1050 + i * 100),
        }));
    }

    // Add AI response
    session.parts.push(SessionPart::AiResponse(AiResponsePart {
        content: "Analysis complete. The code looks good.".to_string(),
        reasoning: Some("Reviewed all components".to_string()),
        timestamp: 3000,
    }));

    session
}

/// Create a test session with skill tool calls that should be protected
pub(super) fn create_test_session_with_skill_calls() -> ExecutionSession {
    let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

    // Add user input
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Use skill to do something".to_string(),
        context: None,
        timestamp: 1000,
    }));

    // Add skill tool calls (should be protected)
    for i in 0..5 {
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: format!("skill-{}", i),
            tool_name: "skill".to_string(),
            input: json!({"param": i}),
            status: ToolCallStatus::Completed,
            output: Some(format!("Skill output {} with content", i)),
            error: None,
            started_at: 1000 + i as i64 * 100,
            completed_at: Some(1050 + i as i64 * 100),
        }));
    }

    // Add another user turn
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Now do more work".to_string(),
        context: None,
        timestamp: 1600,
    }));

    // Add regular tool calls (should be pruned if old enough)
    for i in 0..15 {
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: format!("tool-{}", i),
            tool_name: format!("tool_{}", i),
            input: json!({"param": i}),
            status: ToolCallStatus::Completed,
            output: Some("x".repeat(500)), // ~200 tokens each
            error: None,
            started_at: 2000 + i as i64 * 100,
            completed_at: Some(2050 + i as i64 * 100),
        }));
    }

    // Add third user turn to create safe boundary
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Final request".to_string(),
        context: None,
        timestamp: 4000,
    }));

    session
}

/// Create a large test session for threshold testing
pub(super) fn create_large_test_session() -> ExecutionSession {
    let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

    // Add first user turn
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Start working".to_string(),
        context: None,
        timestamp: 1000,
    }));

    // Add many tool calls with large outputs to exceed thresholds
    for i in 0..50 {
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: format!("large-tool-{}", i),
            tool_name: format!("tool_{}", i % 10),
            input: json!({"param": i}),
            status: ToolCallStatus::Completed,
            // Each output is ~2500 chars = ~1000 tokens
            output: Some("y".repeat(2500)),
            error: None,
            started_at: 2000 + i as i64 * 100,
            completed_at: Some(2050 + i as i64 * 100),
        }));
    }

    // Add second user turn
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Continue".to_string(),
        context: None,
        timestamp: 8000,
    }));

    // Add more recent tool calls
    for i in 0..10 {
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: format!("recent-tool-{}", i),
            tool_name: format!("recent_{}", i),
            input: json!({"param": i}),
            status: ToolCallStatus::Completed,
            output: Some("z".repeat(500)),
            error: None,
            started_at: 9000 + i as i64 * 100,
            completed_at: Some(9050 + i as i64 * 100),
        }));
    }

    // Add third user turn
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Final".to_string(),
        context: None,
        timestamp: 10500,
    }));

    session
}
