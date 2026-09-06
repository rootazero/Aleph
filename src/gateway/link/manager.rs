//! `LinkManager` — central orchestrator for builtin messaging channels.
//!
//! The `LinkManager` manages builtin channel factories and link instance
//! configurations. On startup it scans `~/.aleph/links/` and creates each
//! enabled link using the registered builtin [`ChannelFactory`].
//!
//! # Standalone helpers
//!
//! [`scan_link_configs`] and [`expand_env_vars`] are public, stateless
//! functions usable independently of `LinkManager`.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use super::types::{BridgeId, LinkConfig};
use crate::gateway::channel::{ChannelError, ChannelFactory, ChannelId};

type ChannelMap = HashMap<ChannelId, Arc<Mutex<Box<dyn crate::gateway::channel::Channel>>>>;

// ---------------------------------------------------------------------------
// Scanning helpers
// ---------------------------------------------------------------------------

/// Scan a directory for `*.yaml` / `*.yml` files and parse them as [`LinkConfig`].
///
/// Files that cannot be read or parsed are logged and skipped.  The function
/// returns `Ok([])` if the directory does not exist.
pub async fn scan_link_configs(dir: &Path) -> Result<Vec<LinkConfig>, LinkManagerError> {
    let mut configs = Vec::new();

    if !tokio::fs::try_exists(dir).await.unwrap_or(false) {
        return Ok(configs);
    }

    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| LinkManagerError::IoError(format!("read_dir {}: {e}", dir.display())))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| LinkManagerError::IoError(format!("next_entry: {e}")))?
    {
        let path = entry.path();
        let is_yaml = path
            .extension()
            .is_some_and(|ext| ext == "yaml" || ext == "yml");

        if !is_yaml {
            continue;
        }

        match tokio::fs::read_to_string(&path).await {
            Ok(content) => match crate::yaml::from_str::<LinkConfig>(&content) {
                Ok(config) => {
                    info!(path = %path.display(), id = %config.id, "Loaded link config");
                    configs.push(config);
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to parse link config — skipping");
                }
            },
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to read link config file — skipping");
            }
        }
    }

    Ok(configs)
}

/// Maximum byte length of an expanded `${env.VAR}` value. See
/// [`expand_env_vars`] for the rationale.
const MAX_EXPANDED_ENV_LEN: usize = 4096;

/// Recursively expand `${env.VAR_NAME}` references in a JSON value.
///
/// * String values matching the pattern are replaced with the environment
///   variable's value.  If the variable is not set, the original string is
///   kept and a warning is logged.
/// * Object values are expanded key-by-key.
/// * Array values are expanded element-by-element.
/// * All other value types are returned unchanged.
///
/// **Bounds**: an expanded value larger than [`MAX_EXPANDED_ENV_LEN`] bytes
/// is rejected (the original `${env.…}` string is kept and a warning is
/// logged). A `link.yaml` referencing a multi-megabyte environment variable
/// would otherwise silently embed the whole blob into a link config —
/// settings keys are bounded by the channel adapter downstream, so the
/// blast radius is config bloat and confusing adapter errors. Every
/// expansion is also logged (var NAME only, never the value) so the audit
/// trail shows which env vars flowed into which link configs.
pub fn expand_env_vars(settings: &serde_json::Value) -> serde_json::Value {
    match settings {
        serde_json::Value::String(s) => {
            // Match "${env.VAR_NAME}" pattern.
            if let Some(rest) = s.strip_prefix("${env.") {
                if let Some(var_name) = rest.strip_suffix('}') {
                    match std::env::var(var_name) {
                        Ok(val) => {
                            if val.len() > MAX_EXPANDED_ENV_LEN {
                                warn!(
                                    var = var_name,
                                    len = val.len(),
                                    max = MAX_EXPANDED_ENV_LEN,
                                    "Environment variable value exceeds expansion limit; keeping reference unexpanded"
                                );
                            } else {
                                tracing::debug!(
                                    var = var_name,
                                    "expanded environment variable into link settings"
                                );
                                return serde_json::Value::String(val);
                            }
                        }
                        Err(_) => {
                            warn!(
                                var = var_name,
                                "Environment variable referenced in settings is not set"
                            );
                        }
                    }
                }
            }
            settings.clone()
        }
        serde_json::Value::Object(map) => {
            let expanded = map
                .iter()
                .map(|(k, v)| (k.clone(), expand_env_vars(v)))
                .collect();
            serde_json::Value::Object(expanded)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(expand_env_vars).collect())
        }
        _ => settings.clone(),
    }
}

// ---------------------------------------------------------------------------
// LinkManager
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of builtin messaging channels.
///
/// Builtin bridges (Telegram, Discord, etc.) are created via [`ChannelFactory`].
///
/// # Usage
///
/// ```rust,no_run
/// use std::path::PathBuf;
/// use std::sync::Arc;
/// use alephcore::gateway::link::LinkManager;
///
/// # async fn example() {
/// let base_dir = PathBuf::from(std::env::var("HOME").unwrap()).join(".aleph");
/// let manager = LinkManager::new(base_dir);
/// manager.start().await.unwrap();
/// # }
/// ```
pub struct LinkManager {
    /// Builtin channel factories keyed by bridge id.
    builtin_factories: RwLock<HashMap<BridgeId, Arc<dyn ChannelFactory>>>,

    /// Active builtin channel instances keyed by channel id.
    ///
    /// Wrapped in `Arc<Mutex<>>` because [`Channel::start`] / [`Channel::stop`]
    /// take `&mut self`.
    builtin_channels: RwLock<ChannelMap>,

    /// Base directory (typically `~/.aleph/`).
    base_dir: PathBuf,
}

impl LinkManager {
    /// Create a new `LinkManager` rooted at `base_dir`.
    ///
    /// The manager expects the following directory layout under `base_dir`:
    ///
    /// ```text
    /// base_dir/
    ///   links/     — link instance config files (*.yaml)
    /// ```
    #[must_use]
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            builtin_factories: RwLock::new(HashMap::new()),
            builtin_channels: RwLock::new(HashMap::new()),
            base_dir,
        }
    }

    /// Register a builtin bridge type (e.g. Telegram, Discord).
    pub async fn register_builtin(&self, bridge_id: BridgeId, factory: Arc<dyn ChannelFactory>) {
        self.builtin_factories
            .write()
            .await
            .insert(bridge_id.clone(), factory);
        info!(bridge_id = %bridge_id, "Registered builtin bridge");
    }

    /// Full startup sequence.
    ///
    /// 1. Scans `{base_dir}/links/` for link instance configs.
    /// 2. Creates and starts each enabled link.
    ///
    /// Individual link failures are logged but do not abort the overall
    /// startup — the manager starts as many links as possible.
    pub async fn start(&self) -> Result<(), LinkManagerError> {
        let links_dir = self.base_dir.join("links");
        let link_configs = scan_link_configs(&links_dir).await?;

        for link in link_configs {
            if !link.enabled {
                info!(link_id = %link.id, "Skipping disabled link");
                continue;
            }

            if let Err(e) = self
                .start_builtin_link(&link, expand_env_vars(&link.settings))
                .await
            {
                error!(
                    link_id = %link.id,
                    bridge = %link.bridge,
                    error = %e,
                    "Failed to start link — continuing with remaining links"
                );
            }
        }

        info!("LinkManager startup complete");
        Ok(())
    }

    /// Stop all active builtin channels.
    pub async fn stop(&self) {
        let builtin_ids: Vec<ChannelId> = {
            let guard = self.builtin_channels.read().await;
            guard.keys().cloned().collect()
        };
        for id in builtin_ids {
            let channel = {
                let guard = self.builtin_channels.read().await;
                guard.get(&id).cloned()
            };
            if let Some(ch) = channel {
                let mut guard = ch.lock().await;
                if let Err(e) = guard.stop().await {
                    warn!(channel_id = %id, error = %e, "Error stopping builtin channel");
                }
            }
        }

        info!("LinkManager stopped all links");
    }

    /// List all active builtin channel ids.
    pub async fn list_channel_ids(&self) -> Vec<ChannelId> {
        self.builtin_channels.read().await.keys().cloned().collect()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    async fn start_builtin_link(
        &self,
        link: &LinkConfig,
        settings: serde_json::Value,
    ) -> Result<(), LinkManagerError> {
        let factory = {
            let factories = self.builtin_factories.read().await;
            factories
                .get(&link.bridge)
                .cloned()
                .ok_or_else(|| LinkManagerError::FactoryNotFound(link.bridge.to_string()))?
        };

        let mut channel = factory
            .create(settings)
            .await
            .map_err(|e| LinkManagerError::ChannelCreationFailed(e.to_string()))?;

        channel
            .start()
            .await
            .map_err(|e| LinkManagerError::ChannelStartFailed(e.to_string()))?;

        let channel_id = ChannelId::new(link.id.as_str());
        let wrapped = Arc::new(Mutex::new(channel));
        self.builtin_channels
            .write()
            .await
            .insert(channel_id.clone(), wrapped);

        info!(link_id = %link.id, channel_id = %channel_id, "Started builtin link");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LinkManagerError
// ---------------------------------------------------------------------------

/// Errors returned by [`LinkManager`] and its helper functions.
#[derive(Debug, thiserror::Error)]
pub enum LinkManagerError {
    #[error("IO error: {0}")]
    IoError(String),

    #[error("Factory not found for bridge: {0}")]
    FactoryNotFound(String),

    #[error("Channel creation failed: {0}")]
    ChannelCreationFailed(String),

    #[error("Channel start failed: {0}")]
    ChannelStartFailed(String),

    #[error("Config parse error: {0}")]
    ConfigParseError(String),
}

impl From<ChannelError> for LinkManagerError {
    fn from(e: ChannelError) -> Self {
        Self::ChannelCreationFailed(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_scan_link_configs() {
        let tmp = TempDir::new().unwrap();
        let links_dir = tmp.path().join("links");
        tokio::fs::create_dir_all(&links_dir).await.unwrap();

        tokio::fs::write(
            links_dir.join("test-telegram.yaml"),
            r#"
spec_version: "1.0"
id: "test-telegram"
bridge: "telegram-native"
name: "Test Bot"
enabled: true
settings:
  token: "fake"
routing:
  agent: "main"
"#,
        )
        .await
        .unwrap();

        let configs = scan_link_configs(&links_dir).await.unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].id.as_str(), "test-telegram");
        assert!(configs[0].enabled);
    }

    #[tokio::test]
    async fn test_scan_link_configs_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let links_dir = tmp.path().join("links");
        tokio::fs::create_dir_all(&links_dir).await.unwrap();

        let configs = scan_link_configs(&links_dir).await.unwrap();
        assert!(configs.is_empty());
    }

    #[tokio::test]
    async fn test_scan_link_configs_missing_dir() {
        let tmp = TempDir::new().unwrap();
        let links_dir = tmp.path().join("links-nonexistent");

        let configs = scan_link_configs(&links_dir).await.unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn test_expand_env_vars() {
        std::env::set_var("ALEPH_TEST_TOKEN_XYZZY_12345", "secret-value");
        let settings = serde_json::json!({
            "token": "${env.ALEPH_TEST_TOKEN_XYZZY_12345}",
            "name": "no-expansion",
            "nested": {
                "key": "${env.ALEPH_TEST_TOKEN_XYZZY_12345}"
            },
            "array": ["${env.ALEPH_TEST_TOKEN_XYZZY_12345}", "plain"]
        });

        let expanded = expand_env_vars(&settings);

        assert_eq!(
            expanded.get("token").unwrap().as_str().unwrap(),
            "secret-value"
        );
        assert_eq!(
            expanded.get("name").unwrap().as_str().unwrap(),
            "no-expansion"
        );
        assert_eq!(
            expanded
                .get("nested")
                .unwrap()
                .get("key")
                .unwrap()
                .as_str()
                .unwrap(),
            "secret-value"
        );
        assert_eq!(
            expanded.get("array").unwrap()[0].as_str().unwrap(),
            "secret-value"
        );
        assert_eq!(expanded.get("array").unwrap()[1].as_str().unwrap(), "plain");

        std::env::remove_var("ALEPH_TEST_TOKEN_XYZZY_12345");
    }

    #[test]
    fn test_expand_env_vars_missing_var() {
        std::env::remove_var("ALEPH_TEST_DEFINITELY_NOT_SET_XYZZY");
        let settings = serde_json::json!({
            "token": "${env.ALEPH_TEST_DEFINITELY_NOT_SET_XYZZY}"
        });

        let expanded = expand_env_vars(&settings);

        assert_eq!(
            expanded.get("token").unwrap().as_str().unwrap(),
            "${env.ALEPH_TEST_DEFINITELY_NOT_SET_XYZZY}"
        );
    }

    #[test]
    fn test_expand_env_vars_non_string_passthrough() {
        let settings = serde_json::json!({
            "port": 8080,
            "enabled": true,
            "ratio": 1.5
        });
        let expanded = expand_env_vars(&settings);
        assert_eq!(expanded.get("port").unwrap().as_i64().unwrap(), 8080);
        assert!(expanded.get("enabled").unwrap().as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_link_manager_creation() {
        let tmp = TempDir::new().unwrap();
        let manager = LinkManager::new(tmp.path().to_path_buf());
        let ids = manager.list_channel_ids().await;
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn test_link_manager_start_no_links() {
        let tmp = TempDir::new().unwrap();
        let manager = LinkManager::new(tmp.path().to_path_buf());
        let result = manager.start().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_manager_error_display() {
        let err = LinkManagerError::FactoryNotFound("my-bridge".into());
        assert_eq!(err.to_string(), "Factory not found for bridge: my-bridge");

        let err = LinkManagerError::IoError("disk full".into());
        assert_eq!(err.to_string(), "IO error: disk full");
    }
}
