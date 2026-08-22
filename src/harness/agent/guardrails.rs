//! Guardrail logic — tool-call safety checks.
//!
//! The input surface lives in `crate::guardrails::GuardrailRegistry`
//! (`screen_session_input`): it screens the whole replayed log, not just this
//! turn's message, which is a property of the prompt — not of the loop.

use std::time::Instant;

use serde_json::Value;

use super::{AgentHarness, ToolCallGuardOutcome};
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::HarnessError;
use crate::providers::adapter::NativeToolCall;
use crate::session::events::{now_ms, SessionEvent};
use crate::session::service::SessionId;

impl AgentHarness {
    /// Stage 5b (#9): Apply the tool-call guardrail. `Block` persists a
    /// `ToolError`, fires `on_safety_block`, and emits a trace; the caller
    /// then `continue`s the batch. `Sanitize` returns a fresh `Value`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn apply_tool_call_guardrail(
        &self,
        registry: &crate::guardrails::GuardrailRegistry,
        session_id: &SessionId,
        turn_id: crate::session::events::TurnId,
        call: &NativeToolCall,
        started: Instant,
        iteration: usize,
        callback: &mut dyn HarnessCallback,
    ) -> Result<ToolCallGuardOutcome, HarnessError> {
        match registry
            .evaluate_tool_call(&call.name, &call.arguments)
            .await
        {
            crate::guardrails::GuardrailDecision::Allow => Ok(ToolCallGuardOutcome::Pass),
            crate::guardrails::GuardrailDecision::Warn { reason } => {
                tracing::warn!(?session_id, tool = %call.name, reason = %reason, "tool-call guardrail warned");
                Ok(ToolCallGuardOutcome::Pass)
            }
            crate::guardrails::GuardrailDecision::Sanitize(rep) => {
                // On reparse failure, fail CLOSED: block the tool call rather than
                // silently fall back to the model's original args. The previous
                // behavior (`call.arguments.clone()` on parse error) was
                // fail-open — a guardrail that identified a leak/secret could
                // return a sanitized string the harness then refused to apply,
                // and the un-sanitized original reached the tool. The deeper fix
                // is to have guardrail carry a `Value` not a `String`; until
                // then, a JSON parse error must not be allowed to bypass
                // sanitization.
                let new_args = match serde_json::from_str::<Value>(&rep.text) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(
                            ?session_id,
                            tool = %call.name,
                            source = %rep.source,
                            error = %e,
                            "sanitized tool-call args did not reparse as JSON; blocking call (fail-closed)"
                        );
                        let reason = format!(
                            "guardrail sanitization produced invalid JSON: {e}"
                        );
                        callback.on_safety_block(&reason);
                        return Ok(ToolCallGuardOutcome::Block);
                    }
                };
                tracing::info!(?session_id, tool = %call.name, source = %rep.source, "tool-call args sanitized");
                Ok(ToolCallGuardOutcome::Sanitize(new_args))
            }
            crate::guardrails::GuardrailDecision::Block { reason, class: _ } => {
                callback.on_safety_block(&reason);
                let block_msg = format!("guardrail blocked: {reason}");
                let error_event = SessionEvent::ToolError {
                    turn_id,
                    call_id: call.id.clone(),
                    error: block_msg.clone(),
                    at: now_ms(),
                };
                if let Err(e) = self.deps.session.emit_event(session_id, error_event).await {
                    tracing::warn!(?session_id, call_id = %call.id, ?e, "failed to persist guardrail-block ToolError");
                }
                let dur_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
                // Live "done" event for the blocked call so the broadcast
                // stream closes its ToolStart with a matching ToolEnd (error
                // body) instead of leaving the call pending forever.
                callback.on_tool_call_done(&call.id, None, Some(&block_msg), dur_ms);
                // Same terminal bookkeeping every other Act outcome gets
                // (success / error / memo hit / cross-batch refusal); a block
                // was the only one that skipped it, leaving the call out of
                // `tool_timeline` → `RunSummary.tool_summaries`, the
                // authoritative state consumers reconcile the deliberately
                // lossy `agent_trace` stream against.
                self.push_tool_invocation(
                    call.id.clone(),
                    call.name.clone(),
                    dur_ms,
                    false,
                    Some(block_msg.clone()),
                );
                self.emit(
                    || crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                        iteration,
                        call: crate::harness::trace::ToolCallEndEvent {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            input: call.arguments.clone(),
                            duration_ms: dur_ms,
                        },
                        result: crate::tools::runtime::ToolResult::Error {
                            error: block_msg,
                            retryable: false,
                        },
                    },
                );
                Ok(ToolCallGuardOutcome::Block)
            }
        }
    }
}
