//! Three-Layer Control Architecture
//!
//! A balanced approach to agent control:
//! - Top Layer: Orchestrator (FSM state machine with hard constraints)
//! - Middle Layer: Skill DAG (stable, testable workflows)
//! - Bottom Layer: Tools (capability-based with sandbox)
//!
//! This is the default and only orchestration architecture.
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::three_layer::{Capability, CapabilityGate, PathSandbox};
//!
//! // Create a gate with specific capabilities
//! let gate = CapabilityGate::new(vec![
//!     Capability::FileRead,
//!     Capability::WebSearch,
//! ]);
//!
//! // Create a sandbox for a workspace
//! let sandbox = PathSandbox::with_defaults(vec![
//!     PathBuf::from("/workspace/project"),
//! ]);
//! ```

pub mod orchestrator;
pub mod safety;
pub mod skill;

// Re-exports for convenience
pub use orchestrator::{GuardChecker, GuardViolation, OrchestratorState};
pub use safety::{
    Capability, CapabilityDenied, CapabilityGate, CapabilityLevel, PathSandbox, QuotaExceeded,
    QuotaTracker, QuotaUsage, ResourceQuota, SandboxViolation,
};
pub use skill::{SkillDefinition, SkillNode, SkillNodeType, SkillRegistry};

#[cfg(test)]
mod tests;
