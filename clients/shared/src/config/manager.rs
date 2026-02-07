//! Configuration Manager
//!
//! 4-layer configuration stack for SDK clients:
//! - Layer 0: Hardcoded defaults
//! - Layer 1: Local persistent configuration
//! - Layer 2: Server-synced configuration
//! - Layer 3: Session overrides (runtime-only)

use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

/// Configuration Manager with 4-layer priority stack
pub struct ConfigManager {
    /// Layer 0: Hardcoded defaults
    defaults: HashMap<String, Value>,
    /// Layer 1: Local persistent configuration (path stored for future persistence)
    local_path: PathBuf,
    local: RwLock<HashMap<String, Value>>,
    /// Layer 2: Server-synced configuration
    server: RwLock<HashMap<String, Value>>,
    /// Layer 3: Session overrides (runtime-only)
    session: RwLock<HashMap<String, Value>>,
}

impl ConfigManager {
    /// Create a new ConfigManager with hardcoded defaults
    ///
    /// # Arguments
    /// * `local_path` - Path to local configuration file (not used until Task 6)
    pub fn new(local_path: PathBuf) -> Self {
        let mut defaults = HashMap::new();
        defaults.insert("ui.theme".to_string(), Value::String("system".to_string()));
        defaults.insert("log.level".to_string(), Value::String("info".to_string()));

        Self {
            defaults,
            local_path,
            local: RwLock::new(HashMap::new()),
            server: RwLock::new(HashMap::new()),
            session: RwLock::new(HashMap::new()),
        }
    }

    /// Get configuration value with 4-layer priority (Session > Server > Local > Default)
    ///
    /// # Arguments
    /// * `key` - Configuration key (e.g., "ui.theme")
    ///
    /// # Returns
    /// * `Some(Value)` if found in any layer
    /// * `None` if key doesn't exist
    pub async fn get(&self, key: &str) -> Option<Value> {
        // Layer 3: Session override (highest priority)
        if let Some(value) = self.session.read().await.get(key) {
            return Some(value.clone());
        }

        // Layer 2: Server synced
        if let Some(value) = self.server.read().await.get(key) {
            return Some(value.clone());
        }

        // Layer 1: Local persistent
        if let Some(value) = self.local.read().await.get(key) {
            return Some(value.clone());
        }

        // Layer 0: Hardcoded defaults
        self.defaults.get(key).cloned()
    }

    /// Load local configuration from disk
    ///
    /// # Returns
    /// * `Ok(())` if file loaded successfully or doesn't exist
    /// * `Err(String)` on I/O or parse errors
    pub async fn load_local(&self) -> Result<(), String> {
        if !self.local_path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&self.local_path)
            .await
            .map_err(|e| format!("Failed to read config file: {}", e))?;

        let config: HashMap<String, Value> = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config JSON: {}", e))?;

        *self.local.write().await = config;
        Ok(())
    }

    /// Set local configuration value
    ///
    /// # Arguments
    /// * `key` - Configuration key
    /// * `value` - Configuration value
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(String)` on failure
    pub async fn set_local(&self, key: &str, value: Value) -> Result<(), String> {
        self.local.write().await.insert(key.to_string(), value);

        // Persist to disk
        let local_snapshot = self.local.read().await.clone();
        let json_str = serde_json::to_string_pretty(&local_snapshot)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        tokio::fs::write(&self.local_path, json_str)
            .await
            .map_err(|e| format!("Failed to write config file: {}", e))?;

        Ok(())
    }

    /// Sync configuration from server
    ///
    /// # Arguments
    /// * `server_config` - Server configuration to sync
    pub async fn sync_from_server(&self, server_config: HashMap<String, Value>) {
        *self.server.write().await = server_config;
    }

    /// Set session override (runtime-only)
    ///
    /// # Arguments
    /// * `key` - Configuration key
    /// * `value` - Configuration value
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(String)` if key is Tier 1 (cannot be overridden)
    ///
    /// # Security
    /// Tier 1 keys (auth.*, security.*, identity.*) cannot be overridden
    pub async fn set_session(&self, key: &str, value: Value) -> Result<(), String> {
        if is_tier1_key(key) {
            return Err(format!(
                "Cannot override Tier 1 key '{}' in session",
                key
            ));
        }

        self.session.write().await.insert(key.to_string(), value);
        Ok(())
    }

    /// Clear all session overrides
    pub async fn clear_session_overrides(&self) {
        self.session.write().await.clear();
    }
}

/// Check if a key is Tier 1 (cannot be overridden in session)
///
/// # Arguments
/// * `key` - Configuration key to check
///
/// # Returns
/// * `true` if key is Tier 1 (auth.*, security.*, identity.*)
/// * `false` otherwise
fn is_tier1_key(key: &str) -> bool {
    key.starts_with("auth.") || key.starts_with("security.") || key.starts_with("identity.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_layer() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Test hardcoded defaults
        assert_eq!(
            config.get("ui.theme").await,
            Some(Value::String("system".to_string()))
        );
        assert_eq!(
            config.get("log.level").await,
            Some(Value::String("info".to_string()))
        );

        // Non-existent key
        assert_eq!(config.get("non.existent").await, None);
    }

    #[tokio::test]
    async fn test_local_overrides_default() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Set local override
        config.set_local("ui.theme", Value::String("dark".to_string())).await.unwrap();

        // Local should override default
        assert_eq!(
            config.get("ui.theme").await,
            Some(Value::String("dark".to_string()))
        );

        // Other default should remain
        assert_eq!(
            config.get("log.level").await,
            Some(Value::String("info".to_string()))
        );
    }

    #[tokio::test]
    async fn test_server_overrides_local() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Set local
        config.set_local("ui.theme", Value::String("dark".to_string())).await.unwrap();

        // Sync from server
        let mut server_config = HashMap::new();
        server_config.insert("ui.theme".to_string(), Value::String("light".to_string()));
        config.sync_from_server(server_config).await;

        // Server should override local
        assert_eq!(
            config.get("ui.theme").await,
            Some(Value::String("light".to_string()))
        );
    }

    #[tokio::test]
    async fn test_session_overrides_all() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Set local
        config.set_local("ui.theme", Value::String("dark".to_string())).await.unwrap();

        // Sync from server
        let mut server_config = HashMap::new();
        server_config.insert("ui.theme".to_string(), Value::String("light".to_string()));
        config.sync_from_server(server_config).await;

        // Set session override
        config.set_session("ui.theme", Value::String("auto".to_string())).await.unwrap();

        // Session should override all
        assert_eq!(
            config.get("ui.theme").await,
            Some(Value::String("auto".to_string()))
        );
    }

    #[tokio::test]
    async fn test_tier1_cannot_be_overridden() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Try to set Tier 1 keys in session
        let auth_result = config.set_session("auth.token", Value::String("fake".to_string())).await;
        assert!(auth_result.is_err());
        assert!(auth_result.unwrap_err().contains("Tier 1"));

        let security_result = config.set_session("security.level", Value::String("low".to_string())).await;
        assert!(security_result.is_err());
        assert!(security_result.unwrap_err().contains("Tier 1"));

        let identity_result = config.set_session("identity.user", Value::String("fake".to_string())).await;
        assert!(identity_result.is_err());
        assert!(identity_result.unwrap_err().contains("Tier 1"));

        // Non-Tier 1 key should work
        let ui_result = config.set_session("ui.theme", Value::String("dark".to_string())).await;
        assert!(ui_result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_session_overrides() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Set local
        config.set_local("ui.theme", Value::String("dark".to_string())).await.unwrap();

        // Set session override
        config.set_session("ui.theme", Value::String("auto".to_string())).await.unwrap();
        assert_eq!(
            config.get("ui.theme").await,
            Some(Value::String("auto".to_string()))
        );

        // Clear session overrides
        config.clear_session_overrides().await;

        // Should fall back to local
        assert_eq!(
            config.get("ui.theme").await,
            Some(Value::String("dark".to_string()))
        );
    }

    #[tokio::test]
    async fn test_local_persistence() {
        // Use unique temp file to avoid conflicts
        let temp_file = format!("/tmp/config_test_{}.json", std::process::id());
        let config_path = PathBuf::from(&temp_file);

        // Clean up any existing file
        let _ = std::fs::remove_file(&config_path);

        // Create first manager and set config
        {
            let manager1 = ConfigManager::new(config_path.clone());
            manager1.set_local("ui.theme", Value::String("dark".to_string()))
                .await
                .unwrap();
            manager1.set_local("log.level", Value::String("debug".to_string()))
                .await
                .unwrap();

            // Verify values are set
            assert_eq!(
                manager1.get("ui.theme").await,
                Some(Value::String("dark".to_string()))
            );
            assert_eq!(
                manager1.get("log.level").await,
                Some(Value::String("debug".to_string()))
            );
        } // manager1 dropped here

        // Create second manager and load from disk
        {
            let manager2 = ConfigManager::new(config_path.clone());
            manager2.load_local().await.unwrap();

            // Verify values persisted
            assert_eq!(
                manager2.get("ui.theme").await,
                Some(Value::String("dark".to_string()))
            );
            assert_eq!(
                manager2.get("log.level").await,
                Some(Value::String("debug".to_string()))
            );
        }

        // Clean up
        let _ = std::fs::remove_file(&config_path);
    }
}
