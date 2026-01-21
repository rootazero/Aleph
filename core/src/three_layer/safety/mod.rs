//! Safety module for Three-Layer Control
//!
//! Provides capability-based security, path sandboxing, and resource quotas.

mod capability;
mod gate;
mod quota;
mod sandbox;

pub use capability::{Capability, CapabilityLevel};
pub use gate::{CapabilityDenied, CapabilityGate};
pub use quota::{QuotaExceeded, QuotaTracker, QuotaUsage, ResourceQuota};
pub use sandbox::{PathSandbox, SandboxViolation};
