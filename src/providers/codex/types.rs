//! Codex Responses API request/response types
//!
//! Re-exports shared Responses API types and defines Codex-specific types.

// Re-export all shared Responses API types
pub use crate::providers::responses::types::*;

use serde::Deserialize;

// ─── Security Types (used by security.rs) ─────────────────────

/// Chat requirements response (security tokens)
#[derive(Debug, Deserialize)]
pub struct ChatRequirements {
    pub token: String,
    #[serde(default)]
    pub proofofwork: Option<ProofOfWork>,
}

/// Proof-of-work challenge
#[derive(Debug, Deserialize)]
pub struct ProofOfWork {
    pub required: bool,
    pub seed: Option<String>,
    pub difficulty: Option<String>,
}
