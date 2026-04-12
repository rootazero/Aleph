use crate::gateway::interfaces::whatsapp::config::WhatsAppConfig;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq)]
pub enum WhatsAppClientKind {
    Native,
    Bridge,
}

pub struct FallbackManager {
    config: WhatsAppConfig,
    client_kind: Arc<RwLock<WhatsAppClientKind>>,
}

impl FallbackManager {
    pub fn new(config: WhatsAppConfig) -> Self {
        Self {
            config,
            client_kind: Arc::new(RwLock::new(WhatsAppClientKind::Bridge)),
        }
    }

    pub async fn connect(&self) -> WhatsAppClientKind {
        #[cfg(feature = "native-whatsapp")]
        {
            match self.try_native().await {
                Ok(_) => {
                    tracing::info!("WhatsApp: using native Rust client");
                    *self.client_kind.write().await = WhatsAppClientKind::Native;
                    return WhatsAppClientKind::Native;
                }
                Err(e) => {
                    tracing::warn!("WhatsApp native client failed: {}, falling back to bridge", e);
                }
            }
        }

        tracing::info!("WhatsApp: using Go bridge client");
        *self.client_kind.write().await = WhatsAppClientKind::Bridge;
        WhatsAppClientKind::Bridge
    }

    #[cfg(feature = "native-whatsapp")]
    async fn try_native(&self) -> Result<(), String> {
        use crate::gateway::interfaces::whatsapp::native_baileys::{AuthManager, NativeBaileysError};

        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".aleph")
            .join("channels")
            .join("whatsapp");

        let auth_manager = AuthManager::new(self.config.phone_number.as_deref().unwrap_or("default"), base_dir);

        match auth_manager.load_auth().await {
            Ok(_) => {
                tracing::info!("WhatsApp native: found existing auth");
                Ok(())
            }
            Err(e) => {
                tracing::info!("WhatsApp native: no existing auth: {}", e);
                Err(e.to_string())
            }
        }
    }

    pub async fn get_client_kind(&self) -> WhatsAppClientKind {
        self.client_kind.read().await.clone()
    }
}

impl Clone for FallbackManager {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            client_kind: Arc::clone(&self.client_kind),
        }
    }
}
