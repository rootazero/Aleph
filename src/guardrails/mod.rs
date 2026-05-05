//! Stage 5 — Guardrails Pipeline (#9).
//!
//! Three trait surfaces (`InputGuardrail`, `OutputGuardrail`,
//! `ToolCallGuardrail`) consulted by `AgentHarness` at three callsites
//! (turn entry, model output, tool dispatch). Decisions reuse Stage 1
//! `ErrorClass` so block reasons share the harness-wide retry vocabulary.
//!
//! Stage 5a ships Input + Output + Registry + `PiiSecretsGuardrail`.
//! Stage 5b wires ToolCall callsite + `on_model_fallback`.

pub mod decision;
pub mod pii_secrets;
pub mod registry;
pub mod traits;

pub use decision::{GuardrailDecision, Replacement};
pub use pii_secrets::PiiSecretsGuardrail;
pub use registry::{GuardrailRegistry, GuardrailRegistryBuilder};
pub use traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};

#[cfg(test)]
mod tests {
    mod bench;
    mod input;
    mod output;
    mod registry;
}
