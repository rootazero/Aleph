//! MCP Manager Configuration Persistence
//!
//! This module provides persistent storage for MCP server configurations.
//! Configurations are stored in JSON format and support environment variable expansion.
//!
//! # Features
//!
//! - JSON serialization for human-readable config files
//! - Environment variable expansion with `${VAR}` syntax
//! - Auto-start filtering for startup initialization
//! - Atomic save with parent directory creation
//!
//! # Example
//!
//! ```ignore
//! use alephcore::mcp::manager::McpPersistentConfig;
//! use std::path::Path;
//!
//! // Load configuration
//! let mut config = McpPersistentConfig::load(McpPersistentConfig::default_path().as_path()).await?;
//!
//! // Add a server
//! config.upsert_server(McpManagerConfig::stdio("my-server", "My Server", "npx"));
//!
//! // Expand environment variables before use
//! config.expand_env_vars();
//!
//! // Save configuration
//! config.save(McpPersistentConfig::default_path().as_path()).await?;
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::types::McpManagerConfig;
use crate::error::{AlephError, Result};

/// Persistent configuration for MCP Manager
///
/// This struct holds all MCP server configurations and provides methods for
/// loading, saving, and manipulating the configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPersistentConfig {
    /// Configuration version for future migrations
    #[serde(default = "default_version")]
    pub version: u32,

    /// Map of server ID to server configuration
    #[serde(default)]
    pub servers: HashMap<String, McpManagerConfig>,
}

fn default_version() -> u32 {
    1
}

impl Default for McpPersistentConfig {
    fn default() -> Self {
        Self {
            version: 1,
            servers: HashMap::new(),
        }
    }
}

impl McpPersistentConfig {
    /// Get the default configuration file path
    ///
    /// Returns `~/.aleph/mcp_config.json`
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aether")
            .join("mcp_config.json")
    }

    /// Load configuration from a file
    ///
    /// If the file does not exist, returns a default empty configuration.
    /// If the file exists but cannot be parsed, returns an error.
    pub async fn load(path: &Path) -> Result<Self> {
        match tokio::fs::read_to_string(path).await {
            Ok(contents) => {
                serde_json::from_str(&contents).map_err(|e| {
                    AlephError::ConfigError {
                        message: format!("Failed to parse MCP config at {}: {}", path.display(), e),
                        suggestion: Some(
                            "Check the JSON syntax in your MCP configuration file".to_string(),
                        ),
                    }
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("MCP config not found at {}, using default", path.display());
                Ok(Self::default())
            }
            Err(e) => Err(AlephError::IoError(format!(
                "Failed to read MCP config at {}: {}",
                path.display(),
                e
            ))),
        }
    }

    /// Save configuration to a file
    ///
    /// Creates parent directories if they don't exist.
    /// Writes pretty-printed JSON for human readability.
    pub async fn save(&self, path: &Path) -> Result<()> {
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to create config directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        // Serialize to pretty JSON
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize MCP config: {}", e))
        })?;

        // Write atomically by writing to temp file and renaming
        let temp_path = path.with_extension("json.tmp");
        tokio::fs::write(&temp_path, &json).await.map_err(|e| {
            AlephError::IoError(format!(
                "Failed to write MCP config to {}: {}",
                temp_path.display(),
                e
            ))
        })?;

        tokio::fs::rename(&temp_path, path).await.map_err(|e| {
            AlephError::IoError(format!(
                "Failed to rename temp config to {}: {}",
                path.display(),
                e
            ))
        })?;

        tracing::debug!("Saved MCP config to {}", path.display());
        Ok(())
    }

    /// Add or update a server configuration
    ///
    /// If a server with the same ID exists, it will be replaced.
    pub fn upsert_server(&mut self, config: McpManagerConfig) {
        self.servers.insert(config.id.clone(), config);
    }

    /// Remove a server configuration by ID
    ///
    /// Returns the removed configuration if it existed.
    pub fn remove_server(&mut self, id: &str) -> Option<McpManagerConfig> {
        self.servers.remove(id)
    }

    /// Get a server configuration by ID
    pub fn get_server(&self, id: &str) -> Option<&McpManagerConfig> {
        self.servers.get(id)
    }

    /// Get all servers that should auto-start
    ///
    /// Returns references to all configurations where `auto_start` is true.
    pub fn auto_start_servers(&self) -> Vec<&McpManagerConfig> {
        self.servers
            .values()
            .filter(|config| config.auto_start)
            .collect()
    }

    /// Expand environment variables in all server configurations
    ///
    /// Expands `${VAR}` patterns in:
    /// - Environment variable values
    /// - Command path
    /// - Command arguments
    /// - URL
    pub fn expand_env_vars(&mut self) {
        for config in self.servers.values_mut() {
            // Expand in env values
            let expanded_env: HashMap<String, String> = config
                .env
                .iter()
                .map(|(k, v)| {
                    let expanded = expand_env_var(v).unwrap_or_else(|| v.clone());
                    (k.clone(), expanded)
                })
                .collect();
            config.env = expanded_env;

            // Expand in command
            if let Some(ref cmd) = config.command {
                if let Some(expanded) = expand_env_var(cmd) {
                    config.command = Some(expanded);
                }
            }

            // Expand in args
            config.args = config
                .args
                .iter()
                .map(|arg| expand_env_var(arg).unwrap_or_else(|| arg.clone()))
                .collect();

            // Expand in URL
            if let Some(ref url) = config.url {
                if let Some(expanded) = expand_env_var(url) {
                    config.url = Some(expanded);
                }
            }
        }
    }
}

/// Expand environment variables in a string
///
/// Supports `${VAR}` syntax. Returns `None` if no variables were found,
/// or `Some(expanded)` if at least one variable was expanded.
///
/// Unknown variables are left as-is.
fn expand_env_var(s: &str) -> Option<String> {
    // Match ${VAR_NAME} pattern
    let re = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("Invalid regex pattern");

    let mut result = s.to_string();
    let mut found = false;

    for caps in re.captures_iter(s) {
        let full_match = caps.get(0).unwrap().as_str();
        let var_name = caps.get(1).unwrap().as_str();

        if let Ok(value) = std::env::var(var_name) {
            result = result.replace(full_match, &value);
            found = true;
        }
        // If env var not found, leave the ${VAR} as-is
    }

    if found {
        Some(result)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_path() {
        let path = McpPersistentConfig::default_path();
        assert!(path.ends_with("mcp_config.json"));
        assert!(path.to_string_lossy().contains(".aether"));
    }

    #[test]
    fn test_expand_env_var_basic() {
        std::env::set_var("TEST_MCP_VAR", "expanded_value");

        let result = expand_env_var("prefix_${TEST_MCP_VAR}_suffix");
        assert_eq!(result, Some("prefix_expanded_value_suffix".to_string()));

        std::env::remove_var("TEST_MCP_VAR");
    }

    #[test]
    fn test_expand_env_var_multiple() {
        std::env::set_var("TEST_VAR1", "one");
        std::env::set_var("TEST_VAR2", "two");

        let result = expand_env_var("${TEST_VAR1}/${TEST_VAR2}");
        assert_eq!(result, Some("one/two".to_string()));

        std::env::remove_var("TEST_VAR1");
        std::env::remove_var("TEST_VAR2");
    }

    #[test]
    fn test_expand_env_var_no_match() {
        let result = expand_env_var("no variables here");
        assert_eq!(result, None);
    }

    #[test]
    fn test_expand_env_var_unknown() {
        // Unknown variables are left as-is
        let result = expand_env_var("${UNKNOWN_VAR_12345}");
        assert_eq!(result, None); // No expansion happened, returns None
    }

    #[test]
    fn test_expand_env_var_partial() {
        std::env::set_var("TEST_KNOWN", "known");

        // One known, one unknown
        let result = expand_env_var("${TEST_KNOWN}/${UNKNOWN_VAR_12345}");
        assert_eq!(result, Some("known/${UNKNOWN_VAR_12345}".to_string()));

        std::env::remove_var("TEST_KNOWN");
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("nonexistent.json");

        let config = McpPersistentConfig::load(&config_path).await.unwrap();
        assert_eq!(config.version, 1);
        assert!(config.servers.is_empty());
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("mcp_config.json");

        // Create config with a server
        let mut config = McpPersistentConfig::default();
        config.upsert_server(McpManagerConfig::stdio("test-server", "Test Server", "/usr/bin/test"));

        // Save
        config.save(&config_path).await.unwrap();

        // Load
        let loaded = McpPersistentConfig::load(&config_path).await.unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.servers.len(), 1);
        assert!(loaded.servers.contains_key("test-server"));

        let server = loaded.get_server("test-server").unwrap();
        assert_eq!(server.name, "Test Server");
        assert_eq!(server.command, Some("/usr/bin/test".to_string()));
    }

    #[tokio::test]
    async fn test_save_creates_parent_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir
            .path()
            .join("nested")
            .join("dir")
            .join("config.json");

        let config = McpPersistentConfig::default();
        config.save(&config_path).await.unwrap();

        assert!(config_path.exists());
    }

    #[test]
    fn test_upsert_server() {
        let mut config = McpPersistentConfig::default();

        // Add first server
        config.upsert_server(McpManagerConfig::stdio("server1", "Server 1", "/bin/cmd1"));
        assert_eq!(config.servers.len(), 1);

        // Add second server
        config.upsert_server(McpManagerConfig::stdio("server2", "Server 2", "/bin/cmd2"));
        assert_eq!(config.servers.len(), 2);

        // Update first server
        config.upsert_server(McpManagerConfig::stdio("server1", "Updated Server 1", "/bin/cmd1-updated"));
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.get_server("server1").unwrap().name, "Updated Server 1");
    }

    #[test]
    fn test_remove_server() {
        let mut config = McpPersistentConfig::default();
        config.upsert_server(McpManagerConfig::stdio("server1", "Server 1", "/bin/cmd1"));
        config.upsert_server(McpManagerConfig::stdio("server2", "Server 2", "/bin/cmd2"));

        // Remove existing
        let removed = config.remove_server("server1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, "server1");
        assert_eq!(config.servers.len(), 1);

        // Remove non-existing
        let removed = config.remove_server("nonexistent");
        assert!(removed.is_none());
        assert_eq!(config.servers.len(), 1);
    }

    #[test]
    fn test_get_server() {
        let mut config = McpPersistentConfig::default();
        config.upsert_server(McpManagerConfig::stdio("server1", "Server 1", "/bin/cmd1"));

        assert!(config.get_server("server1").is_some());
        assert!(config.get_server("nonexistent").is_none());
    }

    #[test]
    fn test_auto_start_servers() {
        let mut config = McpPersistentConfig::default();

        // Add server with auto_start = true (default)
        config.upsert_server(McpManagerConfig::stdio("auto1", "Auto 1", "/bin/cmd1"));

        // Add server with auto_start = false
        config.upsert_server(
            McpManagerConfig::stdio("manual1", "Manual 1", "/bin/cmd2").with_auto_start(false),
        );

        // Add another auto-start server
        config.upsert_server(McpManagerConfig::stdio("auto2", "Auto 2", "/bin/cmd3"));

        let auto_servers = config.auto_start_servers();
        assert_eq!(auto_servers.len(), 2);

        let auto_ids: Vec<&str> = auto_servers.iter().map(|s| s.id.as_str()).collect();
        assert!(auto_ids.contains(&"auto1"));
        assert!(auto_ids.contains(&"auto2"));
        assert!(!auto_ids.contains(&"manual1"));
    }

    #[test]
    fn test_expand_env_vars_config() {
        std::env::set_var("TEST_API_KEY", "secret123");
        std::env::set_var("TEST_HOME", "/home/user");

        let mut config = McpPersistentConfig::default();

        let mut server = McpManagerConfig::stdio("test", "Test", "${TEST_HOME}/bin/server");
        server.args = vec!["--config".to_string(), "${TEST_HOME}/config.json".to_string()];
        server.env.insert("API_KEY".to_string(), "${TEST_API_KEY}".to_string());

        config.upsert_server(server);
        config.expand_env_vars();

        let server = config.get_server("test").unwrap();
        assert_eq!(server.command, Some("/home/user/bin/server".to_string()));
        assert_eq!(server.args[1], "/home/user/config.json");
        assert_eq!(server.env.get("API_KEY").unwrap(), "secret123");

        std::env::remove_var("TEST_API_KEY");
        std::env::remove_var("TEST_HOME");
    }

    #[tokio::test]
    async fn test_load_invalid_json() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("invalid.json");

        tokio::fs::write(&config_path, "{ invalid json }")
            .await
            .unwrap();

        let result = McpPersistentConfig::load(&config_path).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AlephError::ConfigError { .. }));
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut config = McpPersistentConfig::default();
        config.upsert_server(
            McpManagerConfig::stdio("test", "Test Server", "/usr/bin/test")
                .with_args(vec!["--verbose".to_string()])
                .with_runtime("node")
                .with_timeout(60),
        );

        let json = serde_json::to_string_pretty(&config).unwrap();
        let deserialized: McpPersistentConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.version, config.version);
        assert_eq!(deserialized.servers.len(), config.servers.len());

        let server = deserialized.get_server("test").unwrap();
        assert_eq!(server.name, "Test Server");
        assert_eq!(server.requires_runtime, Some("node".to_string()));
        assert_eq!(server.timeout_seconds, Some(60));
    }
}
