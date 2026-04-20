//! Harness — Anthropic-style Think→Act driver.
//!
//! Phase 4b of the managed-agents refactor.
//! Spec: docs/superpowers/specs/2026-04-19-harness-think-act-design.md

pub mod adapters;
pub mod agent;
pub mod callback;
pub mod chain_context;
pub mod context_budget;
pub mod context_compactor;
pub mod deps;
pub mod provider_bridge;
pub mod sections;
pub mod skill_prefetch;
pub mod stop_hooks;
pub mod tool_execution_context;
pub mod tool_summary;
pub mod trace;
pub mod trace_sink;
pub mod trait_def;
pub mod verify_stop_hook;

pub use agent::AgentHarness;
pub use callback::{HarnessCallback, NoopHarnessCallback};
pub use deps::HarnessDeps;
pub use trace_sink::{NoopTraceSink, TraceSink};
pub use trait_def::{Harness, HarnessError, TurnState};

#[cfg(test)]
mod tests {
    mod act;
    mod driver;
    mod task10_wiring;
    mod think;
}
