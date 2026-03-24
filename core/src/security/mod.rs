//! Cross-cutting security primitives.
//!
//! Complements `gateway::security` (auth/identity) with:
//! - HTTP security headers
//! - SSRF protection
//! - Content sanitization
//! - Persistent audit logging

pub mod headers;
pub mod ssrf;
pub mod content_sanitizer;
pub mod audit;
