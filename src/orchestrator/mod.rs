//! Orchestrator & Flow Composition (Phase 5).
//!
//! See docs/superpowers/specs/2026-04-19-orchestrator-flow-composition-design.md

pub mod dispatch;
pub mod errors;
pub mod flow_registry;
pub mod flow_spec;
pub mod loader;
pub mod presets;
pub mod resolver;
pub mod sandbox_factory;

pub use dispatch::{
    FlowHandle, FlowOutcome, FlowRequest, FlowStreamEvent, HarnessRunner, Orchestrator,
};
pub use errors::FlowError;
pub use flow_registry::{FlowRegistry, FlowSet};
pub use flow_spec::{
    AgentId, BrainRef, FlowId, FlowInput, FlowOverrides, FlowSpec, ProviderId, SandboxKind,
    SessionStrategy,
};
pub use resolver::{RoutingOverrides, MAX_FLOW_DEPTH};
pub use sandbox_factory::{build_sandbox_factory, DenyAllSandbox, SandboxFactory, WorkspaceBuilder};

#[cfg(test)]
mod tests;
