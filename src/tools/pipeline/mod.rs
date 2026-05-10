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
use std::sync::Arc;
use std::time::Instant;

use crate::sync_primitives::RwLock;

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
        let exec_elapsed_ms = exec_start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);

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

mod helpers;
use helpers::*;

#[cfg(test)]
mod tests;

