//! Gateway client state management for Tauri

use aleph_client_sdk::GatewayClient;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};

/// Gateway client state
///
/// Holds the shared GatewayClient instance for use across Tauri commands.
pub struct GatewayState {
    client: RwLock<Option<Arc<GatewayClient>>>,
}

impl GatewayState {
    /// Create a new empty state
    pub fn new() -> Self {
        Self {
            client: RwLock::new(None),
        }
    }

    /// Initialize with a gateway client
    pub async fn initialize(&self, client: Arc<GatewayClient>) {
        *self.client.write().await = Some(client);
    }

    /// Get the gateway client
    pub fn get_client(&self) -> Result<Arc<GatewayClient>> {
        // Try non-blocking read first
        match self.client.try_read() {
            Ok(guard) => guard.clone().ok_or_else(|| {
                AlephError::NotInitialized("Gateway client not initialized".to_string())
            }),
            Err(_) => Err(AlephError::NotInitialized(
                "Gateway client locked".to_string(),
            )),
        }
    }

    /// Check if initialized
    pub fn is_initialized(&self) -> bool {
        self.client.try_read().map(|g| g.is_some()).unwrap_or(false)
    }
}

impl Default for GatewayState {
    fn default() -> Self {
        Self::new()
    }
}
