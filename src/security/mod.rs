//! Cross-cutting security primitives.
//!
//! Complements `gateway::security` (auth/identity) with:
//! - HTTP security headers
//! - SSRF protection
//! - Content sanitization
//! - Persistent audit logging

pub mod audit;
pub mod content_sanitizer;
pub mod headers;
pub mod runtime_guard;
pub mod ssrf;

pub use runtime_guard::{
    GuardResult, RuntimeSecurityGuard, SecurityContext, SecurityGuardConfig, SecurityGuardError,
};

#[cfg(test)]
mod export_tests {
    #[test]
    fn test_runtime_guard_exports_compile() {
        let _ = crate::security::SecurityGuardConfig::default();
        let _ = crate::security::RuntimeSecurityGuard::default_guard();
    }
}
