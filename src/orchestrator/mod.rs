//! Orchestrator & Flow Composition (Phase 5).
//!
//! See docs/superpowers/specs/2026-04-19-orchestrator-flow-composition-design.md

pub mod errors;
pub mod flow_registry;
pub mod flow_spec;
pub mod loader;
pub mod presets;

pub use errors::FlowError;
pub use flow_registry::{FlowRegistry, FlowSet};
pub use flow_spec::{
    AgentId, BrainRef, FlowId, FlowInput, FlowOverrides, FlowSpec, ProviderId, SandboxKind,
    SessionStrategy,
};

#[cfg(test)]
mod tests;
