// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    
    use crate::tools::pipeline::{MAX_TOOL_RESULT_TOKENS, TRUNCATION_SUFFIX};
    use crate::tools::pipeline::helpers::{default_result_budget, truncate_tool_result, truncate_tool_result_with_budget};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;
    use serde_json::{json, Value};
    use async_trait::async_trait;
    use crate::extension::hooks::HookExecutor;
    use crate::extension::{
        HookAction, HookConfig, HookEvent, HookKind, HookPriority, PermissionAction,
    };
    use crate::session::ingress_safety::SafetyGuard;
    use crate::tools::pipeline::ToolPipeline;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};

    struct EchoTool;

    #[async_trait]
    impl LoopTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input"
        }
        fn schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn execute(&self, input: Value) -> ToolResult {
            ToolResult::Success { output: input }
        }
    }

    fn permissive_guard() -> SafetyGuard {
        SafetyGuard::new(vec![], HashMap::new(), PermissionAction::Allow)
    }

    fn empty_pipeline() -> ToolPipeline {
        ToolPipeline::new(
            Arc::new(HookExecutor::empty()),
            Arc::new(permissive_guard()),
            "test-session",
        )
    }

    #[tokio::test]
    async fn pipeline_executes_tool_without_hooks() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();
        let pipeline = empty_pipeline();

        let outcome = pipeline
            .execute("call1", "echo", &json!({"msg": "hi"}), &registry, &cancel)
            .await;

        assert!(!outcome.outcome.is_error);
        assert!(outcome.outcome.output_text.contains("hi"));
        assert!(outcome.additional_contexts.is_empty());
        assert!(!outcome.prevent_continuation);
    }

    #[tokio::test]
    async fn pipeline_pre_hook_blocks_execution() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'block: forbidden'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline
            .execute("call1", "echo", &json!({}), &registry, &cancel)
            .await;
        assert!(outcome.outcome.is_error);
        assert!(outcome.outcome.output_text.contains("forbidden"));
    }

    #[tokio::test]
    async fn pipeline_post_hook_injects_context() {
        let hooks = vec![HookConfig {
            event: HookEvent::AfterToolCall,
            kind: HookKind::Observer,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'context: auto-formatted'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline
            .execute("call1", "echo", &json!({"x": 1}), &registry, &cancel)
            .await;
        assert!(!outcome.outcome.is_error);
        assert_eq!(outcome.additional_contexts, vec!["auto-formatted"]);
    }

    #[tokio::test]
    async fn pipeline_empty_hooks_zero_overhead() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();
        let pipeline = empty_pipeline();

        let outcome = pipeline
            .execute("call1", "echo", &json!({"a": "b"}), &registry, &cancel)
            .await;
        assert!(!outcome.outcome.is_error);
        assert!(outcome.hook_messages.is_empty());
    }

    // -------------------------------------------------------------------------
    // Integration tests — full pipeline round trip
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn pipeline_full_round_trip_with_hooks() {
        let hooks = vec![
            HookConfig {
                event: HookEvent::BeforeToolCall,
                kind: HookKind::Interceptor,
                priority: HookPriority::default(),
                matcher: Some("echo".to_string()),
                actions: vec![HookAction::Command {
                    command: "echo 'context: pre-hook fired'".to_string(),
                }],
                plugin_name: "test".to_string(),
                plugin_root: PathBuf::from("/tmp"),
                handler: None,
            },
            HookConfig {
                event: HookEvent::AfterToolCall,
                kind: HookKind::Observer,
                priority: HookPriority::default(),
                matcher: Some("echo".to_string()),
                actions: vec![HookAction::Command {
                    command: "echo 'context: post-hook fired'".to_string(),
                }],
                plugin_name: "test".to_string(),
                plugin_root: PathBuf::from("/tmp"),
                handler: None,
            },
        ];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline
            .execute("c1", "echo", &json!({"data": "test"}), &registry, &cancel)
            .await;

        assert!(!outcome.outcome.is_error);
        assert!(outcome.outcome.output_text.contains("test"));
        assert!(
            outcome
                .additional_contexts
                .contains(&"pre-hook fired".to_string()),
            "expected pre-hook fired in contexts: {:?}",
            outcome.additional_contexts
        );
        assert!(
            outcome
                .additional_contexts
                .contains(&"post-hook fired".to_string()),
            "expected post-hook fired in contexts: {:?}",
            outcome.additional_contexts
        );
    }

    #[tokio::test]
    async fn pipeline_update_input_modifies_arguments() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: r#"echo 'update_input: {"injected": true}'"#.to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline
            .execute("c1", "echo", &json!({"original": true}), &registry, &cancel)
            .await;

        assert!(!outcome.outcome.is_error);
        // Echo tool returns its input — should contain the hook-injected field.
        assert!(
            outcome.outcome.output_text.contains("injected"),
            "expected injected field in output: {}",
            outcome.outcome.output_text
        );
    }

    #[tokio::test]
    async fn pipeline_rejects_missing_required_field() {
        struct StrictTool;

        #[async_trait]
        impl LoopTool for StrictTool {
            fn name(&self) -> &str {
                "strict"
            }
            fn description(&self) -> &str {
                "Requires 'path' field"
            }
            fn schema(&self) -> Value {
                json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": { "type": "string" }
                    }
                })
            }
            async fn execute(&self, _input: Value) -> ToolResult {
                ToolResult::Success {
                    output: json!("ok"),
                }
            }
        }

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(StrictTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();
        let pipeline = empty_pipeline();

        let outcome = pipeline
            .execute(
                "c1",
                "strict",
                &json!({"other": "value"}),
                &registry,
                &cancel,
            )
            .await;

        assert!(outcome.outcome.is_error);
        assert!(
            outcome
                .outcome
                .output_text
                .contains("missing required field"),
            "expected validation error, got: {}",
            outcome.outcome.output_text
        );
    }

    #[tokio::test]
    async fn pipeline_passes_validation_when_no_required() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();
        let pipeline = empty_pipeline();

        let outcome = pipeline
            .execute("c1", "echo", &json!({}), &registry, &cancel)
            .await;

        assert!(!outcome.outcome.is_error);
    }

    #[tokio::test]
    async fn pipeline_rejects_non_object_input() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();
        let pipeline = empty_pipeline();

        let outcome = pipeline
            .execute("c1", "echo", &json!("not an object"), &registry, &cancel)
            .await;

        assert!(outcome.outcome.is_error);
        assert!(outcome.outcome.output_text.contains("expected JSON object"));
    }

    #[tokio::test]
    async fn pipeline_deny_produces_non_retryable_error() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'deny: policy forbids this tool'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline
            .execute("c1", "echo", &json!({}), &registry, &cancel)
            .await;

        assert!(outcome.outcome.is_error);
        assert!(
            outcome.outcome.output_text.contains("[HOOK_DENIED]"),
            "expected HOOK_DENIED, got: {}",
            outcome.outcome.output_text
        );
        assert!(!outcome.outcome.retryable, "deny should not be retryable");
    }

    #[tokio::test]
    async fn pipeline_post_hook_updates_output() {
        let hooks = vec![HookConfig {
            event: HookEvent::AfterToolCall,
            kind: HookKind::Observer,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'update_output: [REDACTED]'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline
            .execute("c1", "echo", &json!({"secret": "key"}), &registry, &cancel)
            .await;

        assert!(!outcome.outcome.is_error);
        assert_eq!(
            outcome.outcome.output_text, "[REDACTED]",
            "post-hook should have replaced output"
        );
    }

    #[test]
    fn truncate_short_result_unchanged() {
        let short = "Hello, this is a short result.";
        assert_eq!(truncate_tool_result(short), short);
    }

    #[test]
    fn truncate_large_result_truncated() {
        // Generate a string that's clearly over 8000 tokens
        // At ~2.5 chars/token, 8000 tokens ≈ 20000 chars. Use 30000 chars.
        let large = "x".repeat(30_000);
        let result = truncate_tool_result(&large);
        assert!(result.len() < large.len(), "result should be truncated");
        assert!(
            result.ends_with(TRUNCATION_SUFFIX),
            "should end with truncation suffix"
        );
    }

    #[test]
    fn truncate_preserves_newline_boundary() {
        // Build a string with newlines, large enough to trigger truncation
        let mut lines = String::new();
        for i in 0..1000 {
            lines.push_str(&format!("Line {}: some content here to fill up space\n", i));
        }
        let result = truncate_tool_result(&lines);
        if result.len() < lines.len() {
            // The text before the suffix should end at a newline
            let before_suffix = result.trim_end_matches(TRUNCATION_SUFFIX);
            assert!(
                before_suffix.ends_with('\n'),
                "should truncate at newline boundary"
            );
        }
    }

    #[tokio::test]
    async fn pipeline_failure_hooks_fire_on_error() {
        struct FailTool;

        #[async_trait]
        impl LoopTool for FailTool {
            fn name(&self) -> &str {
                "fail"
            }
            fn description(&self) -> &str {
                "Always fails"
            }
            fn schema(&self) -> Value {
                json!({"type": "object"})
            }
            async fn execute(&self, _input: Value) -> ToolResult {
                ToolResult::Error {
                    error: "boom".to_string(),
                    retryable: false,
                }
            }
        }

        let hooks = vec![HookConfig {
            event: HookEvent::AfterToolCallFailure,
            kind: HookKind::Observer,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'context: failure observed'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(FailTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();

        let pipeline = ToolPipeline::new(
            Arc::new(HookExecutor::new(hooks)),
            Arc::new(permissive_guard()),
            "test-session",
        );

        let outcome = pipeline
            .execute("c1", "fail", &json!({}), &registry, &cancel)
            .await;

        assert!(outcome.outcome.is_error);
        assert!(
            outcome
                .additional_contexts
                .contains(&"failure observed".to_string()),
            "expected failure observed in contexts: {:?}",
            outcome.additional_contexts
        );
    }

    #[tokio::test]
    async fn test_hook_ask_plus_safety_ask_converge_to_confirmation() {
        // SafetyGuard classifies "shell" as Ask
        let perms = [("shell".to_string(), PermissionAction::Ask)]
            .into_iter()
            .collect();
        let safety = Arc::new(SafetyGuard::new(vec![], perms, PermissionAction::Allow));

        // Hook emits Ask decision
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'ask: confirm dangerous operation'".to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];
        let hook_exec = Arc::new(HookExecutor::new(hooks));

        let pipeline = ToolPipeline::new(hook_exec, safety, "test-session");
        let cancel = CancellationToken::new();

        // Need a tool registered as "shell"
        struct ShellTool;
        #[async_trait]
        impl LoopTool for ShellTool {
            fn name(&self) -> &str {
                "shell"
            }
            fn description(&self) -> &str {
                "shell tool"
            }
            fn schema(&self) -> Value {
                json!({"type": "object", "properties": {}})
            }
            async fn execute(&self, _input: Value) -> ToolResult {
                ToolResult::Success {
                    output: json!("ok"),
                }
            }
        }

        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(ShellTool));
        let registry = Arc::new(registry);

        let outcome = pipeline
            .execute(
                "t1",
                "shell",
                &json!({"command": "echo hi"}),
                &registry,
                &cancel,
            )
            .await;

        // Key assertion: needs_user_confirmation is true, NOT an error
        assert!(outcome.needs_user_confirmation, "should need confirmation");
        assert!(outcome.confirmation_reason.is_some(), "should have reason");
        // The tool should NOT have been blocked
        assert!(
            !outcome.outcome.output_text.starts_with("[DENIED]"),
            "should not be denied, got: {}",
            outcome.outcome.output_text
        );
        assert!(
            !outcome.outcome.output_text.starts_with("[BLOCKED]"),
            "should not be blocked"
        );
    }

    #[test]
    fn truncate_with_budget_preserves_head_and_tail() {
        let mut lines = String::new();
        for i in 0..2000 {
            lines.push_str(&format!(
                "Line {:04}: content padding here to fill tokens\n",
                i
            ));
        }
        let result = truncate_tool_result_with_budget(&lines, 4000);
        assert!(result.len() < lines.len(), "should be truncated");
        assert!(result.contains("Line 0000"), "should preserve head");
        assert!(result.contains("Line 1999"), "should preserve tail");
        assert!(result.contains("truncated"), "should have marker");
    }

    #[test]
    fn truncate_with_budget_within_limit_unchanged() {
        let short = "Hello, short result.";
        assert_eq!(truncate_tool_result_with_budget(short, 8000), short);
    }

    #[test]
    fn default_result_budget_returns_correct_values() {
        assert_eq!(default_result_budget("Read"), 12_000);
        assert_eq!(default_result_budget("Grep"), 6_000);
        assert_eq!(default_result_budget("Bash"), 8_000);
        assert_eq!(default_result_budget("WebFetch"), 10_000);
        assert_eq!(default_result_budget("Unknown"), MAX_TOOL_RESULT_TOKENS);
    }
}
