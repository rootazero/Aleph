//! Orchestrator & Flow Composition (Phase 5).
//!
//! See docs/superpowers/specs/2026-04-19-orchestrator-flow-composition-design.md

pub mod deps_builder;
pub mod dispatch;
pub mod errors;
pub mod flow_registry;
pub mod flow_spec;
pub mod harness_bridge;
pub mod loader;
pub mod presets;
pub mod resolver;
pub mod sandbox_factory;

pub use deps_builder::{
    build_cheap_summary_provider, build_context_budget_config, build_context_budget_refiner,
    build_dream_provider, build_failover_chain, build_stability_triple,
    build_strategy_planner_provider, ContextBudgetRefiner, ProviderChain, StabilityTriple,
};

pub use dispatch::{
    FlowHandle, FlowOutcome, FlowRequest, FlowStreamEvent, HarnessRunner, Orchestrator,
    TerminateReason,
};
// `ExecTier` / `SessionMode` appear in the public `crate::thinker::TurnEnvelope`
// fields (`exec_tier`, `session_mode`), and the crate root keeps `config`
// private, so these must be re-exported here to stay publicly nameable — the
// config path alone would leave `TurnEnvelope` referencing an unreachable type.
pub use crate::config::types::policies::ExecTier;
pub use crate::config::types::policies::SessionMode;
// Their `identity_meta.custom` key names, for the same reason one rung up: a
// caller outside this crate that has to name the knob a request carries would
// otherwise spell the string itself, which is the copy that goes stale when the
// key is renamed. The other four knob keys are already reachable through
// `agents::thinking` and `memory::session_memory_mode`.
pub use crate::config::types::policies::{EXEC_TIER_SESSION_KEY, MODE_SESSION_KEY};
pub use errors::FlowError;
pub use flow_registry::{FlowRegistry, FlowSet};
pub use flow_spec::{
    AgentId, BrainRef, FlowHistoryTurn, FlowId, FlowInput, FlowOverrides, FlowSpec, ProviderId,
    SessionStrategy,
};
pub use harness_bridge::AgentHarnessRunner;
pub use sandbox_factory::{build_sandbox_factory, SandboxFactory, WorkspaceBuilder};

#[cfg(test)]
mod tests;
