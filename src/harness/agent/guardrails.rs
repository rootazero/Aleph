//! Guardrail logic — input, output, and tool-call safety checks.

use std::time::Instant;

use serde_json::Value;

use super::{AgentHarness, InputGuardrailOutcome, ToolCallGuardOutcome};
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::HarnessError;
use crate::providers::adapter::NativeToolCall;
use crate::session::events::{now_ms, SessionEvent};
use crate::session::service::SessionId;

impl AgentHarness {
    /// Stage 5a (#9): Apply the input guardrail to the latest `UserMessage` in
    /// the tail. Returns the (possibly rewritten) events vector or a `Block`
    /// reason. The original session log is never mutated — sanitisation
    /// happens only on the in-memory clone passed to the prompt builder, so
    /// audit trails preserve the original text.
    pub(crate) async fn apply_input_guardrail(
        &self,
        registry: &crate::guardrails::GuardrailRegistry,
        events: Vec<crate::session::events::SessionEventRecord>,
        tail_start: usize,
    ) -> Result<InputGuardrailOutcome, HarnessError> {
        // Locate the latest UserMessage index in the tail, if any.
        let latest_user_idx = events[tail_start..]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(rel, r)| matches!(r.event, SessionEvent::UserMessage { .. }).then_some(rel))
            .map(|rel| tail_start + rel);
        let Some(idx) = latest_user_idx else {
            return Ok(InputGuardrailOutcome::Allow(events));
        };
        let text = match &events[idx].event {
            SessionEvent::UserMessage { content, .. } => content.text.clone(),
            _ => return Ok(InputGuardrailOutcome::Allow(events)),
        };
        match registry.evaluate_input(&text).await {
            crate::guardrails::GuardrailDecision::Allow => Ok(InputGuardrailOutcome::Allow(events)),
            crate::guardrails::GuardrailDecision::Warn { reason } => {
                tracing::warn!(reason = %reason, "input guardrail warned");
                Ok(InputGuardrailOutcome::Allow(events))
            }
            crate::guardrails::GuardrailDecision::Sanitize(rep) => {
                let mut events = events;
                if let SessionEvent::UserMessage { content, .. } = &mut events[idx].event {
                    content.text = rep.text;
                }
                Ok(InputGuardrailOutcome::Sanitized(events))
            }
            crate::guardrails::GuardrailDecision::Block { reason, class: _ } => {
                Ok(InputGuardrailOutcome::Blocked(reason))
            }
        }
    }

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
                let new_args = serde_json::from_str(&rep.text)
                    .unwrap_or_else(|_| Value::String(rep.text.clone()));
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
