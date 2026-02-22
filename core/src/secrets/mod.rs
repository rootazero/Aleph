//! Secret management module
//!
//! Provides encrypted storage for sensitive credentials (API keys, tokens).
//! Uses AES-256-GCM with per-entry HKDF-SHA256 key derivation.

pub mod crypto;
pub mod migration;
pub mod types;
pub mod vault;

pub use types::{DecryptedSecret, SecretError};
pub use vault::{resolve_master_key, SecretVault};
