//! Cross-cutting security primitives.
//!
//! Complements `gateway::security` (auth/identity) with:
//! - HTTP security headers
//! - SSRF protection
//! - Content sanitization
//! - Persistent audit logging

pub mod audit;
pub mod audit_drain;
pub mod content_sanitizer;
pub mod context_id_hasher;
pub mod dangerous_tools;
pub mod headers;
pub mod injection_patterns;
pub mod runtime_guard;
pub mod safe_regex;
pub mod secret_env;
pub mod secret_equal;
pub mod ssrf;
pub mod unicode_guard;

pub use audit_drain::spawn_audit_drain;
pub use context_id_hasher::ContextIdHasher;
pub use runtime_guard::{
    GuardResult, RuntimeSecurityGuard, SecurityContext, SecurityGuardConfig, SecurityGuardError,
};
pub use secret_equal::{secret_equal, secret_equal_bytes};

#[cfg(test)]
mod export_tests {
    #[test]
    fn test_runtime_guard_exports_compile() {
        let _ = crate::security::SecurityGuardConfig::default();
    }
}
