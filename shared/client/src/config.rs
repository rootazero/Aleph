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

    /// Device name
    #[serde(default = "default_device_name")]
    pub device_name: String,

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
    // Single source of truth: the canonical gateway WS endpoint. The literal
    // `18789` (sans `/ws`) was a stale leftover from the 18789→18790 port
    // migration and silently broke the default connection.
    crate::DEFAULT_GATEWAY_URL.to_string()
}

fn default_device_name() -> String {
    "aleph-cli".to_string()
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            server: default_server(),
            device_name: default_device_name(),
            default_session: None,
            manifest: ManifestConfig::default(),
        }
    }
}

impl CliConfig {
    /// Get the default config file path
    #[must_use]
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aleph-cli")
            .join("config.toml")
    }

    /// Load configuration from file
    pub fn load(path: Option<&str>) -> CliResult<Self> {
        let config_path = path.map_or_else(Self::default_path, PathBuf::from);

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| CliError::Config(format!("Failed to read config: {e}")))?;

            toml::from_str(&content)
                .map_err(|e| CliError::Config(format!("Failed to parse config: {e}")))
        } else {
            // Return default config if file doesn't exist
            Ok(Self::default())
        }
    }

    /// Save configuration to file
    pub fn save(&self, path: Option<&str>) -> CliResult<()> {
        let config_path = path.map_or_else(Self::default_path, PathBuf::from);

        // Create parent directory if needed
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::Config(format!("Failed to create config dir: {e}")))?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| CliError::Config(format!("Failed to serialize config: {e}")))?;

        std::fs::write(&config_path, content)
            .map_err(|e| CliError::Config(format!("Failed to write config: {e}")))?;

        // Restrict to owner read/write so the config isn't world-readable
        // (default file mode is 0644). On filesystems without POSIX mode
        // bits (FAT32, 9P, Windows shares, etc.) chmod is silently a
        // no-op; surface the unexpected case at warn so the operator at
        // least sees that the world-readable mode stuck.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) =
                std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600))
            {
                tracing::warn!(
                    "could not chmod {} to 0600 (config may remain world-readable): {e}",
                    config_path.display()
                );
            }
        }

        Ok(())
    }
}
