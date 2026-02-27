//! Embedding manager — manages provider lifecycle and active provider switching.

use crate::config::types::memory::{EmbeddingProviderConfig, EmbeddingSettings};
use crate::error::AlephError;
use crate::memory::embedding_provider::{create_provider, EmbeddingProvider, RemoteEmbeddingProvider};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Manages embedding provider lifecycle
pub struct EmbeddingManager {
    settings: Arc<RwLock<EmbeddingSettings>>,
    active_provider: Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
}

impl EmbeddingManager {
    /// Create a new EmbeddingManager from settings
    pub fn new(settings: EmbeddingSettings) -> Self {
        Self {
            settings: Arc::new(RwLock::new(settings)),
            active_provider: Arc::new(RwLock::new(None)),
        }
    }

    /// Initialize the active provider from current settings.
    /// Returns Ok(()) even if no provider is configured (degrades gracefully).
    pub async fn init(&self) -> Result<(), AlephError> {
        let (active_id, config) = {
            let settings = self.settings.read().await;
            let active_id = settings.active_provider_id.clone();
            let config = settings.providers.iter().find(|p| p.id == active_id).cloned();
            (active_id, config)
        }; // settings lock released

        if let Some(config) = config {
            match create_provider(&config) {
                Ok(provider) => {
                    *self.active_provider.write().await = Some(provider);
                    info!(provider_id = %active_id, "Embedding provider initialized");
                }
                Err(e) => {
                    warn!(provider_id = %active_id, error = %e, "Failed to initialize embedding provider");
                }
            }
        } else {
            warn!("No active embedding provider configured (id={})", active_id);
        }

        Ok(())
    }

    /// Get the currently active provider. Returns None if not configured.
    pub async fn get_active_provider(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.active_provider.read().await.clone()
    }

    /// Get the active provider or return an error.
    pub async fn require_active_provider(&self) -> Result<Arc<dyn EmbeddingProvider>, AlephError> {
        self.get_active_provider().await.ok_or_else(|| {
            AlephError::config("No active embedding provider configured. Please configure one in Settings > Embedding Providers.".to_string())
        })
    }

    /// Switch the active provider.
    ///
    /// Multi-dimension vector columns (vec_768, vec_1024, vec_1536) allow
    /// different providers to coexist — no need to clear the vector store.
    pub async fn switch_provider(&self, new_id: &str) -> Result<(), AlephError> {
        // Extract config, then drop the settings lock before creating the provider.
        // This avoids holding two write locks simultaneously.
        let config = {
            let settings = self.settings.read().await;
            settings
                .providers
                .iter()
                .find(|p| p.id == new_id)
                .ok_or_else(|| AlephError::config(format!("Provider not found: {}", new_id)))?
                .clone()
        }; // settings lock released

        // Create provider before mutating settings — if this fails, nothing changes.
        let provider = create_provider(&config)?;

        // Now update both settings and active provider
        let old_id = self.settings.read().await.active_provider_id.clone();
        self.settings.write().await.active_provider_id = new_id.to_string();
        *self.active_provider.write().await = Some(provider);

        info!(old = %old_id, new = %new_id, "Embedding provider switched");

        Ok(())
    }

    /// Test a specific provider's connectivity
    pub async fn test_provider(&self, provider_id: &str) -> Result<(), AlephError> {
        let settings = self.settings.read().await;
        let config = settings
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| AlephError::config(format!("Provider not found: {}", provider_id)))?;

        let provider = RemoteEmbeddingProvider::from_config(config)?;
        provider.test_connection().await
    }

    /// Test a provider config without it being saved (for "test connection" button)
    pub async fn test_config(config: &EmbeddingProviderConfig) -> Result<(), AlephError> {
        let provider = RemoteEmbeddingProvider::from_config(config)?;
        provider.test_connection().await
    }

    /// Update the internal settings (called after config save)
    pub async fn update_settings(&self, settings: EmbeddingSettings) {
        *self.settings.write().await = settings;
    }

    /// Get a snapshot of current settings
    pub async fn get_settings(&self) -> EmbeddingSettings {
        self.settings.read().await.clone()
    }
}
