//! Sandbox subsystem for secure execution of AI-generated skills.
//!
//! Provides OS-native sandboxing with fine-grained permission control.

pub mod adapter;
pub mod audit;
pub mod capabilities;
pub mod executor;
pub mod platforms;
pub mod presets;
pub mod profile;

#[cfg(test)]
mod tests;

// Re-exports will be enabled as types are implemented
// pub use adapter::{SandboxAdapter, SandboxCommand, SandboxProfile};
// pub use audit::{ExecutionStatus, SandboxAuditLog, SandboxViolation};
// pub use capabilities::{
//     Capabilities, EnvironmentCapability, FileSystemCapability, NetworkCapability,
//     ProcessCapability,
// };
// pub use executor::{FallbackPolicy, SandboxManager};
// pub use profile::ProfileGenerator;
