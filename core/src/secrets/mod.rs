//! Secret management module
//!
//! Provides encrypted storage for sensitive credentials (API keys, tokens).
//! Uses AES-256-GCM with per-entry HKDF-SHA256 key derivation.

pub mod cache;
pub mod crypto;
pub mod injection;
pub mod leak_detector;
pub mod migration;
pub mod placeholder;
pub mod provider;
pub mod router;
pub mod types;
pub mod vault;
pub mod web3_signer;

pub use injection::{render_with_secrets, InjectedSecret};
pub use leak_detector::{LeakDecision, LeakDetector};
pub use placeholder::{extract_secret_refs, SecretRef};
pub use types::{DecryptedSecret, EntryMetadata, SecretError};
pub use vault::SecretVault;
pub use web3_signer::{EvmSigner, SignIntent, SignedResult};

// Re-export config types used in routing (so integration tests can access them)
pub use crate::config::types::secrets::{SecretMapping, Sensitivity};
