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
use std::sync::RwLock;

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
    pub fn get(&self, key: &str) -> Option<Value> {
        // Layer 3: Session override (highest priority)
        if let Ok(session) = self.session.read() {
            if let Some(value) = session.get(key) {
                return Some(value.clone());
            }
        }

        // Layer 2: Server synced
        if let Ok(server) = self.server.read() {
            if let Some(value) = server.get(key) {
                return Some(value.clone());
            }
        }

        // Layer 1: Local persistent
        if let Ok(local) = self.local.read() {
            if let Some(value) = local.get(key) {
                return Some(value.clone());
            }
        }

        // Layer 0: Hardcoded defaults
        self.defaults.get(key).cloned()
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
    ///
    /// # Note
    /// Persistence to disk will be added in Task 6
    pub fn set_local(&self, key: &str, value: Value) -> Result<(), String> {
        if let Ok(mut local) = self.local.write() {
            local.insert(key.to_string(), value);
            // TODO: Persist to disk (Task 6)
            Ok(())
        } else {
            Err("Failed to acquire write lock on local config".to_string())
        }
    }

    /// Sync configuration from server
    ///
    /// # Arguments
    /// * `server_config` - Server configuration to sync
    pub fn sync_from_server(&self, server_config: HashMap<String, Value>) {
        if let Ok(mut server) = self.server.write() {
            *server = server_config;
        }
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
    pub fn set_session(&self, key: &str, value: Value) -> Result<(), String> {
        if is_tier1_key(key) {
            return Err(format!(
                "Cannot override Tier 1 key '{}' in session",
                key
            ));
        }

        if let Ok(mut session) = self.session.write() {
            session.insert(key.to_string(), value);
            Ok(())
        } else {
            Err("Failed to acquire write lock on session config".to_string())
        }
    }

    /// Clear all session overrides
    pub fn clear_session_overrides(&self) {
        if let Ok(mut session) = self.session.write() {
            session.clear();
        }
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

    #[test]
    fn test_default_layer() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Test hardcoded defaults
        assert_eq!(
            config.get("ui.theme"),
            Some(Value::String("system".to_string()))
        );
        assert_eq!(
            config.get("log.level"),
            Some(Value::String("info".to_string()))
        );

        // Non-existent key
        assert_eq!(config.get("non.existent"), None);
    }

    #[test]
    fn test_local_overrides_default() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Set local override
        config.set_local("ui.theme", Value::String("dark".to_string())).unwrap();

        // Local should override default
        assert_eq!(
            config.get("ui.theme"),
            Some(Value::String("dark".to_string()))
        );

        // Other default should remain
        assert_eq!(
            config.get("log.level"),
            Some(Value::String("info".to_string()))
        );
    }

    #[test]
    fn test_server_overrides_local() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Set local
        config.set_local("ui.theme", Value::String("dark".to_string())).unwrap();

        // Sync from server
        let mut server_config = HashMap::new();
        server_config.insert("ui.theme".to_string(), Value::String("light".to_string()));
        config.sync_from_server(server_config);

        // Server should override local
        assert_eq!(
            config.get("ui.theme"),
            Some(Value::String("light".to_string()))
        );
    }

    #[test]
    fn test_session_overrides_all() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Set local
        config.set_local("ui.theme", Value::String("dark".to_string())).unwrap();

        // Sync from server
        let mut server_config = HashMap::new();
        server_config.insert("ui.theme".to_string(), Value::String("light".to_string()));
        config.sync_from_server(server_config);

        // Set session override
        config.set_session("ui.theme", Value::String("auto".to_string())).unwrap();

        // Session should override all
        assert_eq!(
            config.get("ui.theme"),
            Some(Value::String("auto".to_string()))
        );
    }

    #[test]
    fn test_tier1_cannot_be_overridden() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Try to set Tier 1 keys in session
        let auth_result = config.set_session("auth.token", Value::String("fake".to_string()));
        assert!(auth_result.is_err());
        assert!(auth_result.unwrap_err().contains("Tier 1"));

        let security_result = config.set_session("security.level", Value::String("low".to_string()));
        assert!(security_result.is_err());
        assert!(security_result.unwrap_err().contains("Tier 1"));

        let identity_result = config.set_session("identity.user", Value::String("fake".to_string()));
        assert!(identity_result.is_err());
        assert!(identity_result.unwrap_err().contains("Tier 1"));

        // Non-Tier 1 key should work
        let ui_result = config.set_session("ui.theme", Value::String("dark".to_string()));
        assert!(ui_result.is_ok());
    }

    #[test]
    fn test_clear_session_overrides() {
        let config = ConfigManager::new(PathBuf::from("/tmp/config.json"));

        // Set local
        config.set_local("ui.theme", Value::String("dark".to_string())).unwrap();

        // Set session override
        config.set_session("ui.theme", Value::String("auto".to_string())).unwrap();
        assert_eq!(
            config.get("ui.theme"),
            Some(Value::String("auto".to_string()))
        );

        // Clear session overrides
        config.clear_session_overrides();

        // Should fall back to local
        assert_eq!(
            config.get("ui.theme"),
            Some(Value::String("dark".to_string()))
        );
    }
}
