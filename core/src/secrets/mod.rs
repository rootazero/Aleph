//! Secret management module
//!
//! Provides encrypted storage for sensitive credentials (API keys, tokens).
//! Uses AES-256-GCM with per-entry HKDF-SHA256 key derivation.

pub mod crypto;
pub mod injection;
pub mod leak_detector;
pub mod placeholder;
pub mod provider;
pub mod types;
pub mod vault;
pub mod web3_signer;

pub use injection::{render_with_secrets, AsyncSecretResolver, InjectedSecret};
pub use leak_detector::{LeakDecision, LeakDetector};
pub use placeholder::{extract_secret_refs, SecretRef};
pub use types::{DecryptedSecret, EntryMetadata, SecretError};
pub use vault::SecretVault;
pub use web3_signer::{EvmSigner, SecretResolver, SignIntent, SignedResult};
pub use provider::ProviderStatus;
