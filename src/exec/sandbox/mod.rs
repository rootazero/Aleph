//! Sandbox subsystem — DEPRECATED.
//!
//! This module is being phased out in favor of `crate::sandbox`. All new code
//! should use the platform-specific drivers in `crate::sandbox::platforms`.
//! These exports are retained for backward compatibility during migration.

pub mod adapter;
pub mod audit;
pub mod capabilities;
pub mod capability_resolver;
pub mod executor;
pub mod parameter_binding;
pub mod platforms;
pub mod presets;
pub mod profile;

#[cfg(test)]
mod tests;

pub use adapter::{SandboxAdapter, SandboxCommand, SandboxProfile};
pub use audit::{ExecutionStatus, SandboxAuditLog, SandboxViolation};
pub use capabilities::{
    Capabilities, EnvironmentCapability, FileSystemCapability, NetworkCapability, ProcessCapability,
};
pub use executor::{FallbackPolicy, OsSandboxDriver};
pub use profile::ProfileGenerator;
