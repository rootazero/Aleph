//! Harness — Anthropic-style Think→Act driver.
//!
//! Phase 4b of the managed-agents refactor.
//! Spec: docs/superpowers/specs/2026-04-19-harness-think-act-design.md

pub mod agent;
pub mod callback;
pub mod deps;
pub mod trait_def;

pub use agent::AgentHarness;
pub use callback::{HarnessCallback, NoopHarnessCallback};
pub use deps::HarnessDeps;
pub use trait_def::{Harness, HarnessError, TurnState};

#[cfg(test)]
mod tests {
    mod act;
    mod driver;
    mod think;
}
