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

pub mod safety;

// Re-exports
pub use safety::{Capability, CapabilityGate, CapabilityLevel, PathSandbox, SandboxViolation};
