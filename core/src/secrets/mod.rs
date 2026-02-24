//! Secret management module
//!
//! Provides encrypted storage for sensitive credentials (API keys, tokens).
//! Uses AES-256-GCM with per-entry HKDF-SHA256 key derivation.

pub mod crypto;
pub mod injection;
pub mod leak_detector;
pub mod migration;
pub mod placeholder;
pub mod provider;
pub mod types;
pub mod vault;
pub mod web3_signer;

pub use injection::{render_with_secrets, InjectedSecret, SecretResolver};
pub use leak_detector::{LeakDecision, LeakDetector};
pub use placeholder::{extract_secret_refs, SecretRef};
pub use types::{DecryptedSecret, SecretError};
pub use vault::{resolve_master_key, SecretVault};
pub use provider::{ProviderStatus, SecretMetadata, SecretProvider};
pub use web3_signer::{EvmSigner, SignIntent, SignedResult};
