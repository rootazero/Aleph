//! `WeChat` Authentication & Token Management
//!
//! QR login flow, context token storage, and session persistence.

use std::collections::HashMap;
use std::path::Path;
use tokio::sync::RwLock;

/// Context token store for per-conversation tokens.
/// Each (`account_id`, `user_id`) pair has its own `context_token`.
pub struct ContextTokenStore {
    root: std::path::PathBuf,
    cache: RwLock<HashMap<String, String>>,
}

impl ContextTokenStore {
    #[must_use]
    pub fn new(hermes_home: &str) -> Self {
        let root = Path::new(hermes_home).join("wechat").join("accounts");
        Self {
            root,
            cache: RwLock::new(HashMap::new()),
        }
    }

    fn key(account_id: &str, user_id: &str) -> String {
        format!("{account_id}:{user_id}")
    }

    fn cache_path(&self, account_id: &str) -> std::path::PathBuf {
        self.root.join(format!("{account_id}.context-tokens.json"))
    }

    /// Get context token for a user.
    pub async fn get(&self, account_id: &str, user_id: &str) -> Option<String> {
        let cache = self.cache.read().await;
        cache.get(&Self::key(account_id, user_id)).cloned()
    }

    /// Set context token for a user.
    pub async fn set(&self, account_id: &str, user_id: &str, token: String) {
        {
            let mut cache = self.cache.write().await;
            cache.insert(Self::key(account_id, user_id), token);
        }
        self.persist(account_id).await;
    }

    /// Restore tokens from disk for an account.
    pub async fn restore(&self, account_id: &str) {
        let path = self.cache_path(account_id);
        if !path.exists() {
            return;
        }
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            if let Ok(data) = serde_json::from_str::<HashMap<String, String>>(&content) {
                let mut cache = self.cache.write().await;
                for (user_id, token) in data {
                    cache.insert(format!("{account_id}:{user_id}"), token);
                }
            }
        }
    }

    async fn persist(&self, account_id: &str) {
        let payload = {
            let cache = self.cache.read().await;
            let mut payload = HashMap::new();
            let prefix = format!("{account_id}:");
            for (key, value) in cache.iter() {
                if key.starts_with(&prefix) {
                    payload.insert(key[prefix.len()..].to_string(), value.clone());
                }
            }
            payload
        };
        let path = self.cache_path(account_id);
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let tmp_path = path.with_extension("tmp");
        if let Ok(json) = serde_json::to_string_pretty(&payload) {
            let _ = tokio::fs::write(&tmp_path, json).await;
            let _ = tokio::fs::rename(&tmp_path, &path).await;
        }
    }
}

/// QR Login result credentials.
#[derive(Debug, Clone)]
pub struct QrLoginResult {
    pub account_id: String,
    pub token: String,
    pub base_url: String,
    pub user_id: String,
}

/// Perform QR login flow (requires interactive terminal).
pub async fn qr_login(_hermes_home: &str, _bot_type: &str) -> Result<QrLoginResult, String> {
    Err("QR login requires terminal interaction. Use CLI tool instead.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_context_token_store_get_set() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = ContextTokenStore::new(temp_dir.path().to_str().unwrap());

        store
            .set("account1", "user1", "token_abc".to_string())
            .await;
        assert_eq!(
            store.get("account1", "user1").await,
            Some("token_abc".to_string())
        );
        assert_eq!(store.get("account1", "user2").await, None);
    }

    #[tokio::test]
    async fn test_context_token_store_persistence() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        let store = ContextTokenStore::new(path);
        store
            .set("account1", "user1", "token_xyz".to_string())
            .await;

        let new_store = ContextTokenStore::new(path);
        new_store.restore("account1").await;
        assert_eq!(
            new_store.get("account1", "user1").await,
            Some("token_xyz".to_string())
        );
    }
}
