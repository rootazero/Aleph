//! Orchestrator & Flow Composition (Phase 5).
//!
//! See docs/superpowers/specs/2026-04-19-orchestrator-flow-composition-design.md

pub mod deps_builder;
pub mod dispatch;
pub mod errors;
pub mod flow_registry;
pub mod flow_run_tool;
pub mod flow_spec;
pub mod harness_bridge;
pub mod loader;
pub mod presets;
pub mod resolver;
pub mod sandbox_factory;
pub mod summary_format;

pub use deps_builder::{
    build_cheap_summary_provider, build_context_budget_config, build_dream_provider,
    build_failover_chain, build_stability_triple, build_strategy_planner_provider, ProviderChain,
    StabilityTriple,
};

pub use dispatch::{
    FlowHandle, FlowOutcome, FlowRequest, FlowStreamEvent, HarnessRunner, Orchestrator,
    TerminateReason,
};
// `ExecTier` appears in the public `FlowRequest::exec_tier` field and the
// `HarnessRunner::run` trait param, so it must be publicly nameable — the crate
// root keeps `config` private, which would otherwise leave those pub API items
// referencing an unreachable type (external `HarnessRunner` impls could not name
// the param). Re-exported here alongside the types that surface it.
pub use crate::config::types::policies::ExecTier;
// `SessionMode` — same publicly-nameable requirement as `ExecTier` above: it
// appears in `FlowRequest::session_mode` and the `HarnessRunner::run` param.
pub use crate::config::types::policies::SessionMode;
pub use errors::FlowError;
pub use flow_registry::{FlowRegistry, FlowSet};
pub use flow_run_tool::{FlowRunContext, FlowRunDescriptor, FlowRunInput, FlowRunTool};
pub use flow_spec::{
    AgentId, BrainRef, FlowHistoryTurn, FlowId, FlowInput, FlowOverrides, FlowSpec, ProviderId,
    SandboxKind, SessionStrategy,
};
pub use harness_bridge::AgentHarnessRunner;
pub use resolver::{RoutingOverrides, MAX_FLOW_DEPTH};
pub use sandbox_factory::{
    build_sandbox_factory, DenyAllSandbox, SandboxFactory, WorkspaceBuilder,
};

#[cfg(test)]
mod tests;
