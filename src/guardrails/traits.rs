//! The three Guardrail trait surfaces: Input (turn entry), Output (turn exit),
//! ToolCall (per dispatch).
//!
//! All three are `Send + Sync + 'static` and `async_trait` to permit IO-bound
//! impls (e.g. external classifier service). Stage 5a ships the trait surface
//! + Input/Output callsites; Stage 5b wires ToolCall into `agent.rs::act`.

use async_trait::async_trait;
use serde_json::Value;

use crate::guardrails::decision::GuardrailDecision;

/// Inspects user-provided input before it enters the LLM request.
#[async_trait]
pub trait InputGuardrail: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision;
}

/// Inspects model output before it is persisted / streamed to channel.
#[async_trait]
pub trait OutputGuardrail: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision;
}

/// Inspects each tool dispatch before `ToolService::execute(...)`.
/// Stage 5a defines the trait; Stage 5b wires the callsite.
#[async_trait]
pub trait ToolCallGuardrail: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn evaluate_tool_call(&self, tool_name: &str, args: &Value) -> GuardrailDecision;
}
