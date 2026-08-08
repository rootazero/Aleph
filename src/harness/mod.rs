//! Harness — Anthropic-style Think→Act driver.
//!
//! Phase 4b of the managed-agents refactor.
//! Spec: docs/superpowers/specs/2026-04-19-harness-think-act-design.md

pub mod agent;
pub mod callback;
pub mod chain_context;
pub mod deps;
pub mod trace;
pub mod trace_sink;
pub mod trait_def;

pub use agent::AgentHarness;
pub use callback::{HarnessCallback, NoopHarnessCallback};
pub use deps::HarnessDeps;
pub use deps::{StallConfig, StallTracker};
pub use trace_sink::{NoopTraceSink, TraceSink};
pub use trait_def::{HarnessError, TurnState};

#[cfg(test)]
mod tests {
    mod act;
    mod agent;
    mod budget;
    mod guardrails;
    mod harness_ext;
    mod prompt;
    mod reactive_compaction;
    mod stability;
    mod task10_wiring;
    mod think;
    mod tools_surface;
}
