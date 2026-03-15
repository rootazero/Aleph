//! LLM-Driven Summarization tests (Task 4)

use super::helpers::create_test_session;
use crate::components::session_compactor::*;
use crate::components::types::{
    ExecutionSession, SessionPart, SummaryPart, ToolCallPart, ToolCallStatus,
    UserInputPart,
};
use serde_json::json;

    #[test]
    fn test_compaction_prompt_exists() {
        let prompt = super::super::compaction_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("summarizing conversations"));
        assert!(prompt.contains("What was done"));
        assert!(prompt.contains("What needs to be done next"));
    }

    #[test]
    fn test_build_summary_context_basic() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let context = compactor.build_summary_context(&session);

        // Should contain user input
        assert!(context.contains("User:"), "Context should contain 'User:' prefix");
        assert!(context.contains("Please help me analyze this code"), "Context should contain user request");

        // Should contain tool information
        assert!(context.contains("Tool"), "Context should contain tool calls");
        assert!(context.contains("completed"), "Context should show tool completion status");
    }

    #[test]
    fn test_build_summary_context_with_ai_response() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let context = compactor.build_summary_context(&session);

        // Should contain AI response
        assert!(context.contains("Assistant:"), "Context should contain 'Assistant:' prefix");
        assert!(context.contains("Analysis complete"), "Context should contain AI response content");
    }

    #[test]
    fn test_build_summary_context_truncates_long_output() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add tool call with output > 200 chars
        let long_output = "x".repeat(500);
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: "long-output-tool".to_string(),
            tool_name: "test_tool".to_string(),
            input: json!({"param": "value"}),
            status: ToolCallStatus::Completed,
            output: Some(long_output),
            error: None,
            started_at: 1100,
            completed_at: Some(1200),
        }));

        let context = compactor.build_summary_context(&session);

        // Should be truncated with "..."
        assert!(context.contains("..."), "Long output should be truncated with '...'");
        // Original 500 char output should not appear in full
        assert!(!context.contains(&"x".repeat(500)), "Full 500 char output should not appear");
        // But truncated 200 chars should appear
        assert!(context.contains(&"x".repeat(200)), "Truncated 200 chars should appear");
    }

    #[test]
    fn test_build_summary_context_with_failed_tool() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add failed tool call
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: "failed-tool".to_string(),
            tool_name: "failing_tool".to_string(),
            input: json!({}),
            status: ToolCallStatus::Failed,
            output: None,
            error: Some("Connection timeout".to_string()),
            started_at: 1100,
            completed_at: Some(1200),
        }));

        let context = compactor.build_summary_context(&session);

        // Should show failed status
        assert!(context.contains("failed"), "Context should show 'failed' for tool with error");
    }

    #[test]
    fn test_build_summary_context_with_summary_part() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add previous summary
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Previous session worked on feature X".to_string(),
            original_count: 50,
            compacted_at: 1000,
        }));

        // Add new user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Continue with feature X".to_string(),
            context: None,
            timestamp: 2000,
        }));

        let context = compactor.build_summary_context(&session);

        // Should contain previous summary
        assert!(context.contains("[Previous Summary]:"), "Context should contain previous summary marker");
        assert!(context.contains("Previous session worked on feature X"));
    }

    #[test]
    fn test_build_summary_context_empty_session() {
        let compactor = SessionCompactor::new();
        let session = ExecutionSession::new().with_model("gpt-4-turbo");

        let context = compactor.build_summary_context(&session);

        // Should be empty for empty session
        assert!(context.is_empty(), "Empty session should produce empty context");
    }

    #[tokio::test]
    async fn test_generate_llm_summary_without_callback() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Without callback, should fall back to template-based summary
        let summary = compactor.generate_llm_summary(&session, None).await;

        // Should contain template-based summary elements
        assert!(summary.contains("Original Request"));
        assert!(summary.contains("Please help me analyze this code"));
    }

    #[tokio::test]
    async fn test_generate_llm_summary_with_callback() {
        use crate::sync_primitives::Arc;
        use crate::sync_primitives::{AtomicBool, Ordering};

        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Track if callback was called
        let callback_called = Arc::new(AtomicBool::new(false));
        let callback_called_clone = callback_called.clone();

        // Create mock callback
        let callback: super::super::LlmCallback = Box::new(move |system_prompt, user_content| {
            let called = callback_called_clone.clone();
            Box::pin(async move {
                called.store(true, Ordering::SeqCst);

                // Verify prompts
                assert!(system_prompt.contains("summarizing conversations"));
                assert!(user_content.contains("conversation to summarize"));

                Ok("LLM generated summary: The user is analyzing code.".to_string())
            })
        });

        let summary = compactor.generate_llm_summary(&session, Some(&callback)).await;

        assert!(callback_called.load(Ordering::SeqCst), "Callback should be called");
        assert!(summary.contains("LLM generated summary"));
    }

    #[tokio::test]
    async fn test_generate_llm_summary_fallback_on_error() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Create callback that returns error
        let callback: super::super::LlmCallback = Box::new(|_system_prompt, _user_content| {
            Box::pin(async move {
                Err("LLM service unavailable".to_string())
            })
        });

        let summary = compactor.generate_llm_summary(&session, Some(&callback)).await;

        // Should fall back to template-based summary
        assert!(summary.contains("Original Request"), "Should fall back to template on error");
        assert!(summary.contains("Please help me analyze this code"));
    }

    #[test]
    fn test_build_summary_context_with_reasoning() {
        use crate::components::types::ReasoningPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add reasoning part
        session.parts.push(SessionPart::Reasoning(ReasoningPart {
            content: "Thinking through the problem...".to_string(),
            step: 0,
            is_complete: true,
            timestamp: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Reasoning]:"), "Context should contain reasoning marker");
        assert!(context.contains("Thinking through the problem"));
    }

    #[test]
    fn test_build_summary_context_with_plan() {
        use crate::components::types::PlanPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add plan
        session.parts.push(SessionPart::PlanCreated(PlanPart {
            plan_id: "plan-123".to_string(),
            steps: vec![
                crate::components::types::PlanStep {
                    step_id: "step-1".to_string(),
                    description: "Step 1".to_string(),
                    status: crate::components::types::StepStatus::Pending,
                    dependencies: vec![],
                },
                crate::components::types::PlanStep {
                    step_id: "step-2".to_string(),
                    description: "Step 2".to_string(),
                    status: crate::components::types::StepStatus::Pending,
                    dependencies: vec![],
                },
                crate::components::types::PlanStep {
                    step_id: "step-3".to_string(),
                    description: "Step 3".to_string(),
                    status: crate::components::types::StepStatus::Pending,
                    dependencies: vec![],
                },
            ],
            requires_confirmation: false,
            created_at: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Plan Created]:"), "Context should contain plan marker");
        assert!(context.contains("plan-123"));
        assert!(context.contains("Step 1, Step 2, Step 3"));
    }

    #[test]
    fn test_build_summary_context_with_subagent() {
        use crate::components::types::SubAgentPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add subagent call with result
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "code-review-agent".to_string(),
            prompt: "Review this code".to_string(),
            result: Some("Code looks good with minor suggestions".to_string()),
            timestamp: 1100,
        }));

        // Add subagent call without result (pending)
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "test-agent".to_string(),
            prompt: "Run tests".to_string(),
            result: None,
            timestamp: 1300,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[SubAgent code-review-agent]:"));
        assert!(context.contains("Review this code"));
        assert!(context.contains("Code looks good"));
        assert!(context.contains("[pending]"), "Pending subagent should show [pending]");
    }

    #[test]
    fn test_build_summary_context_subagent_truncates_long_result() {
        use crate::components::types::SubAgentPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add subagent call with long result
        let long_result = "x".repeat(500);
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "analysis-agent".to_string(),
            prompt: "Analyze".to_string(),
            result: Some(long_result),
            timestamp: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        // Should be truncated
        assert!(context.contains("..."), "Long subagent result should be truncated");
        assert!(!context.contains(&"x".repeat(500)), "Full result should not appear");
    }
