//! Authentication and configuration management
//!
//! Provides ConfigStore trait for platform-specific token storage.

use crate::Result;

#[cfg(feature = "client")]
use async_trait::async_trait;

/// Authentication token
pub type AuthToken = String;

/// Configuration store trait
///
/// Clients implement this to provide platform-specific storage
/// (e.g., file-based for CLI, Tauri store for desktop, Keychain for macOS).
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Load authentication token from storage
    async fn load_token(&self) -> Result<Option<String>>;

    /// Save authentication token to storage
    async fn save_token(&self, token: &str) -> Result<()>;

    /// Clear authentication token from storage
    async fn clear_token(&self) -> Result<()>;

    /// Get or create device ID
    async fn get_or_create_device_id(&self) -> String;
}
