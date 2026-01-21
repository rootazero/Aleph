//! Three-Layer Control Architecture
//!
//! A balanced approach to agent control:
//! - Top Layer: Orchestrator (FSM state machine with hard constraints)
//! - Middle Layer: Skill DAG (stable, testable workflows)
//! - Bottom Layer: Tools (capability-based with sandbox)
//!
//! # Usage
//!
//! Enable via config: `orchestrator.use_three_layer_control = true`
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

pub mod safety;

// Re-exports for convenience
pub use safety::{
    Capability, CapabilityDenied, CapabilityGate, CapabilityLevel, PathSandbox, SandboxViolation,
};
