//! Windows sandbox platform implementation.
//!
//! Provides Windows-specific sandboxing using:
//! - AppContainer isolation (Win10+, preferred)
//! - Restricted tokens (CreateRestrictedToken, fallback)
//! - ACL-based filesystem restrictions
//! - Job objects for resource limits
//! - WFP network filtering (AllowHosts enforcement)
//!
//! Based on Windows security model patterns from codex-windows-sandbox,
//! adapted for Aleph's OsSandboxDriverTrait architecture.

mod acl;
mod appcontainer;
pub mod driver;
mod filter;
mod job;
mod token;
mod wfp;

pub use appcontainer::{AppContainer, AppContainerCapability};
pub use driver::WindowsSandboxDriver;
