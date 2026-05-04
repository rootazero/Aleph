//! Harness — Anthropic-style Think→Act driver.
//!
//! Phase 4b of the managed-agents refactor.
//! Spec: docs/superpowers/specs/2026-04-19-harness-think-act-design.md

pub mod agent;
pub mod callback;
pub mod chain_context;
pub mod deps;
pub mod loop_callback;
pub mod stall;
pub mod trace;
pub mod trace_sink;
pub mod trait_def;

pub use agent::AgentHarness;
pub use callback::{HarnessCallback, NoopHarnessCallback};
pub use deps::HarnessDeps;
pub use loop_callback::{LoopCallback, NoopCallback};
pub use stall::{StallConfig, StallTracker};
pub use trace_sink::{NoopTraceSink, TraceSink};
pub use trait_def::{Harness, HarnessError, TurnState};

#[cfg(test)]
mod tests {
    mod act;
    mod driver;
    mod stability;
    mod task10_wiring;
    mod think;
}
