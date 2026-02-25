//! LinkManager — central orchestrator for the social connectivity plugin system.
//!
//! The `LinkManager` ties together bridge definitions, link instance configs,
//! the `BridgeSupervisor`, and channel lifecycle management. On startup it:
//!
//! 1. Scans `~/.aleph/bridges/` for external bridge plugin definitions.
//! 2. Scans `~/.aleph/links/` for link instance configurations.
//! 3. Creates and starts each enabled link.
//!
//! # Standalone helpers
//!
//! [`scan_link_configs`], [`scan_bridge_definitions`], and [`expand_env_vars`]
//! are public, stateless functions usable independently of `LinkManager`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use super::types::LinkConfig;
use crate::gateway::bridge::{
    BridgeDefinition, BridgeId, BridgeRuntime, BridgeSupervisor, BridgedChannel,
    ManagedProcessConfig,
};
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
            Ok(content) => match serde_yaml::from_str::<LinkConfig>(&content) {
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

/// Scan a directory for bridge plugin definitions.
///
/// Each bridge plugin lives in its own sub-directory containing a `bridge.yaml`
/// manifest file.  Directories without a `bridge.yaml` are silently skipped.
/// The function returns `Ok([])` if `dir` does not exist.
pub async fn scan_bridge_definitions(
    dir: &Path,
) -> Result<Vec<BridgeDefinition>, LinkManagerError> {
    let mut defs = Vec::new();

    if !tokio::fs::try_exists(dir).await.unwrap_or(false) {
        return Ok(defs);
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

        // Bridges live in sub-directories.
        if !path.is_dir() {
            continue;
        }

        let bridge_yaml = path.join("bridge.yaml");
        if !tokio::fs::try_exists(&bridge_yaml).await.unwrap_or(false) {
            continue;
        }

        match tokio::fs::read_to_string(&bridge_yaml).await {
            Ok(content) => match serde_yaml::from_str::<BridgeDefinition>(&content) {
                Ok(def) => {
                    info!(path = %bridge_yaml.display(), id = %def.id, "Loaded bridge definition");
                    defs.push(def);
                }
                Err(e) => {
                    warn!(
                        path = %bridge_yaml.display(),
                        error = %e,
                        "Failed to parse bridge.yaml — skipping"
                    );
                }
            },
            Err(e) => {
                warn!(
                    path = %bridge_yaml.display(),
                    error = %e,
                    "Failed to read bridge.yaml — skipping"
                );
            }
        }
    }

    Ok(defs)
}

/// Recursively expand `${env.VAR_NAME}` references in a JSON value.
///
/// * String values matching the pattern are replaced with the environment
///   variable's value.  If the variable is not set, the original string is
///   kept and a warning is logged.
/// * Object values are expanded key-by-key.
/// * Array values are expanded element-by-element.
/// * All other value types are returned unchanged.
pub fn expand_env_vars(settings: &serde_json::Value) -> serde_json::Value {
    match settings {
        serde_json::Value::String(s) => {
            // Match "${env.VAR_NAME}" pattern.
            if let Some(rest) = s.strip_prefix("${env.") {
                if let Some(var_name) = rest.strip_suffix('}') {
                    match std::env::var(var_name) {
                        Ok(val) => return serde_json::Value::String(val),
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

/// Manages the full lifecycle of all messaging links:
///
/// - **Builtin** bridges (Telegram, Discord, iMessage) created via [`ChannelFactory`].
/// - **External** bridges (arbitrary executables) managed by [`BridgeSupervisor`].
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
    /// Registered bridge type definitions (both builtin and external).
    bridge_registry: RwLock<HashMap<BridgeId, BridgeDefinition>>,

    /// Builtin channel factories keyed by bridge id.
    builtin_factories: RwLock<HashMap<BridgeId, Arc<dyn ChannelFactory>>>,

    /// Active builtin channel instances keyed by channel id.
    ///
    /// Wrapped in `Arc<Mutex<>>` because [`Channel::start`] / [`Channel::stop`]
    /// take `&mut self`.
    builtin_channels: RwLock<ChannelMap>,

    /// Active bridged channel instances keyed by channel id.
    ///
    /// `BridgedChannel` does not implement the `Channel` trait (see module
    /// docs) so it is stored separately.
    bridged_channels: RwLock<HashMap<ChannelId, Arc<Mutex<BridgedChannel>>>>,

    /// External bridge process supervisor.
    bridge_supervisor: Arc<BridgeSupervisor>,

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
    ///   bridges/   — external bridge plugin directories
    ///   links/     — link instance config files (*.yaml)
    ///   run/       — runtime files (Unix sockets, PIDs)
    /// ```
    pub fn new(base_dir: PathBuf) -> Self {
        let run_dir = base_dir.join("run");
        Self {
            bridge_registry: RwLock::new(HashMap::new()),
            builtin_factories: RwLock::new(HashMap::new()),
            builtin_channels: RwLock::new(HashMap::new()),
            bridged_channels: RwLock::new(HashMap::new()),
            bridge_supervisor: Arc::new(BridgeSupervisor::new(run_dir)),
            base_dir,
        }
    }

    /// Register a builtin bridge type (e.g. Telegram, Discord).
    ///
    /// Builtin bridges have a `BridgeRuntime::Builtin` definition paired with
    /// a [`ChannelFactory`] that creates channel instances from settings JSON.
    pub async fn register_builtin(
        &self,
        definition: BridgeDefinition,
        factory: Arc<dyn ChannelFactory>,
    ) {
        let id = definition.id.clone();
        self.bridge_registry
            .write()
            .await
            .insert(id.clone(), definition);
        self.builtin_factories.write().await.insert(id.clone(), factory);
        info!(bridge_id = %id, "Registered builtin bridge");
    }

    /// Full startup sequence.
    ///
    /// 1. Scans `{base_dir}/bridges/` for external bridge definitions.
    /// 2. Scans `{base_dir}/links/` for link instance configs.
    /// 3. Creates and starts each enabled link.
    ///
    /// Individual link failures are logged but do not abort the overall
    /// startup — the manager starts as many links as possible.
    pub async fn start(&self) -> Result<(), LinkManagerError> {
        // 1. Scan external bridge definitions.
        let bridges_dir = self.base_dir.join("bridges");
        let external_defs = scan_bridge_definitions(&bridges_dir).await?;
        for def in external_defs {
            let id = def.id.clone();
            self.bridge_registry.write().await.insert(id.clone(), def);
            info!(bridge_id = %id, "Registered external bridge");
        }

        // 2. Scan link instance configs.
        let links_dir = self.base_dir.join("links");
        let link_configs = scan_link_configs(&links_dir).await?;

        // 3. Instantiate and start each enabled link.
        for link in link_configs {
            if !link.enabled {
                info!(link_id = %link.id, "Skipping disabled link");
                continue;
            }

            if let Err(e) = self.create_and_start_link(&link).await {
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

    /// Stop all active links and bridge processes.
    pub async fn stop(&self) {
        // Stop all bridged channels first.
        let bridged_ids: Vec<ChannelId> = {
            let guard = self.bridged_channels.read().await;
            guard.keys().cloned().collect()
        };
        for id in bridged_ids {
            let channel = {
                let guard = self.bridged_channels.read().await;
                guard.get(&id).cloned()
            };
            if let Some(ch) = channel {
                let mut guard = ch.lock().await;
                if let Err(e) = guard.stop().await {
                    warn!(channel_id = %id, error = %e, "Error stopping bridged channel");
                }
            }
        }

        // Stop all builtin channels.
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

        // Stop all bridge processes.
        self.bridge_supervisor.stop_all().await;

        info!("LinkManager stopped all links");
    }

    /// List all active channel ids (both builtin and bridged).
    pub async fn list_channel_ids(&self) -> Vec<ChannelId> {
        let mut ids = Vec::new();
        ids.extend(self.builtin_channels.read().await.keys().cloned());
        ids.extend(self.bridged_channels.read().await.keys().cloned());
        ids
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    async fn create_and_start_link(&self, link: &LinkConfig) -> Result<(), LinkManagerError> {
        // Look up the bridge definition.  Hold the lock only for the lookup.
        let bridge = {
            let registry = self.bridge_registry.read().await;
            registry
                .get(&link.bridge)
                .cloned()
                .ok_or_else(|| LinkManagerError::BridgeNotFound(link.bridge.to_string()))?
        };

        // Expand ${env.VAR} references in settings before passing to the bridge.
        let expanded_settings = expand_env_vars(&link.settings);

        match &bridge.runtime {
            BridgeRuntime::Builtin => {
                self.start_builtin_link(link, expanded_settings).await
            }
            BridgeRuntime::Process { .. } => {
                self.start_process_link(link, &bridge.runtime).await
            }
        }
    }

    /// Create and start a builtin (Rust-compiled) bridge channel.
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

    /// Spawn an external bridge process and start the bridged channel.
    async fn start_process_link(
        &self,
        link: &LinkConfig,
        runtime: &BridgeRuntime,
    ) -> Result<(), LinkManagerError> {
        let process_config = ManagedProcessConfig::from_runtime(runtime).ok_or_else(|| {
            LinkManagerError::InvalidRuntime("Expected process runtime".into())
        })?;

        // Spawn the bridge process and get its transport + handshake response.
        let result = self
            .bridge_supervisor
            .spawn(&link.id, process_config)
            .await
            .map_err(|e| LinkManagerError::BridgeSpawnFailed(e.to_string()))?;

        // Build and start the BridgedChannel.
        let mut bridged = BridgedChannel::new(
            link.id.as_str(),
            &link.name,
            link.bridge.as_str(),
        );
        bridged.set_transport(result.transport);

        bridged
            .start()
            .await
            .map_err(|e| LinkManagerError::ChannelStartFailed(e.to_string()))?;

        let channel_id = ChannelId::new(link.id.as_str());
        let wrapped = Arc::new(Mutex::new(bridged));
        self.bridged_channels
            .write()
            .await
            .insert(channel_id.clone(), wrapped);

        info!(link_id = %link.id, channel_id = %channel_id, "Started external bridge link");
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

    #[error("Bridge not found: {0}")]
    BridgeNotFound(String),

    #[error("Factory not found for bridge: {0}")]
    FactoryNotFound(String),

    #[error("Invalid runtime: {0}")]
    InvalidRuntime(String),

    #[error("Channel creation failed: {0}")]
    ChannelCreationFailed(String),

    #[error("Channel start failed: {0}")]
    ChannelStartFailed(String),

    #[error("Bridge spawn failed: {0}")]
    BridgeSpawnFailed(String),

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
        std::fs::create_dir_all(&links_dir).unwrap();

        // Write a test link.yaml.
        std::fs::write(
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
        std::fs::create_dir_all(&links_dir).unwrap();

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

    #[tokio::test]
    async fn test_scan_bridge_definitions() {
        let tmp = TempDir::new().unwrap();
        let bridges_dir = tmp.path().join("bridges");
        let whatsapp_dir = bridges_dir.join("whatsapp-go");
        std::fs::create_dir_all(&whatsapp_dir).unwrap();

        std::fs::write(
            whatsapp_dir.join("bridge.yaml"),
            r#"
spec_version: "1.0"
id: "whatsapp-go"
name: "WhatsApp"
version: "1.0.0"
runtime:
  type: process
  executable: "./bin/whatsapp-bridge"
  transport: unix-socket
capabilities:
  messaging:
    - send_text
"#,
        )
        .unwrap();

        let defs = scan_bridge_definitions(&bridges_dir).await.unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id.as_str(), "whatsapp-go");
    }

    #[tokio::test]
    async fn test_scan_bridge_definitions_missing_dir() {
        let tmp = TempDir::new().unwrap();
        let bridges_dir = tmp.path().join("bridges-nonexistent");

        let defs = scan_bridge_definitions(&bridges_dir).await.unwrap();
        assert!(defs.is_empty());
    }

    #[tokio::test]
    async fn test_scan_bridge_skips_non_directories() {
        let tmp = TempDir::new().unwrap();
        let bridges_dir = tmp.path().join("bridges");
        std::fs::create_dir_all(&bridges_dir).unwrap();

        // A plain file at the bridges_dir level — should be skipped.
        std::fs::write(bridges_dir.join("README.md"), "# Bridges").unwrap();

        let defs = scan_bridge_definitions(&bridges_dir).await.unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn test_expand_env_vars() {
        // Use a unique var name to avoid collision with real env vars.
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

        // Unexpanded string is kept as-is.
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
        // Should succeed even if bridges/ and links/ don't exist.
        let result = manager.start().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_link_manager_error_display() {
        let err = LinkManagerError::BridgeNotFound("my-bridge".into());
        assert_eq!(err.to_string(), "Bridge not found: my-bridge");

        let err = LinkManagerError::IoError("disk full".into());
        assert_eq!(err.to_string(), "IO error: disk full");

        let err = LinkManagerError::BridgeSpawnFailed("timeout".into());
        assert_eq!(err.to_string(), "Bridge spawn failed: timeout");
    }
}
