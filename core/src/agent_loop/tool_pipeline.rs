//! ToolPipeline — 6-stage hook-integrated tool execution pipeline.
//!
//! Stages:
//! 1. Build HookContext from tool call metadata
//! 2. Pre-hooks (interceptors): block or modify arguments before execution
//! 3. Safety check: blocked patterns and permission policy
//! 4. Execute tool with cancellation support
//! 5. Post-hooks (observers): inject additional context after success
//! 6. Failure hooks (observers): fire on error outcomes

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent_loop::safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
use crate::agent_loop::tool::{LoopToolRegistry, ToolResult};
use crate::agent_loop::tool_orchestrator::ToolOutcome;
use crate::extension::hooks::{HookContext, HookExecutor};
use crate::extension::HookEvent;
use crate::tool_output::compressor::compress_tool_output;

// =============================================================================
// PipelineOutcome
// =============================================================================

/// Extended outcome carrying hook-injected metadata alongside the core ToolOutcome.
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    /// The core tool result.
    pub outcome: ToolOutcome,
    /// Additional contexts injected by hooks (for LLM consumption).
    pub additional_contexts: Vec<String>,
    /// Whether hooks requested stopping the agent loop.
    pub prevent_continuation: bool,
    /// Messages from hooks to surface in conversation.
    pub hook_messages: Vec<String>,
}

// =============================================================================
// ToolPipeline
// =============================================================================

/// 6-stage hook-integrated tool execution pipeline.
pub struct ToolPipeline {
    hooks: Arc<HookExecutor>,
    safety: Arc<SafetyGuard>,
    session_id: String,
    working_dir: Option<PathBuf>,
}

impl ToolPipeline {
    /// Create a new pipeline.
    pub fn new(
        hooks: Arc<HookExecutor>,
        safety: Arc<SafetyGuard>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            hooks,
            safety,
            session_id: session_id.into(),
            working_dir: None,
        }
    }

    /// Set an optional working directory passed to hook commands.
    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = Some(dir);
        self
    }

    /// Access the underlying safety guard.
    pub fn safety(&self) -> &SafetyGuard {
        &self.safety
    }

    /// Access the underlying hook executor.
    pub fn hooks(&self) -> &HookExecutor {
        &self.hooks
    }

    /// Whether any hooks are registered.
    pub fn has_hooks(&self) -> bool {
        self.hooks.hook_count() > 0
    }

    // -------------------------------------------------------------------------
    // execute — 6-stage pipeline
    // -------------------------------------------------------------------------

    /// Execute a single tool call through the full 6-stage pipeline.
    pub async fn execute(
        &self,
        id: &str,
        name: &str,
        arguments: &Value,
        registry: &Arc<LoopToolRegistry>,
        cancel: &CancellationToken,
    ) -> PipelineOutcome {
        let mut additional_contexts: Vec<String> = Vec::new();
        let mut hook_messages: Vec<String> = Vec::new();
        let mut prevent_continuation = false;

        // -----------------------------------------------------------------
        // Stage 1: Build initial HookContext
        // -----------------------------------------------------------------
        let base_ctx = self.build_context(name, arguments);

        // -----------------------------------------------------------------
        // Stage 2: Pre-hooks (interceptors)
        // -----------------------------------------------------------------
        let effective_args = if self.has_hooks() {
            // Run interceptors — they can block or modify arguments.
            let (ctx_after, block_reason) = match self
                .hooks
                .execute_interceptors(HookEvent::BeforeToolCall, base_ctx.clone())
                .await
            {
                Ok(pair) => pair,
                Err(e) => {
                    // Interceptor infrastructure failure — treat as block.
                    let msg = format!("[HOOK_BLOCKED] Interceptor error: {}", e);
                    return self.blocked_outcome(id, name, msg);
                }
            };

            if let Some(reason) = block_reason {
                let msg = format!("[HOOK_BLOCKED] {}", reason);
                return self.blocked_outcome(id, name, msg);
            }

            // Run observer pre-hooks to collect messages and contexts.
            let pre_result = match self
                .hooks
                .execute(HookEvent::BeforeToolCall, &ctx_after)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(tool = name, error = %e, "Pre-hook execute failed");
                    // Non-fatal — continue with original arguments.
                    return self.run_from_safety(
                        id,
                        name,
                        arguments,
                        registry,
                        cancel,
                        additional_contexts,
                        hook_messages,
                        prevent_continuation,
                    )
                    .await;
                }
            };

            hook_messages.extend(pre_result.messages);
            additional_contexts.extend(pre_result.additional_contexts);
            if pre_result.prevent_continuation {
                prevent_continuation = true;
            }

            // Use hook-modified arguments if provided, otherwise originals.
            pre_result.updated_input.unwrap_or_else(|| arguments.clone())
        } else {
            arguments.clone()
        };

        // -----------------------------------------------------------------
        // Stage 3: Safety check
        // -----------------------------------------------------------------
        let safety_call = SafetyToolCall {
            name: name.to_string(),
            input: effective_args.clone(),
        };
        if let Err(e) = self.safety.check(&safety_call) {
            let msg = map_safety_error(&e);
            return PipelineOutcome {
                outcome: ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    output_text: msg,
                    is_error: true,
                    should_stop: false,
                    retryable: false,
                },
                additional_contexts,
                prevent_continuation,
                hook_messages,
            };
        }

        // -----------------------------------------------------------------
        // Stage 4: Execute tool with cancellation
        // -----------------------------------------------------------------
        let result = tokio::select! {
            r = registry.execute(name, effective_args.clone()) => r,
            _ = cancel.cancelled() => {
                return PipelineOutcome {
                    outcome: ToolOutcome {
                        tool_id: id.to_string(),
                        tool_name: name.to_string(),
                        output_text: "[CANCELLED] Tool execution was cancelled".to_string(),
                        is_error: true,
                        should_stop: false,
                        retryable: false,
                    },
                    additional_contexts,
                    prevent_continuation,
                    hook_messages,
                };
            }
        };

        let mut outcome = Self::map_result(id, name, &result);

        // -----------------------------------------------------------------
        // Stages 5 & 6: Post-hooks
        // -----------------------------------------------------------------
        if self.has_hooks() {
            let post_ctx = base_ctx
                .clone()
                .with_tool_output(&outcome.output_text)
                .with_tool_error(outcome.is_error);

            // Stage 5: AfterToolCall (always)
            match self
                .hooks
                .execute(HookEvent::AfterToolCall, &post_ctx)
                .await
            {
                Ok(post_result) => {
                    hook_messages.extend(post_result.messages);
                    additional_contexts.extend(post_result.additional_contexts);
                    if post_result.prevent_continuation {
                        prevent_continuation = true;
                    }
                }
                Err(e) => {
                    tracing::warn!(tool = name, error = %e, "Post-hook execute failed");
                }
            }

            // Stage 6: AfterToolCallFailure (only on error)
            if outcome.is_error {
                match self
                    .hooks
                    .execute(HookEvent::AfterToolCallFailure, &post_ctx)
                    .await
                {
                    Ok(fail_result) => {
                        hook_messages.extend(fail_result.messages);
                        additional_contexts.extend(fail_result.additional_contexts);
                        if fail_result.prevent_continuation {
                            prevent_continuation = true;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(tool = name, error = %e, "Failure hook execute failed");
                    }
                }
            }
        }

        if prevent_continuation {
            outcome.should_stop = true;
        }

        PipelineOutcome {
            outcome,
            additional_contexts,
            prevent_continuation,
            hook_messages,
        }
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    /// Build a `HookContext` from tool call parameters.
    fn build_context(&self, name: &str, arguments: &Value) -> HookContext {
        let args_str = arguments.to_string();

        let mut ctx = HookContext::new(&self.session_id)
            .with_tool_name(name)
            .with_arguments(&args_str);

        // Extract file_path from arguments if present.
        if let Some(path) = arguments
            .get("path")
            .or_else(|| arguments.get("file_path"))
            .and_then(|v| v.as_str())
        {
            ctx = ctx.with_file_path(path);
        }

        if let Some(ref dir) = self.working_dir {
            ctx = ctx.with_working_dir(dir.clone());
        }

        ctx
    }

    /// Map a `ToolResult` to a `ToolOutcome`.
    fn map_result(id: &str, name: &str, result: &ToolResult) -> ToolOutcome {
        match result {
            ToolResult::Success { output } => {
                let raw = value_to_text(output);
                let compressed = compress_tool_output(name, &raw);
                ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    output_text: compressed,
                    is_error: false,
                    should_stop: false,
                    retryable: false,
                }
            }
            ToolResult::Error { error, retryable } => ToolOutcome {
                tool_id: id.to_string(),
                tool_name: name.to_string(),
                output_text: error.clone(),
                is_error: true,
                should_stop: false,
                retryable: *retryable,
            },
            ToolResult::SuccessAndStopLoop { output } => {
                let raw = value_to_text(output);
                let compressed = compress_tool_output(name, &raw);
                ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    output_text: compressed,
                    is_error: false,
                    should_stop: true,
                    retryable: false,
                }
            }
        }
    }

    /// Produce a blocked (error) outcome without running the tool.
    fn blocked_outcome(&self, id: &str, name: &str, message: String) -> PipelineOutcome {
        PipelineOutcome {
            outcome: ToolOutcome {
                tool_id: id.to_string(),
                tool_name: name.to_string(),
                output_text: message,
                is_error: true,
                should_stop: false,
                retryable: false,
            },
            additional_contexts: Vec::new(),
            prevent_continuation: false,
            hook_messages: Vec::new(),
        }
    }

    /// Run stages 3-6 directly (used when pre-hook infrastructure fails non-fatally).
    #[allow(clippy::too_many_arguments)]
    async fn run_from_safety(
        &self,
        id: &str,
        name: &str,
        arguments: &Value,
        registry: &Arc<LoopToolRegistry>,
        cancel: &CancellationToken,
        mut additional_contexts: Vec<String>,
        mut hook_messages: Vec<String>,
        mut prevent_continuation: bool,
    ) -> PipelineOutcome {
        let safety_call = SafetyToolCall {
            name: name.to_string(),
            input: arguments.clone(),
        };
        if let Err(e) = self.safety.check(&safety_call) {
            let msg = map_safety_error(&e);
            return PipelineOutcome {
                outcome: ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    output_text: msg,
                    is_error: true,
                    should_stop: false,
                    retryable: false,
                },
                additional_contexts,
                prevent_continuation,
                hook_messages,
            };
        }

        let result = tokio::select! {
            r = registry.execute(name, arguments.clone()) => r,
            _ = cancel.cancelled() => {
                return PipelineOutcome {
                    outcome: ToolOutcome {
                        tool_id: id.to_string(),
                        tool_name: name.to_string(),
                        output_text: "[CANCELLED] Tool execution was cancelled".to_string(),
                        is_error: true,
                        should_stop: false,
                        retryable: false,
                    },
                    additional_contexts,
                    prevent_continuation,
                    hook_messages,
                };
            }
        };

        let mut outcome = Self::map_result(id, name, &result);

        if self.has_hooks() {
            let post_ctx = self
                .build_context(name, arguments)
                .with_tool_output(&outcome.output_text)
                .with_tool_error(outcome.is_error);

            if let Ok(post_result) = self
                .hooks
                .execute(HookEvent::AfterToolCall, &post_ctx)
                .await
            {
                hook_messages.extend(post_result.messages);
                additional_contexts.extend(post_result.additional_contexts);
                if post_result.prevent_continuation {
                    prevent_continuation = true;
                }
            }

            if outcome.is_error {
                if let Ok(fail_result) = self
                    .hooks
                    .execute(HookEvent::AfterToolCallFailure, &post_ctx)
                    .await
                {
                    hook_messages.extend(fail_result.messages);
                    additional_contexts.extend(fail_result.additional_contexts);
                    if fail_result.prevent_continuation {
                        prevent_continuation = true;
                    }
                }
            }
        }

        if prevent_continuation {
            outcome.should_stop = true;
        }

        PipelineOutcome {
            outcome,
            additional_contexts,
            prevent_continuation,
            hook_messages,
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Map a `SafetyError` to a human-readable error string.
fn map_safety_error(e: &SafetyError) -> String {
    match e {
        SafetyError::Blocked { tool, pattern } => {
            format!(
                "[BLOCKED] Tool '{}' blocked by safety pattern '{}'",
                tool, pattern
            )
        }
        SafetyError::NeedsConfirmation { tool } => {
            format!(
                "[NEEDS_CONFIRMATION] Tool '{}' requires user confirmation",
                tool
            )
        }
        SafetyError::PolicyDenied { tool } => {
            format!("[DENIED] Tool '{}' denied by policy", tool)
        }
    }
}

/// Convert a JSON Value to a display string.
fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::tool::{LoopTool, LoopToolRegistry, ToolResult};
    use crate::extension::hooks::HookExecutor;
    use crate::extension::{HookAction, HookConfig, HookEvent, HookKind, HookPriority, PermissionAction};
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::path::PathBuf;

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
                kind: HookKind::Observer,
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
            outcome.additional_contexts.contains(&"pre-hook fired".to_string()),
            "expected pre-hook fired in contexts: {:?}",
            outcome.additional_contexts
        );
        assert!(
            outcome.additional_contexts.contains(&"post-hook fired".to_string()),
            "expected post-hook fired in contexts: {:?}",
            outcome.additional_contexts
        );
    }

    #[tokio::test]
    async fn pipeline_update_input_modifies_arguments() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Observer,
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
            outcome.additional_contexts.contains(&"failure observed".to_string()),
            "expected failure observed in contexts: {:?}",
            outcome.additional_contexts
        );
    }
}
