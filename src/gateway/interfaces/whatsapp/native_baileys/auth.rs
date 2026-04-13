use crate::gateway::interfaces::whatsapp::native_baileys::errors::NativeBaileysError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaAuthData {
    pub creds: Creds,
    pub keys: Keys,
    pub app_state_sync: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Creds {
    pub device_identity: Vec<u8>,
    pub session_id: String,
    pub noise_key: Vec<u8>,
    pub identity_key: Vec<u8>,
    pub signed_identity_key: Vec<u8>,
    pub registration_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keys {
    pub chat_state: Vec<u8>,
    pub session: Vec<u8>,
    pub sender_key: Vec<u8>,
    pub app_state_sync_key: Vec<u8>,
}

pub struct AuthManager {
    account_id: String,
    auth_path: PathBuf,
}

impl AuthManager {
    pub fn new(account_id: impl Into<String>, base_dir: PathBuf) -> Self {
        let account_id = account_id.into();
        let auth_path = base_dir.join("auth.json");
        Self {
            account_id,
            auth_path,
        }
    }

    pub async fn save_auth(&self, auth: &WaAuthData) -> Result<(), NativeBaileysError> {
        let json =
            serde_json::to_vec(auth).map_err(|e| NativeBaileysError::VaultError(e.to_string()))?;

        if let Some(parent) = self.auth_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                NativeBaileysError::VaultError(format!("failed to create dir: {}", e))
            })?;
        }

        fs::write(&self.auth_path, json)
            .await
            .map_err(|e| NativeBaileysError::VaultError(format!("failed to write auth: {}", e)))?;

        Ok(())
    }

    pub async fn load_auth(&self) -> Result<WaAuthData, NativeBaileysError> {
        if !self.auth_path.exists() {
            return Err(NativeBaileysError::AuthFailed("No existing auth".into()));
        }

        let json = fs::read(&self.auth_path)
            .await
            .map_err(|e| NativeBaileysError::VaultError(format!("failed to read auth: {}", e)))?;

        serde_json::from_slice(&json)
            .map_err(|e| NativeBaileysError::AuthFailed(format!("failed to parse auth: {}", e)))
    }

    pub fn auth_path(&self) -> &PathBuf {
        &self.auth_path
    }
}
