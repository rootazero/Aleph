//! Secret provider abstraction
//!
//! Defines the `SecretProvider` trait for pluggable secret backends
//! (local vault, 1Password, AWS Secrets Manager, etc.).

pub mod onepassword;

use async_trait::async_trait;

use super::types::SecretError;

/// Health status of a secret provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStatus {
    /// Provider is ready to serve secrets.
    Ready,
    /// Provider requires authentication before use.
    NeedsAuth { message: String },
    /// Provider is not available (e.g., network down, CLI missing).
    Unavailable { reason: String },
}

/// Trait for pluggable secret backends.
///
/// Each implementation encapsulates access to one secret source.
/// `SharedTokenManager` dispatches `get()` calls to the local vault.
/// External providers (e.g., 1Password) can be registered separately.
#[async_trait]
pub trait SecretProvider: Send + Sync {
    /// Returns a human-readable provider type identifier (e.g., "`local_vault`", "1password").
    fn provider_type(&self) -> &str;

    /// Check whether the provider is healthy and ready to serve.
    async fn health_check(&self) -> Result<ProviderStatus, SecretError>;
}
