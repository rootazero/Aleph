//! Safety module for Three-Layer Control
//!
//! Provides capability-based security, path sandboxing, and resource quotas.

mod capability;

pub use capability::{Capability, CapabilityLevel};

// ===== Placeholder types for future implementation =====
// These will be properly implemented in subsequent tasks

use std::collections::HashSet;
use std::path::PathBuf;

/// Gate that controls capability access
///
/// TODO: Implement in Task 1.x
#[derive(Debug, Clone, Default)]
pub struct CapabilityGate {
    /// Capabilities that are allowed
    pub allowed: HashSet<Capability>,
    /// Capabilities that require confirmation
    pub confirmation_required: HashSet<Capability>,
    /// Capabilities that are blocked
    pub blocked: HashSet<Capability>,
}

/// Sandbox for path access control
///
/// TODO: Implement in Task 1.x
#[derive(Debug, Clone, Default)]
pub struct PathSandbox {
    /// Allowed read paths
    pub read_paths: Vec<PathBuf>,
    /// Allowed write paths
    pub write_paths: Vec<PathBuf>,
}

/// Error when sandbox is violated
///
/// TODO: Implement in Task 1.x
#[derive(Debug, Clone, thiserror::Error)]
pub enum SandboxViolation {
    /// Path is not allowed for reading
    #[error("Path not allowed for reading: {0}")]
    ReadNotAllowed(PathBuf),
    /// Path is not allowed for writing
    #[error("Path not allowed for writing: {0}")]
    WriteNotAllowed(PathBuf),
    /// Capability is not granted
    #[error("Capability not granted: {0}")]
    CapabilityNotGranted(Capability),
}
