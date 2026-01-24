//! Session Sync - Bridge between LoopState and ExecutionSession
//!
//! This module provides synchronization between the legacy LoopState
//! and the unified ExecutionSession model during the migration period.
//!
//! # Overview
//!
//! ```text
//! LoopState (legacy)           ExecutionSession (unified)
//! ├── session_id          →    ├── id
//! ├── original_request    →    ├── original_request
//! ├── context             →    ├── context
//! ├── steps               →    ├── parts (converted)
//! ├── step_count          →    ├── iteration_count
//! ├── total_tokens        →    ├── total_tokens
//! ├── started_at          →    ├── started_at
//! ├── history_summary     →    │   (used in SummaryPart)
//! └── compressed_until    →    └── last_compaction_index
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::agent_loop::{LoopState, RequestContext, SessionSync};
//! use aethecore::components::ExecutionSession;
//!
//! let loop_state = LoopState::new("session-1".into(), "Hello".into(), RequestContext::empty());
//! let session = SessionSync::to_execution_session(&loop_state);
//! ```

use crate::agent_loop::decision::{Action, ActionResult};
use crate::agent_loop::state::{LoopState, LoopStep};
use crate::components::{
    AiResponsePart, ExecutionSession, SessionPart, SessionStatus, SummaryPart, ToolCallPart,
    ToolCallStatus, UserInputPart,
};

/// Bridge for synchronizing between LoopState and ExecutionSession.
///
/// This struct provides static methods to convert between the legacy
/// LoopState model and the unified ExecutionSession model. It is used
/// during the migration period to ensure both representations stay in sync.
pub struct SessionSync;

impl SessionSync {
    /// Convert a LoopState to an ExecutionSession.
    ///
    /// This creates a new ExecutionSession with all fields populated from
    /// the LoopState, including converted parts from the loop steps.
    ///
    /// # Arguments
    ///
    /// * `state` - The LoopState to convert
    ///
    /// # Returns
    ///
    /// A new ExecutionSession populated from the LoopState
    pub fn to_execution_session(state: &LoopState) -> ExecutionSession {
        let now = chrono::Utc::now().timestamp();

        // Convert started_at from Instant to unix timestamp
        // Since Instant is monotonic and doesn't map directly to wall clock,
        // we approximate by subtracting elapsed time from current time
        let started_at = now - state.elapsed().as_secs() as i64;

        // Convert steps to parts
        let parts = Self::steps_to_parts(&state.steps, &state.original_request);

        // Determine if compaction is needed based on compressed state
        // Note: `state.steps.len() > 0` is redundant when checking `compressed_until_step < steps.len()`
        let needs_compaction = state.compressed_until_step < state.steps.len()
            && !state.history_summary.is_empty();

        ExecutionSession {
            id: state.session_id.clone(),
            parent_id: None,
            agent_id: "main".into(),
            status: SessionStatus::Running,
            iteration_count: state.step_count as u32,
            total_tokens: state.total_tokens as u64,
            parts,
            recent_calls: Vec::new(),
            model: "default".into(),
            created_at: started_at,
            updated_at: now,
            // Unified session model fields
            original_request: state.original_request.clone(),
            context: Some(state.context.clone()),
            started_at,
            needs_compaction,
            last_compaction_index: state.compressed_until_step,
        }
    }

    /// Convert LoopSteps to SessionParts.
    ///
    /// This method creates parts from the loop steps, including:
    /// - An initial UserInputPart from the original request
    /// - AiResponsePart for each step's reasoning (if present)
    /// - ToolCallPart for tool calls with proper status mapping
    ///
    /// # Arguments
    ///
    /// * `steps` - The steps to convert
    /// * `original_request` - The original user request (for the initial UserInputPart)
    ///
    /// # Returns
    ///
    /// A Vec of SessionParts representing the steps
    pub fn steps_to_parts(steps: &[LoopStep], original_request: &str) -> Vec<SessionPart> {
        let mut parts = Vec::new();
        let now = chrono::Utc::now().timestamp();

        // Always create initial UserInputPart from original request
        if !original_request.is_empty() {
            parts.push(SessionPart::UserInput(UserInputPart {
                text: original_request.to_string(),
                context: None,
                timestamp: now,
            }));
        }

        // Convert each step to parts
        for step in steps {
            // Add AiResponsePart for reasoning (if present)
            if let Some(ref reasoning) = step.thinking.reasoning {
                if !reasoning.is_empty() {
                    parts.push(SessionPart::AiResponse(AiResponsePart {
                        content: String::new(), // No direct content from thinking
                        reasoning: Some(reasoning.clone()),
                        timestamp: now,
                    }));
                }
            }

            // Convert action to part
            match &step.action {
                Action::ToolCall {
                    tool_name,
                    arguments,
                } => {
                    let (status, output, error) = Self::action_result_to_status(&step.result);

                    parts.push(SessionPart::ToolCall(ToolCallPart {
                        id: format!("call_{}", step.step_id),
                        tool_name: tool_name.clone(),
                        input: arguments.clone(),
                        status,
                        output,
                        error,
                        started_at: now,
                        completed_at: Some(now + (step.duration_ms as i64 / 1000)),
                    }));
                }
                Action::Completion { summary } => {
                    parts.push(SessionPart::AiResponse(AiResponsePart {
                        content: summary.clone(),
                        reasoning: None,
                        timestamp: now,
                    }));
                }
                Action::Failure { reason } => {
                    parts.push(SessionPart::AiResponse(AiResponsePart {
                        content: format!("Failed: {}", reason),
                        reasoning: None,
                        timestamp: now,
                    }));
                }
                Action::UserInteraction { .. } => {
                    // User interactions are handled separately
                }
            }
        }

        parts
    }

    /// Convert ActionResult to ToolCallStatus with output/error.
    ///
    /// # Arguments
    ///
    /// * `result` - The ActionResult to convert
    ///
    /// # Returns
    ///
    /// A tuple of (ToolCallStatus, Option<output>, Option<error>)
    fn action_result_to_status(
        result: &ActionResult,
    ) -> (ToolCallStatus, Option<String>, Option<String>) {
        match result {
            ActionResult::ToolSuccess { output, .. } => {
                (ToolCallStatus::Completed, Some(output.to_string()), None)
            }
            ActionResult::ToolError { error, .. } => {
                // Both retryable and non-retryable errors map to Failed
                (ToolCallStatus::Failed, None, Some(error.clone()))
            }
            ActionResult::UserResponse { response } => {
                (ToolCallStatus::Completed, Some(response.clone()), None)
            }
            ActionResult::Completed => (ToolCallStatus::Completed, None, None),
            ActionResult::Failed => (
                ToolCallStatus::Failed,
                None,
                Some("Action failed".to_string()),
            ),
        }
    }

    /// Sync changes from ExecutionSession back to LoopState.
    ///
    /// This method updates a LoopState with changes from an ExecutionSession,
    /// primarily for fields that can be modified during execution:
    /// - total_tokens
    /// - compression state (via needs_compaction and last_compaction_index)
    ///
    /// Note: Steps are not synced back as they are append-only in LoopState.
    ///
    /// # Arguments
    ///
    /// * `session` - The ExecutionSession with updated values
    /// * `state` - The LoopState to update
    pub fn sync_to_loop_state(session: &ExecutionSession, state: &mut LoopState) {
        // Sync token count
        state.total_tokens = session.total_tokens as usize;

        // Sync compression state
        state.compressed_until_step = session.last_compaction_index;

        // If session has a SummaryPart, update history_summary
        for part in session.parts.iter().rev() {
            if let SessionPart::Summary(summary) = part {
                state.history_summary = summary.content.clone();
                break;
            }
        }
    }

    /// Create a SummaryPart from LoopState compression data.
    ///
    /// This is a helper method to create a SummaryPart that can be added
    /// to an ExecutionSession when compression occurs.
    ///
    /// # Arguments
    ///
    /// * `state` - The LoopState containing compression data
    ///
    /// # Returns
    ///
    /// A SessionPart::Summary if there is compressed history, None otherwise
    pub fn create_summary_part(state: &LoopState) -> Option<SessionPart> {
        if state.history_summary.is_empty() {
            return None;
        }

        Some(SessionPart::Summary(SummaryPart {
            content: state.history_summary.clone(),
            original_count: state.compressed_until_step as u32,
            compacted_at: chrono::Utc::now().timestamp(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::decision::Decision;
    use crate::agent_loop::state::{RequestContext, Thinking};
    use serde_json::json;

    /// Test basic conversion from LoopState to ExecutionSession
    #[test]
    fn test_sync_loop_state_to_session() {
        let state = LoopState::new(
            "test-session-123".to_string(),
            "Find all rust files".to_string(),
            RequestContext::empty(),
        );

        let session = SessionSync::to_execution_session(&state);

        assert_eq!(session.id, "test-session-123");
        assert_eq!(session.original_request, "Find all rust files");
        assert_eq!(session.iteration_count, 0);
        assert_eq!(session.total_tokens, 0);
        assert!(session.context.is_some());
        assert!(!session.needs_compaction);
        assert_eq!(session.last_compaction_index, 0);
    }

    /// Test that UserInputPart is created from original request
    #[test]
    fn test_sync_creates_user_input_part() {
        let state = LoopState::new(
            "session-1".to_string(),
            "Hello, world!".to_string(),
            RequestContext::empty(),
        );

        let session = SessionSync::to_execution_session(&state);

        // Should have at least one part (the user input)
        assert!(!session.parts.is_empty());

        // First part should be UserInput
        match &session.parts[0] {
            SessionPart::UserInput(part) => {
                assert_eq!(part.text, "Hello, world!");
            }
            _ => panic!("Expected UserInput part as first part"),
        }
    }

    /// Test conversion of steps with tool calls
    #[test]
    fn test_steps_to_parts_tool_call() {
        let steps = vec![LoopStep {
            step_id: 0,
            observation_summary: String::new(),
            thinking: Thinking {
                reasoning: Some("I need to search for files".to_string()),
                decision: Decision::UseTool {
                    tool_name: "search".to_string(),
                    arguments: json!({"query": "*.rs"}),
                },
            },
            action: Action::ToolCall {
                tool_name: "search".to_string(),
                arguments: json!({"query": "*.rs"}),
            },
            result: ActionResult::ToolSuccess {
                output: json!(["file1.rs", "file2.rs"]),
                duration_ms: 100,
            },
            tokens_used: 500,
            duration_ms: 100,
        }];

        let parts = SessionSync::steps_to_parts(&steps, "Find rust files");

        // Should have: UserInput, AiResponse (reasoning), ToolCall
        assert_eq!(parts.len(), 3);

        // First: UserInput
        assert!(matches!(&parts[0], SessionPart::UserInput(_)));

        // Second: AiResponse with reasoning
        match &parts[1] {
            SessionPart::AiResponse(ai) => {
                assert_eq!(ai.reasoning, Some("I need to search for files".to_string()));
            }
            _ => panic!("Expected AiResponse part"),
        }

        // Third: ToolCall
        match &parts[2] {
            SessionPart::ToolCall(tc) => {
                assert_eq!(tc.tool_name, "search");
                assert_eq!(tc.status, ToolCallStatus::Completed);
                assert!(tc.output.is_some());
            }
            _ => panic!("Expected ToolCall part"),
        }
    }

    /// Test ActionResult to ToolCallStatus mapping
    #[test]
    fn test_action_result_to_status_mapping() {
        // ToolSuccess -> Completed
        let (status, output, error) =
            SessionSync::action_result_to_status(&ActionResult::ToolSuccess {
                output: json!("result"),
                duration_ms: 50,
            });
        assert_eq!(status, ToolCallStatus::Completed);
        assert!(output.is_some());
        assert!(error.is_none());

        // ToolError (retryable) -> Failed
        let (status, output, error) =
            SessionSync::action_result_to_status(&ActionResult::ToolError {
                error: "timeout".to_string(),
                retryable: true,
            });
        assert_eq!(status, ToolCallStatus::Failed);
        assert!(output.is_none());
        assert_eq!(error, Some("timeout".to_string()));

        // ToolError (non-retryable) -> Failed
        let (status, _, _) = SessionSync::action_result_to_status(&ActionResult::ToolError {
            error: "not found".to_string(),
            retryable: false,
        });
        assert_eq!(status, ToolCallStatus::Failed);

        // Completed -> Completed
        let (status, _, _) = SessionSync::action_result_to_status(&ActionResult::Completed);
        assert_eq!(status, ToolCallStatus::Completed);

        // Failed -> Failed
        let (status, _, _) = SessionSync::action_result_to_status(&ActionResult::Failed);
        assert_eq!(status, ToolCallStatus::Failed);
    }

    /// Test sync back to LoopState
    #[test]
    fn test_sync_to_loop_state() {
        let mut state = LoopState::new(
            "session-1".to_string(),
            "Test request".to_string(),
            RequestContext::empty(),
        );
        state.total_tokens = 100;

        let mut session = SessionSync::to_execution_session(&state);

        // Modify session
        session.total_tokens = 500;
        session.last_compaction_index = 3;
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Compressed history".to_string(),
            original_count: 3,
            compacted_at: chrono::Utc::now().timestamp(),
        }));

        // Sync back
        SessionSync::sync_to_loop_state(&session, &mut state);

        assert_eq!(state.total_tokens, 500);
        assert_eq!(state.compressed_until_step, 3);
        assert_eq!(state.history_summary, "Compressed history");
    }

    /// Test create_summary_part
    #[test]
    fn test_create_summary_part() {
        let mut state = LoopState::new(
            "session-1".to_string(),
            "Test".to_string(),
            RequestContext::empty(),
        );

        // No summary yet
        assert!(SessionSync::create_summary_part(&state).is_none());

        // Add summary
        state.history_summary = "Previous context was about X".to_string();
        state.compressed_until_step = 5;

        let part = SessionSync::create_summary_part(&state);
        assert!(part.is_some());

        match part.unwrap() {
            SessionPart::Summary(summary) => {
                assert_eq!(summary.content, "Previous context was about X");
                assert_eq!(summary.original_count, 5);
            }
            _ => panic!("Expected Summary part"),
        }
    }

    /// Test conversion with empty original request
    #[test]
    fn test_steps_to_parts_empty_request() {
        let parts = SessionSync::steps_to_parts(&[], "");
        assert!(parts.is_empty());
    }

    /// Test LoopState with steps and compression
    #[test]
    fn test_sync_with_compressed_state() {
        let mut state = LoopState::new(
            "session-compressed".to_string(),
            "Complex task".to_string(),
            RequestContext::empty(),
        );

        // Simulate some steps
        for i in 0..5 {
            state.steps.push(LoopStep {
                step_id: i,
                observation_summary: String::new(),
                thinking: Thinking {
                    reasoning: Some(format!("Step {} reasoning", i)),
                    decision: Decision::Complete {
                        summary: "done".to_string(),
                    },
                },
                action: Action::Completion {
                    summary: format!("Step {} done", i),
                },
                result: ActionResult::Completed,
                tokens_used: 100,
                duration_ms: 50,
            });
        }

        // Simulate compression
        state.history_summary = "Previous 3 steps summarized".to_string();
        state.compressed_until_step = 3;

        let session = SessionSync::to_execution_session(&state);

        assert!(session.needs_compaction);
        assert_eq!(session.last_compaction_index, 3);
    }

    /// Test RequestContext is preserved
    #[test]
    fn test_context_preservation() {
        let context = RequestContext {
            current_app: Some("Terminal".to_string()),
            working_directory: Some("/home/user".to_string()),
            selected_text: Some("selected code".to_string()),
            ..Default::default()
        };

        let state = LoopState::new("session-ctx".to_string(), "Task".to_string(), context);

        let session = SessionSync::to_execution_session(&state);

        let ctx = session.context.as_ref().unwrap();
        assert_eq!(ctx.current_app, Some("Terminal".to_string()));
        assert_eq!(ctx.working_directory, Some("/home/user".to_string()));
        assert_eq!(ctx.selected_text, Some("selected code".to_string()));
    }
}
