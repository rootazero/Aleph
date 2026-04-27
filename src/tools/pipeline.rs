// TODO(Phase 5): move to Orchestrator. See docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md §8 Phase 5.
// This file is deeply coupled to LoopToolRegistry (loop-internal tool trait) and loop primitives
// (FileContentTracker, ToolResultStore, RwLock<String> session_id). In Phase 5/6, LoopToolRegistry
// is retired and all tools route through ToolService; this pipeline is then replaced by ToolService
// middleware (PermissionLayer + hook middleware), and this file is deleted.

//! ToolPipeline — 7-stage hook-integrated tool execution pipeline.
//!
//! Stages:
//! 1. Build HookContext from tool call metadata
//! 2. Input schema validation (fast-fail before hooks)
//! 3. Pre-hooks (interceptors): block, deny, or modify arguments before execution
//! 4. Safety check: blocked patterns and permission policy
//! 5. Execute tool with cancellation support
//! 6. Post-hooks (observers): inject additional context or modify output after success
//! 7. Failure hooks (observers): fire on error outcomes

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::compact::file_content_tracker::FileContentTracker;
use crate::extension::hooks::{HookContext, HookExecutor, PermissionDecision};
use crate::extension::HookEvent;
use crate::session::ingress_safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
use crate::tool_output::compressor::compress_tool_output;
use crate::tools::orchestrator::ToolOutcome;
use crate::tools::result_store::ToolResultStore;
use crate::tools::runtime::{LoopToolRegistry, ToolResult};

/// Maximum tool result size in estimated tokens. Results exceeding this are truncated
/// to prevent a single tool call from consuming a disproportionate share of the context window.
const MAX_TOOL_RESULT_TOKENS: usize = 8_000;

const TRUNCATION_SUFFIX: &str = "\n... [output truncated, showing first ~8000 tokens]";

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
    /// If true, execution was paused pending user confirmation.
    pub needs_user_confirmation: bool,
    /// Reason for requiring confirmation (from hook Ask decision).
    pub confirmation_reason: Option<String>,
}

// =============================================================================
// ToolPipeline
// =============================================================================

/// 7-stage hook-integrated tool execution pipeline.
pub struct ToolPipeline {
    hooks: Arc<HookExecutor>,
    safety: Arc<SafetyGuard>,
    session_id: RwLock<String>,
    working_dir: Option<PathBuf>,
    result_store: Option<ToolResultStore>,
    file_tracker: Option<Arc<FileContentTracker>>,
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
            session_id: RwLock::new(session_id.into()),
            working_dir: None,
            result_store: None,
            file_tracker: None,
        }
    }

    /// Set an optional working directory passed to hook commands.
    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = Some(dir);
        self
    }

    /// Attach a `ToolResultStore` for disk-offloading large tool results.
    pub fn with_result_store(mut self, store: ToolResultStore) -> Self {
        self.result_store = Some(store);
        self
    }

    /// Attach a `FileContentTracker` for post-compaction file content recovery.
    pub fn with_file_tracker(mut self, tracker: Arc<FileContentTracker>) -> Self {
        self.file_tracker = Some(tracker);
        self
    }

    /// Update the session identifier used for hook contexts.
    pub fn set_session_id(&self, session_id: impl Into<String>) {
        let mut current = self.session_id.write().unwrap_or_else(|e| e.into_inner());
        *current = session_id.into();
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
    #[tracing::instrument(
        name = "tool_pipeline",
        skip(self, arguments, registry, cancel),
        fields(tool_name = %name, tool_id = %id)
    )]
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
        let mut needs_user_confirmation = false;
        let mut confirmation_reason: Option<String> = None;
        let mut skip_safety_patterns = false;

        // -----------------------------------------------------------------
        // Stage 1: Build initial HookContext
        // -----------------------------------------------------------------
        let base_ctx = self.build_context(name, arguments);

        // -----------------------------------------------------------------
        // Stage 2: Input schema validation (fast-fail before hooks)
        // -----------------------------------------------------------------
        {
            let _span =
                tracing::info_span!("pipeline.validate", tool = name, tool_id = id).entered();
            if let Some(tool) = registry.resolve(name) {
                let schema = tool.schema();
                if let Err(msg) = validate_input_fast(&schema, arguments) {
                    return PipelineOutcome {
                        outcome: ToolOutcome {
                            tool_id: id.to_string(),
                            tool_name: name.to_string(),
                            duration_ms: 0,
                            output_text: format!("[VALIDATION_ERROR] {}", msg),
                            is_error: true,
                            should_stop: false,
                            retryable: true,
                        },
                        additional_contexts: Vec::new(),
                        prevent_continuation: false,
                        hook_messages: Vec::new(),
                        needs_user_confirmation: false,
                        confirmation_reason: None,
                    };
                }
            }
        }

        // -----------------------------------------------------------------
        // Stage 3: Pre-hooks (interceptors)
        // -----------------------------------------------------------------
        let mut hook_decision_str: Option<&str> = None;

        let effective_args = if self.has_hooks() {
            let hooks_span = tracing::info_span!("pipeline.pre_hooks", tool = name);
            // Run interceptors — they can block or modify arguments.
            let (ctx_after, interceptor_result) = match self
                .hooks
                .execute_interceptors(HookEvent::BeforeToolCall, base_ctx.clone())
                .instrument(hooks_span.clone())
                .await
            {
                Ok(pair) => pair,
                Err(e) => {
                    let msg = format!("[HOOK_BLOCKED] Interceptor error: {}", e);
                    return self.blocked_outcome(id, name, msg);
                }
            };
            // Resolve permission decision (new field takes precedence over legacy)
            let decision = interceptor_result.permission_decision.clone().or_else(|| {
                if interceptor_result.denied {
                    Some(PermissionDecision::Deny {
                        reason: interceptor_result.deny_reason.clone().unwrap_or_default(),
                    })
                } else if interceptor_result.blocked {
                    Some(PermissionDecision::Block {
                        reason: interceptor_result.block_reason.clone().unwrap_or_default(),
                    })
                } else {
                    None
                }
            });

            match decision {
                Some(PermissionDecision::Deny { reason }) => {
                    return PipelineOutcome {
                        outcome: ToolOutcome {
                            tool_id: id.to_string(),
                            tool_name: name.to_string(),
                            duration_ms: 0,
                            output_text: format!("[HOOK_DENIED] {}", reason),
                            is_error: true,
                            should_stop: false,
                            retryable: false,
                        },
                        additional_contexts: Vec::new(),
                        prevent_continuation: false,
                        hook_messages: Vec::new(),
                        needs_user_confirmation: false,
                        confirmation_reason: None,
                    };
                }
                Some(PermissionDecision::Block { reason }) => {
                    let msg = format!("[HOOK_BLOCKED] {}", reason);
                    return self.blocked_outcome(id, name, msg);
                }
                Some(PermissionDecision::Ask { reason }) => {
                    hook_decision_str = Some("ask");
                    // NOTE: Ask does NOT block execution. The tool runs normally
                    // and PipelineOutcome carries the confirmation request back to
                    // the caller (agent loop). The caller decides whether to surface
                    // the result to the user or request confirmation first.
                    needs_user_confirmation = true;
                    confirmation_reason = Some(reason);
                }
                Some(PermissionDecision::Allow) => {
                    hook_decision_str = Some("allow");
                    skip_safety_patterns = true;
                }
                None => {}
            }

            // Collect interceptor outputs (messages, contexts, prevent_continuation).
            hook_messages.extend(interceptor_result.messages);
            additional_contexts.extend(interceptor_result.additional_contexts);
            if interceptor_result.prevent_continuation {
                prevent_continuation = true;
            }

            // Run observer-only pre-hooks (fire-and-forget, no duplicate interceptors).
            self.hooks
                .execute_observers(HookEvent::BeforeToolCall, &ctx_after)
                .await;

            // Use interceptor-modified arguments if provided, otherwise originals.
            interceptor_result
                .updated_input
                .unwrap_or_else(|| arguments.clone())
        } else {
            arguments.clone()
        };

        // -----------------------------------------------------------------
        // Stage 4: Safety check
        // -----------------------------------------------------------------
        {
            let _span = tracing::info_span!("pipeline.safety", tool = name).entered();
            let safety_call = SafetyToolCall {
                name: name.to_string(),
                input: effective_args.clone(),
            };
            let safety_result = if skip_safety_patterns {
                self.safety.check_permissions_only(&safety_call)
            } else {
                self.safety.check(&safety_call)
            };
            if let Err(e) = safety_result {
                match e {
                    SafetyError::NeedsConfirmation { ref tool } => {
                        // Safety agrees tool needs confirmation.
                        // Don't return error — route through confirmation flow.
                        if !needs_user_confirmation {
                            needs_user_confirmation = true;
                            confirmation_reason =
                                Some(format!("Tool '{}' is classified as high-risk", tool));
                        }
                        tracing::debug!(
                            tool = name,
                            "safety NeedsConfirmation routed to confirmation flow"
                        );
                    }
                    _ => {
                        // Blocked or PolicyDenied — hard stop
                        let msg = map_safety_error(&e);
                        return PipelineOutcome {
                            outcome: ToolOutcome {
                                tool_id: id.to_string(),
                                tool_name: name.to_string(),
                                duration_ms: 0,
                                output_text: msg,
                                is_error: true,
                                should_stop: false,
                                retryable: false,
                            },
                            additional_contexts,
                            prevent_continuation,
                            hook_messages,
                            needs_user_confirmation: false,
                            confirmation_reason: None,
                        };
                    }
                }
            }
        }

        let final_action = if needs_user_confirmation {
            "confirm"
        } else {
            "execute"
        };
        tracing::info!(
            tool = name,
            hook_decision = hook_decision_str.unwrap_or("none"),
            safety_passed = true,
            final_action = final_action,
            "permission resolved"
        );

        // -----------------------------------------------------------------
        // Stage 5: Execute tool with cancellation
        // -----------------------------------------------------------------
        let exec_span = tracing::info_span!("pipeline.execute", tool = name, tool_id = id);
        let exec_start = Instant::now();
        let result = tokio::select! {
            r = registry.execute(name, effective_args.clone()).instrument(exec_span) => r,
            _ = cancel.cancelled() => {
                return PipelineOutcome {
                    outcome: ToolOutcome {
                        tool_id: id.to_string(),
                        tool_name: name.to_string(),
                        duration_ms: 0,
                        output_text: "[CANCELLED] Tool execution was cancelled".to_string(),
                        is_error: true,
                        should_stop: false,
                        retryable: false,
                    },
                    additional_contexts,
                    prevent_continuation,
                    hook_messages,
                    needs_user_confirmation: false,
                    confirmation_reason: None,
                };
            }
        };
        let exec_elapsed_ms = exec_start.elapsed().as_millis() as u64;

        let budget = default_result_budget(name);
        let mut outcome = Self::map_result(id, name, &result, self.result_store.as_ref(), budget);
        outcome.duration_ms = exec_elapsed_ms;

        // Record file reads for post-compaction recovery
        if let Some(tracker) = &self.file_tracker {
            if is_file_read_tool(name) && !outcome.is_error {
                if let Some(path) = effective_args.get("file_path").and_then(|v| v.as_str()) {
                    tracker.record_read(path, &outcome.output_text);
                }
            }
        }

        // -----------------------------------------------------------------
        // Stages 6 & 7: Post-hooks
        // -----------------------------------------------------------------
        if self.has_hooks() {
            let post_hooks_span = tracing::info_span!(
                "pipeline.post_hooks",
                tool = name,
                is_error = outcome.is_error
            );
            // Build post context from effective_args (not base_ctx) so post-hooks
            // see the actual arguments the tool was invoked with.
            let post_ctx = self
                .build_context(name, &effective_args)
                .with_tool_output(&outcome.output_text)
                .with_tool_error(outcome.is_error);

            // Stage 6: AfterToolCall (always)
            match self
                .hooks
                .execute(HookEvent::AfterToolCall, &post_ctx)
                .instrument(post_hooks_span.clone())
                .await
            {
                Ok(post_result) => {
                    hook_messages.extend(post_result.messages);
                    additional_contexts.extend(post_result.additional_contexts);
                    if post_result.prevent_continuation {
                        prevent_continuation = true;
                    }
                    // Apply output modification (last-writer-wins)
                    if let Some(new_output) = post_result.updated_output {
                        outcome.output_text = new_output;
                    }
                }
                Err(e) => {
                    tracing::warn!(tool = name, error = %e, "Post-hook execute failed");
                }
            }
            // Stage 7: AfterToolCallFailure (only on error)
            if outcome.is_error {
                match self
                    .hooks
                    .execute(HookEvent::AfterToolCallFailure, &post_ctx)
                    .instrument(post_hooks_span.clone())
                    .await
                {
                    Ok(fail_result) => {
                        hook_messages.extend(fail_result.messages);
                        additional_contexts.extend(fail_result.additional_contexts);
                        if fail_result.prevent_continuation {
                            prevent_continuation = true;
                        }
                        if let Some(new_output) = fail_result.updated_output {
                            outcome.output_text = new_output;
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
            needs_user_confirmation,
            confirmation_reason,
        }
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    /// Build a `HookContext` from tool call parameters.
    fn build_context(&self, name: &str, arguments: &Value) -> HookContext {
        let args_str = arguments.to_string();
        let session_id = self
            .session_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let mut ctx = HookContext::new(session_id)
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

    /// Map a `ToolResult` to a `ToolOutcome`, applying compression and truncation.
    ///
    /// Successful results are compressed (domain-specific summarization) then
    /// truncated if they still exceed the per-tool budget. Error results
    /// are passed through unchanged (error messages are typically short and
    /// their completeness matters for debugging).
    fn map_result(
        id: &str,
        name: &str,
        result: &ToolResult,
        store: Option<&ToolResultStore>,
        budget: usize,
    ) -> ToolOutcome {
        match result {
            ToolResult::Success { output } => {
                let raw = value_to_text(output);
                let compressed = compress_tool_output(name, &raw);
                let final_text =
                    match store.and_then(|s| s.persist_if_large(id, name, &compressed, budget)) {
                        Some(ref_marker) => ref_marker,
                        None => truncate_tool_result_with_budget(&compressed, budget),
                    };
                ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    duration_ms: 0,
                    output_text: final_text,
                    is_error: false,
                    should_stop: false,
                    retryable: false,
                }
            }
            ToolResult::Error { error, retryable } => ToolOutcome {
                tool_id: id.to_string(),
                tool_name: name.to_string(),
                duration_ms: 0,
                output_text: error.clone(),
                is_error: true,
                should_stop: false,
                retryable: *retryable,
            },
            ToolResult::SuccessAndStopLoop { output } => {
                let raw = value_to_text(output);
                let compressed = compress_tool_output(name, &raw);
                let final_text =
                    match store.and_then(|s| s.persist_if_large(id, name, &compressed, budget)) {
                        Some(ref_marker) => ref_marker,
                        None => truncate_tool_result_with_budget(&compressed, budget),
                    };
                ToolOutcome {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    duration_ms: 0,
                    output_text: final_text,
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
                duration_ms: 0,
                output_text: message,
                is_error: true,
                should_stop: false,
                retryable: false,
            },
            additional_contexts: Vec::new(),
            prevent_continuation: false,
            hook_messages: Vec::new(),
            needs_user_confirmation: false,
            confirmation_reason: None,
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Map a `SafetyError` to a human-readable error string.
///
/// `NeedsConfirmation` is downgraded to a denial with explanation when no
/// interactive confirmation handler is available (the common case for automated
/// agent runs). This prevents the LLM from retrying indefinitely.
fn map_safety_error(e: &SafetyError) -> String {
    match e {
        SafetyError::Blocked { tool, pattern } => {
            format!(
                "[BLOCKED] Tool '{}' blocked by safety pattern '{}'",
                tool, pattern
            )
        }
        SafetyError::NeedsConfirmation { tool } => {
            // No interactive confirmation handler → downgrade to denied with explanation.
            // The LLM sees a clear reason and won't retry.
            format!(
                "[DENIED] Tool '{}' is classified as high-risk and requires user confirmation. \
                 No confirmation handler is available in this session. \
                 Use a safer alternative or ask the user to grant permission for this tool.",
                tool
            )
        }
        SafetyError::PolicyDenied { tool } => {
            format!("[DENIED] Tool '{}' denied by policy", tool)
        }
    }
}

/// Fast-fail input validation against tool schema.
///
/// Checks:
/// 1. Input is a JSON object
/// 2. All `required` fields from schema are present
///
/// This is a lightweight pre-check; full validation happens inside tool.call()
/// via serde deserialization.
fn validate_input_fast(schema: &Value, input: &Value) -> Result<(), String> {
    if !input.is_object() {
        return Err("expected JSON object".into());
    }
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        let obj = input.as_object().unwrap();
        for field in required {
            if let Some(name) = field.as_str() {
                if !obj.contains_key(name) {
                    return Err(format!("missing required field: {name}"));
                }
            }
        }
    }
    Ok(())
}

/// Convert a JSON Value to a display string.
fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Check if a tool name corresponds to a file read operation.
fn is_file_read_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("read_file") || lower.contains("file_read") || lower == "read"
}

/// Default per-tool result budgets.
fn default_result_budget(tool_name: &str) -> usize {
    match tool_name {
        "Read" => 12_000,
        "WebFetch" | "web_fetch" => 10_000,
        "Bash" | "bash_exec" => 8_000,
        "Grep" => 6_000,
        _ => MAX_TOOL_RESULT_TOKENS, // 8_000
    }
}

/// Truncate a tool result with head+tail preservation.
/// Keeps 70% from the head and 30% from the tail.
fn truncate_tool_result_with_budget(text: &str, budget_tokens: usize) -> String {
    let estimated = estimate_tokens_smart(text);
    if estimated <= budget_tokens {
        return text.to_string();
    }

    let chars_per_token: f64 = 2.5;
    let total_chars = (budget_tokens as f64 * chars_per_token) as usize;
    let head_chars = (total_chars as f64 * 0.7) as usize;
    let tail_chars = total_chars.saturating_sub(head_chars);

    // Find safe head boundary (last newline before head_chars)
    let head_end = text
        .char_indices()
        .take(head_chars)
        .filter(|(_, c)| *c == '\n')
        .last()
        .map(|(i, _)| i + 1)
        .unwrap_or_else(|| {
            text.char_indices()
                .nth(head_chars)
                .map(|(i, _)| i)
                .unwrap_or(text.len())
        });

    // Find safe tail boundary (snap to char boundary)
    let tail_byte_approx = text.len().saturating_sub(tail_chars * 4);
    let tail_byte_approx = (tail_byte_approx..text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let tail_start = text[tail_byte_approx..]
        .find('\n')
        .map(|i| tail_byte_approx + i + 1)
        .unwrap_or(tail_byte_approx);

    if head_end >= tail_start {
        // Overlap — fall back to head-only
        return truncate_tool_result(text);
    }

    let truncated_tokens = estimated.saturating_sub(budget_tokens);
    format!(
        "{}\n\n[... truncated ~{} tokens ...]\n\n{}",
        &text[..head_end],
        truncated_tokens,
        &text[tail_start..],
    )
}

/// Truncate a tool result string if it exceeds `MAX_TOOL_RESULT_TOKENS`.
///
/// Truncates at a safe boundary (last newline before the byte limit) to avoid
/// splitting lines or breaking JSON structure mid-object.
fn truncate_tool_result(text: &str) -> String {
    let estimated = estimate_tokens_smart(text);
    if estimated <= MAX_TOOL_RESULT_TOKENS {
        return text.to_string();
    }

    // Estimate how many characters correspond to the token limit.
    // estimate_tokens_smart uses chars/ratio, so we reverse: chars = tokens * ratio.
    // Use a conservative ratio (2.5 — code-like, since tool output is often structured).
    let max_chars = MAX_TOOL_RESULT_TOKENS * 25 / 10; // 2.5 chars/token

    // Find the byte offset at max_chars, then search backwards for a newline.
    let byte_limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len());

    // Search backwards from byte_limit for a newline to make a clean cut.
    let cut_point = text[..byte_limit]
        .rfind('\n')
        .map(|i| i + 1) // include the newline itself
        .unwrap_or(byte_limit);

    // Safe UTF-8 slice
    let truncated = text.get(..cut_point).unwrap_or(text);
    format!("{}{}", truncated, TRUNCATION_SUFFIX)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::hooks::HookExecutor;
    use crate::extension::{
        HookAction, HookConfig, HookEvent, HookKind, HookPriority, PermissionAction,
    };
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
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
