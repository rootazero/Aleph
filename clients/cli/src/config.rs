//! Configuration management for Aleph CLI

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::{CliError, CliResult};

/// CLI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// Default server URL
    #[serde(default = "default_server")]
    pub server: String,

    /// Device ID for this client
    #[serde(default = "default_device_id")]
    pub device_id: String,

    /// Device name
    #[serde(default = "default_device_name")]
    pub device_name: String,

    /// Authentication token (if authenticated)
    pub auth_token: Option<String>,

    /// Default session key
    pub default_session: Option<String>,

    /// Client manifest settings
    #[serde(default)]
    pub manifest: ManifestConfig,
}

/// Client manifest configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestConfig {
    /// Tool categories this client supports
    #[serde(default)]
    pub tool_categories: Vec<String>,

    /// Specific tools this client supports
    #[serde(default)]
    pub specific_tools: Vec<String>,

    /// Tools to exclude
    #[serde(default)]
    pub excluded_tools: Vec<String>,
}

fn default_server() -> String {
    "ws://127.0.0.1:18789".to_string()
}

fn default_device_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn default_device_name() -> String {
    "aleph-cli".to_string()
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            server: default_server(),
            device_id: default_device_id(),
            device_name: default_device_name(),
            auth_token: None,
            default_session: None,
            manifest: ManifestConfig::default(),
        }
    }
}

impl CliConfig {
    /// Get the default config file path
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aleph-cli")
            .join("config.toml")
    }

    /// Load configuration from file
    pub fn load(path: Option<&str>) -> CliResult<Self> {
        let config_path = path
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_path);

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| CliError::Config(format!("Failed to read config: {}", e)))?;

            toml::from_str(&content)
                .map_err(|e| CliError::Config(format!("Failed to parse config: {}", e)))
        } else {
            // Return default config if file doesn't exist
            Ok(Self::default())
        }
    }

    /// Save configuration to file
    pub fn save(&self, path: Option<&str>) -> CliResult<()> {
        let config_path = path
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_path);

        // Create parent directory if needed
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::Config(format!("Failed to create config dir: {}", e)))?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| CliError::Config(format!("Failed to serialize config: {}", e)))?;

        std::fs::write(&config_path, content)
            .map_err(|e| CliError::Config(format!("Failed to write config: {}", e)))?;

        Ok(())
    }

    /// Update auth token and save
    pub fn set_auth_token(&mut self, token: String, path: Option<&str>) -> CliResult<()> {
        self.auth_token = Some(token);
        self.save(path)
    }
}

// Add toml dependency
use serde as _;
