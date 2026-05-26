//! Execute pipeline: confirmation gate, BeforeToolCall hooks, retry, Layer 2
//! budget, AfterToolCall hooks, error sanitization.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::extension::hooks::{HookContext, PermissionDecision};
use crate::extension::HookEvent;
use crate::sandbox::exec_approval::gate::ApprovalOutcome;
use crate::session::events::ToolOutput;
use crate::tools::runtime::LoopTool;
use crate::tools::service::ToolError;

use super::ScopedToolService;

/// Wrap a tool result `Value` with `<system-reminder>` blocks for each
/// `context:` line emitted by hooks. Strings are prefixed in-place; other
/// values are stringified so the LLM-visible payload stays uniform.
///
/// This is the seam that makes Aleph's `context:` prefix protocol actually
/// reach the model: the contexts are appended to the tool-result text the
/// LLM consumes on its next turn. Without this wiring, `additional_contexts`
/// would be a silent no-op (a historical bug).
fn wrap_value_with_hook_contexts(value: Value, contexts: &[String]) -> Value {
    if contexts.is_empty() {
        return value;
    }
    let reminders = contexts
        .iter()
        .map(|c| format!("<system-reminder>\n{}\n</system-reminder>", c.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let text = match value {
        Value::String(s) => format!("{reminders}\n\n{s}"),
        Value::Null => reminders,
        other => format!("{reminders}\n\n{other}"),
    };
    Value::String(text)
}

/// Which dispatch branch `execute_inner` is routing into. Kept as a
/// fieldless enum so the retry closure can capture it by value.
#[derive(Copy, Clone)]
enum RoutingTarget {
    Subagent,
    Inner,
    Missing,
}

impl ScopedToolService {
    /// Tool dispatch proper. Wrapped by the `ToolService::execute_with_cancel`
    /// trait method, which scopes `TURN_CONTEXT` around it. The `cancel`
    /// token is forked per-call by the harness Act phase and threaded into
    /// the inner [`crate::tools::runtime::LoopToolRegistry::execute`] /
    /// [`crate::agents::subagent_tool::SubagentTool::execute`] so subprocess
    /// kill_on_drop, reqwest abort, etc. propagate naturally.
    pub(super) async fn execute_inner(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        // Enforce allowed filter.
        if !self.is_allowed(name) {
            return Err(ToolError::NotFound {
                name: name.to_string(),
            });
        }

        // Confirmation gate: tools flagged `requires_confirmation` must be
        // approved by the user before they run. Fails closed when no approval
        // transport is wired.
        if self.confirm_tools.contains(name) {
            match &self.approval_requester {
                Some(requester) => {
                    let reason = format!("Tool `{name}` requires your confirmation to run.");
                    // Fire PermissionRequest + Notification (best-effort,
                    // observer-only) so user-facing channels can pop a
                    // toast / send an email / etc. without blocking the
                    // approval path itself.
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::PermissionRequest,
                        &self.hook_session_id,
                        vec![
                            ("TOOL_NAME", name.to_string()),
                            ("REASON", reason.clone()),
                        ],
                    )
                    .await;
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::Notification,
                        &self.hook_session_id,
                        vec![
                            ("KIND", "permission_request".to_string()),
                            ("TOOL_NAME", name.to_string()),
                            ("MESSAGE", reason.clone()),
                        ],
                    )
                    .await;
                    let outcome = requester.request_approval(name, &reason).await;
                    if outcome != ApprovalOutcome::Approved {
                        return Err(ToolError::Execution {
                            name: name.to_string(),
                            cause: format!(
                                "User did not approve running `{name}` ({outcome:?}). \
                                 Do not retry; ask the user how to proceed."
                            ),
                        });
                    }
                }
                None => {
                    return Err(ToolError::Execution {
                        name: name.to_string(),
                        cause: format!(
                            "Tool `{name}` requires confirmation but no approval \
                             channel is available. Do not retry."
                        ),
                    });
                }
            }
        }

        // Fire pre-hook (legacy observational decorator).
        if let Some(ref hook) = self.hook_decorator {
            hook.before_execute(name, &input);
        }

        // Extension `BeforeToolCall` interceptors. May block / deny / ask, or
        // rewrite the tool input via `update_input:`. Inert when no executor
        // is wired or when no hooks match the event. Runs BEFORE routing so a
        // blocked call never reaches the retry pipeline.
        let started = std::time::Instant::now();
        let (effective_input, pre_hook_contexts) =
            match self.run_before_tool_hooks(name, input.clone()).await {
                Ok(outcome) => outcome,
                Err(err) => {
                    let duration_ms: u64 =
                        started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
                    let rejection: Result<ToolOutput, ToolError> = Err(err);
                    if let Some(ref hook) = self.hook_decorator {
                        hook.after_execute(name, &rejection);
                        hook.after_execute_with_duration(name, &rejection, duration_ms);
                    }
                    return rejection;
                }
            };

        // Route to subagent tool if name matches; otherwise route into the
        // inner LoopToolRegistry. Both paths share the retry/Layer 2/sanitize
        // pipeline below.
        let routing = if self
            .subagent_tool
            .as_ref()
            .is_some_and(|st| st.name() == name)
        {
            RoutingTarget::Subagent
        } else if self.inner.get(name).is_some() || self.inner.resolve(name).is_some() {
            RoutingTarget::Inner
        } else {
            RoutingTarget::Missing
        };

        let mut result = match routing {
            RoutingTarget::Missing => Err(ToolError::NotFound {
                name: name.to_string(),
            }),
            target => {
                // One-shot retry: if the inner Loop tool returned
                // `retryable: true` (mapped to `ToolError::Transport` in
                // `tool_result_to_output`), the helper sleeps 100ms and
                // retries exactly once — but ONLY for tools declared
                // idempotent. Non-idempotent tools (default) skip the
                // retry to avoid duplicate side effects on a timeout that
                // may have already reached the server. R10-safe: no policy
                // selection beyond the static idempotency classification.
                let idempotent = crate::tools::retry::is_idempotent_builtin_name(name);
                let raw_outcome =
                    crate::tools::retry::execute_with_one_shot_backoff(idempotent, || {
                        let input = effective_input.clone();
                        let name_owned = name.to_string();
                        let cancel = cancel.clone();
                        async move {
                            let raw = match target {
                                RoutingTarget::Subagent => {
                                    let st = self.subagent_tool.as_ref().ok_or_else(|| {
                                        ToolError::Execution {
                                            name: name_owned.clone(),
                                            cause: "SubagentTool was checked above but is now None"
                                                .into(),
                                        }
                                    })?;
                                    st.execute(input, cancel).await
                                }
                                RoutingTarget::Inner => {
                                    self.inner.execute(&name_owned, input, cancel).await
                                }
                                RoutingTarget::Missing => unreachable!(),
                            };
                            Self::tool_result_to_output(&name_owned, raw)
                        }
                    })
                    .await;
                match raw_outcome {
                    Ok(output) => Ok(self.apply_layer_two(name, output).await),
                    Err(err) => Err(Self::sanitize_tool_error(name, err)),
                }
            }
        };

        // Extension `AfterToolCall` / `AfterToolCallFailure` hooks. Observers
        // fire in parallel; Interceptors run sequentially and may rewrite the
        // visible tool output via `update_output:` on the success path.
        // `pre_hook_contexts` from BeforeToolCall are merged in here so they
        // ride along on the same tool result the LLM sees next turn.
        self.run_after_tool_hooks(name, &effective_input, &mut result, pre_hook_contexts)
            .await;

        let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

        // Fire post-hooks. Both for back-compat (v1) and with-duration (v2).
        if let Some(ref hook) = self.hook_decorator {
            hook.after_execute(name, &result);
            hook.after_execute_with_duration(name, &result, duration_ms);
        }

        result
    }

    /// Fire `BeforeToolCall` interceptors. Returns the (possibly rewritten)
    /// input + any `context:` lines the interceptors emitted (to be wrapped
    /// into the tool result), or a `ToolError` when a hook blocks / denies
    /// the call or when an `Ask` decision is not approved by the user.
    async fn run_before_tool_hooks(
        &self,
        name: &str,
        input: Value,
    ) -> Result<(Value, Vec<String>), ToolError> {
        let executor = match self.hook_executor.as_ref() {
            Some(e) if e.hook_count() > 0 => e.clone(),
            _ => return Ok((input, Vec::new())),
        };

        let ctx = self.build_hook_context(name, &input, None, None);
        let (_ctx, hook_result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, ctx)
            .await
            .map_err(|e| ToolError::Execution {
                name: name.to_string(),
                cause: format!("BeforeToolCall hook executor failed: {e}"),
            })?;

        // Hard deny — not retryable.
        if hook_result.denied {
            return Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason: hook_result
                    .deny_reason
                    .unwrap_or_else(|| "denied by hook".to_string()),
            });
        }

        // Soft block — surfaces as an execution error so the LLM can react.
        if hook_result.blocked {
            return Err(ToolError::Execution {
                name: name.to_string(),
                cause: hook_result
                    .block_reason
                    .unwrap_or_else(|| "blocked by hook".to_string()),
            });
        }

        // Ask: route through the approval requester (same seam as
        // `confirm_tools`). Fails closed when no transport is wired.
        if let Some(PermissionDecision::Ask { reason }) = hook_result.permission_decision {
            match &self.approval_requester {
                Some(requester) => {
                    // Mirror confirm_tools: fire PermissionRequest +
                    // Notification observers for user-attention plumbing.
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::PermissionRequest,
                        &self.hook_session_id,
                        vec![
                            ("TOOL_NAME", name.to_string()),
                            ("REASON", reason.clone()),
                        ],
                    )
                    .await;
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::Notification,
                        &self.hook_session_id,
                        vec![
                            ("KIND", "permission_request".to_string()),
                            ("TOOL_NAME", name.to_string()),
                            ("MESSAGE", reason.clone()),
                        ],
                    )
                    .await;
                    let outcome = requester.request_approval(name, &reason).await;
                    if outcome != ApprovalOutcome::Approved {
                        return Err(ToolError::Execution {
                            name: name.to_string(),
                            cause: format!(
                                "Hook requested user confirmation for `{name}` and the \
                                 user did not approve ({outcome:?})."
                            ),
                        });
                    }
                }
                None => {
                    return Err(ToolError::Execution {
                        name: name.to_string(),
                        cause: format!(
                            "Hook requested user confirmation for `{name}` but no \
                             approval channel is available. Do not retry."
                        ),
                    });
                }
            }
        }

        // Last-writer-wins rewrite of the tool input; surface
        // `context:` lines so they actually reach the LLM next turn.
        Ok((
            hook_result.updated_input.unwrap_or(input),
            hook_result.additional_contexts,
        ))
    }

    /// Fire `AfterToolCall` / `AfterToolCallFailure` hooks. Observers run in
    /// parallel; Interceptors run sequentially and may override the visible
    /// tool output via `update_output:` on the success path. Any
    /// `additional_contexts` from BeforeToolCall (`pre_contexts`) plus those
    /// emitted here are wrapped into the tool output as
    /// `<system-reminder>` blocks so the LLM actually sees them next turn.
    async fn run_after_tool_hooks(
        &self,
        name: &str,
        input: &Value,
        result: &mut Result<ToolOutput, ToolError>,
        pre_contexts: Vec<String>,
    ) {
        let executor = match self.hook_executor.as_ref() {
            Some(e) if e.hook_count() > 0 => e.clone(),
            _ => {
                if !pre_contexts.is_empty() {
                    if let Ok(output) = result {
                        output.value = wrap_value_with_hook_contexts(
                            std::mem::take(&mut output.value),
                            &pre_contexts,
                        );
                    }
                }
                return;
            }
        };

        match result {
            Ok(output) => {
                let output_str = output.value.to_string();
                let ctx = self.build_hook_context(name, input, Some(&output_str), Some(false));
                // Fire fire-and-forget Observer-kind hooks in parallel first.
                executor
                    .execute_observers(HookEvent::AfterToolCall, &ctx)
                    .await;
                // Then run Interceptor-kind hooks to harvest `update_output:`
                // — the only post-execution mutation we honor. block / deny
                // semantics make no sense post-hoc and are ignored.
                let mut all_contexts = pre_contexts;
                if let Ok((_ctx, hr)) = executor
                    .execute_interceptors(HookEvent::AfterToolCall, ctx)
                    .await
                {
                    if let Some(text) = hr.updated_output {
                        output.value = Value::String(text);
                    }
                    all_contexts.extend(hr.additional_contexts);
                }
                if !all_contexts.is_empty() {
                    output.value = wrap_value_with_hook_contexts(
                        std::mem::take(&mut output.value),
                        &all_contexts,
                    );
                }
            }
            Err(err) => {
                let err_str = err.to_string();
                let ctx = self.build_hook_context(name, input, Some(&err_str), Some(true));
                executor
                    .execute_observers(HookEvent::AfterToolCallFailure, &ctx)
                    .await;
                // Symmetry with the success path: let Interceptor-kind hooks
                // fire too (e.g., for structured logging), but the failure
                // path is read-only — `update_output:` is ignored because
                // there is no `ToolOutput` to mutate. Pre-hook contexts are
                // intentionally dropped on failure; they referenced an input
                // that never produced a result for the LLM to attach them to.
                let _ = executor
                    .execute_interceptors(HookEvent::AfterToolCallFailure, ctx)
                    .await;
            }
        }
    }

    fn build_hook_context(
        &self,
        name: &str,
        input: &Value,
        tool_output: Option<&str>,
        tool_error: Option<bool>,
    ) -> HookContext {
        let mut ctx = HookContext::new(self.hook_session_id.clone())
            .with_tool_name(name.to_string())
            .with_arguments(input.to_string())
            .with_tool_input(input.to_string());
        if let Some(out) = tool_output {
            ctx = ctx.with_tool_output(out.to_string());
        }
        if let Some(is_err) = tool_error {
            ctx = ctx.with_tool_error(is_err);
        }
        ctx
    }

    /// Apply Layer 2 of the budget pipeline (`compress → persist-if-large
    /// → truncate`) to a successful tool output. Reuses the existing
    /// per-tool compression hook (`compress_tool_output`) and the shared
    /// `result_store` if one is wired; falls back to head+tail truncation
    /// otherwise.
    async fn apply_layer_two(&self, name: &str, mut out: ToolOutput) -> ToolOutput {
        // Compress first: hands JSON to the per-tool summarizer that
        // already exists in `tool_output::compressor`. The text we feed
        // into Layer 2 reflects what the LLM will ultimately see.
        let raw = match &out.value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let compressed = crate::tool_output::compressor::compress_tool_output(name, &raw);

        let explicit = self.inner.max_result_tokens_for(name);
        let budget = crate::tools::result_processing::resolve_result_budget(name, explicit);

        // Generate a per-call file name suffix so concurrent calls to the
        // same tool do not collide on disk. The LLM correlates the result
        // through the surrounding conversation history, not the path.
        let call_id = uuid::Uuid::new_v4().simple().to_string();

        let processed = crate::tools::result_processing::apply_result_budget(
            &call_id,
            name,
            &compressed,
            self.result_store.as_deref(),
            budget,
        );

        // Mark `metadata.truncated` whenever Layer 2 shortened the text.
        if processed.persisted_path.is_some() || processed.text.contains("[output truncated") {
            out.metadata.truncated = true;
        }

        // Extension hooks observe large tool results offloaded to disk.
        if let Some(ref path) = processed.persisted_path {
            if let Some(executor) = self.hook_executor.as_ref() {
                let ctx = HookContext::new(self.hook_session_id.clone())
                    .with_tool_name(name)
                    .with_env("TOOL_CALL_ID", call_id.clone())
                    .with_env("PERSIST_PATH", path.display().to_string())
                    .with_env("PERSIST_REF", processed.text.clone());
                executor
                    .execute_observers(HookEvent::ToolResultPersist, &ctx)
                    .await;
            }
        }

        out.value = Value::String(processed.text);
        out
    }

    /// Wrap a `ToolError` text payload with the standard external-content
    /// fence so reflected user input / scraped remote data inside the
    /// error message cannot smuggle prompt-injection patterns back into
    /// the LLM. The fence labels the source as `tool_error:<tool>` so the
    /// model can pattern-match consistently with `tool_error` outputs
    /// from other channels.
    fn sanitize_tool_error(name: &str, err: ToolError) -> ToolError {
        use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
        // Preserve the original variant so callers can keep matching on
        // `Timeout` / `Transport` / `Execution`; only the `cause` /
        // message string is sanitized.
        match err {
            ToolError::Execution { name: n, cause } => ToolError::Execution {
                name: n,
                cause: wrap_external_content(
                    &cause,
                    ContentSource::ToolError {
                        tool: name.to_string(),
                    },
                ),
            },
            ToolError::Transport { name: n, cause } => ToolError::Transport {
                name: n,
                cause: wrap_external_content(
                    &cause,
                    ContentSource::ToolError {
                        tool: name.to_string(),
                    },
                ),
            },
            // Other variants either have no untrusted payload (NotFound,
            // PermissionDenied, Duplicate, ValidationFailed) or are
            // structured enough not to need wrapping (Timeout). Pass
            // through unchanged.
            other => other,
        }
    }
}
