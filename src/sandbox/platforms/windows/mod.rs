//! Windows sandbox platform implementation.
//!
//! Provides Windows-specific sandboxing using:
//! - Restricted tokens (CreateRestrictedToken)
//! - ACL-based filesystem restrictions
//! - Job objects for resource limits
//!
//! Based on Windows security model patterns from codex-windows-sandbox,
//! adapted for Aleph's OsSandboxDriverTrait architecture.

mod acl;
mod driver;
mod job;
mod token;

pub use driver::WindowsSandboxDriver;
