//! Hot Configuration Reload
//!
//! Watches configuration files for changes and broadcasts reload events.
//! Supports atomic validation and rollback on failure.

use std::path::{Path, PathBuf};
use crate::sync_primitives::Arc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use notify_debouncer_full::new_debouncer;
use serde::Deserialize;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};

use super::config::{ConfigError, GatewayConfig};

/// Controls how configuration sections are reloaded at runtime.
///
/// - `Off`: Never hot-reload; changes require a full restart.
/// - `Hot`: Always hot-reload every section immediately.
/// - `Restart`: Never hot-reload; all changes require restart.
/// - `Hybrid`: Hot-reload only safe sections (ui, channels, skills, workspace, cron);
///   other sections require restart.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReloadMode {
    Off,
    #[default]
    Hot,
    Restart,
    Hybrid,
}

impl ReloadMode {
    /// Whether the given configuration section should be hot-reloaded
    /// under this mode.
    pub fn should_hot_reload(&self, section: &str) -> bool {
        match self {
            Self::Off | Self::Restart => false,
            Self::Hot => true,
            Self::Hybrid => matches!(section, "ui" | "channels" | "skills" | "workspace" | "cron"),
        }
    }
}

/// Event emitted when configuration changes
#[derive(Debug, Clone)]
pub enum ConfigEvent {
    /// Configuration was successfully reloaded
    Reloaded(Arc<GatewayConfig>),
    /// Configuration validation failed
    ValidationFailed(String),
    /// File system error occurred
    FileError(String),
}

/// Configuration for the ConfigWatcher
#[derive(Debug, Clone)]
pub struct ConfigWatcherConfig {
    /// Path to the configuration file
    pub config_path: PathBuf,
    /// Debounce duration for file change events (default: 500ms)
    pub debounce_duration: Duration,
    /// Maximum number of pending events in broadcast channel (default: 16)
    pub channel_capacity: usize,
}

impl Default for ConfigWatcherConfig {
    fn default() -> Self {
        Self {
            config_path: dirs::home_dir()
                .map(|h| h.join(".aleph/config.toml"))
                .unwrap_or_else(|| PathBuf::from("config.toml")),
            debounce_duration: Duration::from_millis(500),
            channel_capacity: 16,
        }
    }
}

/// Watches configuration file for changes and broadcasts events
#[derive(Debug)]
pub struct ConfigWatcher {
    config: ConfigWatcherConfig,
    current_config: Arc<RwLock<Arc<GatewayConfig>>>,
    event_tx: broadcast::Sender<ConfigEvent>,
    _event_rx: broadcast::Receiver<ConfigEvent>,
}

impl ConfigWatcher {
    /// Create a new ConfigWatcher
    pub fn new(config: ConfigWatcherConfig) -> Result<Self, ConfigWatcherError> {
        // Validate config path exists
        if !config.config_path.exists() {
            return Err(ConfigWatcherError::ConfigNotFound(
                config.config_path.display().to_string(),
            ));
        }

        // Load initial configuration
        let initial_config = GatewayConfig::load(&config.config_path)?;

        let (event_tx, event_rx) = broadcast::channel(config.channel_capacity);

        Ok(Self {
            config,
            current_config: Arc::new(RwLock::new(Arc::new(initial_config))),
            event_tx,
            _event_rx: event_rx,
        })
    }

    /// Create a ConfigWatcher with default path
    pub fn with_default_path() -> Result<Self, ConfigWatcherError> {
        Self::new(ConfigWatcherConfig::default())
    }

    /// Subscribe to configuration events
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigEvent> {
        self.event_tx.subscribe()
    }

    /// Get the current configuration
    pub async fn current_config(&self) -> Arc<GatewayConfig> {
        self.current_config.read().await.clone()
    }

    /// Get the config path being watched
    pub fn config_path(&self) -> &Path {
        &self.config.config_path
    }

    /// Manually trigger a config reload
    pub async fn reload(&self) -> Result<Arc<GatewayConfig>, ConfigWatcherError> {
        self.reload_config().await
    }

    /// Validate configuration file without applying
    pub fn validate(&self) -> Result<GatewayConfig, ConfigWatcherError> {
        GatewayConfig::load(&self.config.config_path).map_err(ConfigWatcherError::from)
    }

    /// Start watching for configuration changes
    ///
    /// This spawns a background task that monitors the config file.
    /// Returns a JoinHandle that can be used to stop the watcher.
    pub fn start_watching(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let watcher = self.clone();

        tokio::spawn(async move {
            if let Err(e) = watcher.watch_loop().await {
                error!("Config watcher error: {}", e);
            }
        })
    }

    /// Internal watch loop
    async fn watch_loop(&self) -> Result<(), ConfigWatcherError> {
        let config_path = self.config.config_path.clone();
        let debounce_duration = self.config.debounce_duration;

        // Create channel for file events
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);

        // Create debounced file watcher
        let tx_clone = tx.clone();
        let mut debouncer = new_debouncer(
            debounce_duration,
            None,
            move |result: Result<Vec<notify_debouncer_full::DebouncedEvent>, Vec<notify::Error>>| {
                match result {
                    Ok(events) => {
                        for event in events {
                            if matches!(
                                event.event.kind,
                                EventKind::Modify(_) | EventKind::Create(_)
                            ) {
                                let _ = tx_clone.blocking_send(FileWatchEvent::Changed);
                            }
                        }
                    }
                    Err(errors) => {
                        for e in errors {
                            let _ = tx_clone.blocking_send(FileWatchEvent::Error(e.to_string()));
                        }
                    }
                }
            },
        )
        .map_err(|e| ConfigWatcherError::WatcherError(e.to_string()))?;

        // Watch the parent directory (to catch file recreation)
        let watch_path = config_path
            .parent()
            .ok_or_else(|| ConfigWatcherError::WatcherError("Invalid config path".to_string()))?;

        debouncer
            .watcher()
            .watch(watch_path, RecursiveMode::NonRecursive)
            .map_err(|e| ConfigWatcherError::WatcherError(e.to_string()))?;

        info!("Config watcher started for: {}", config_path.display());

        // Process file events
        while let Some(event) = rx.recv().await {
            match event {
                FileWatchEvent::Changed => {
                    debug!("Config file change detected");
                    match self.reload_config().await {
                        Ok(new_config) => {
                            info!("Configuration reloaded successfully");
                            let _ = self.event_tx.send(ConfigEvent::Reloaded(new_config));
                        }
                        Err(e) => {
                            warn!("Configuration reload failed: {}", e);
                            let _ = self
                                .event_tx
                                .send(ConfigEvent::ValidationFailed(e.to_string()));
                        }
                    }
                }
                FileWatchEvent::Error(e) => {
                    error!("File watcher error: {}", e);
                    let _ = self.event_tx.send(ConfigEvent::FileError(e));
                }
            }
        }

        Ok(())
    }

    /// Reload configuration from file
    async fn reload_config(&self) -> Result<Arc<GatewayConfig>, ConfigWatcherError> {
        // Load and validate new configuration
        let new_config = GatewayConfig::load(&self.config.config_path)?;

        // Update current config atomically
        let new_config = Arc::new(new_config);
        {
            let mut current = self.current_config.write().await;
            *current = new_config.clone();
        }

        Ok(new_config)
    }
}

/// Internal file watch event
enum FileWatchEvent {
    Changed,
    Error(String),
}

/// Errors that can occur during config watching
#[derive(Debug, thiserror::Error)]
pub enum ConfigWatcherError {
    #[error("Configuration file not found: {0}")]
    ConfigNotFound(String),

    #[error("Configuration error: {0}")]
    ConfigError(#[from] ConfigError),

    #[error("File watcher error: {0}")]
    WatcherError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_config_watcher_creation() {
        // Create a temp config file
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[gateway]
port = 18790

[agents.main]
model = "claude-sonnet-4-5"
"#
        )
        .unwrap();

        let config = ConfigWatcherConfig {
            config_path: temp_file.path().to_path_buf(),
            debounce_duration: Duration::from_millis(100),
            channel_capacity: 8,
        };

        let watcher = ConfigWatcher::new(config);
        assert!(watcher.is_ok());
    }

    #[test]
    fn test_config_watcher_not_found() {
        let config = ConfigWatcherConfig {
            config_path: PathBuf::from("/nonexistent/config.toml"),
            ..Default::default()
        };

        let result = ConfigWatcher::new(config);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigWatcherError::ConfigNotFound(_)));
    }

    #[tokio::test]
    async fn test_config_reload() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[gateway]
port = 18790

[agents.main]
model = "claude-sonnet-4-5"
"#
        )
        .unwrap();

        let config = ConfigWatcherConfig {
            config_path: temp_file.path().to_path_buf(),
            debounce_duration: Duration::from_millis(100),
            channel_capacity: 8,
        };

        let watcher = ConfigWatcher::new(config).unwrap();

        // Get initial config
        let initial = watcher.current_config().await;
        assert_eq!(initial.gateway.port, 18790);

        // Modify config file - truncate and rewrite
        use std::io::{Seek, SeekFrom};
        let file = temp_file.as_file_mut();
        file.set_len(0).unwrap(); // Truncate
        file.seek(SeekFrom::Start(0)).unwrap(); // Seek to beginning
        writeln!(
            temp_file,
            r#"
[gateway]
port = 9999

[agents.main]
model = "claude-opus-4-5"
"#
        )
        .unwrap();

        // Manually reload
        let new_config = watcher.reload().await.unwrap();
        assert_eq!(new_config.gateway.port, 9999);
    }

    #[test]
    fn test_reload_mode_default() {
        assert_eq!(ReloadMode::default(), ReloadMode::Hot);
    }

    #[test]
    fn test_reload_mode_hot_reload_decisions() {
        let sections = ["ui", "channels", "skills", "workspace", "cron", "agents", "gateway", "providers"];

        // Off → always false
        for s in &sections {
            assert!(!ReloadMode::Off.should_hot_reload(s), "Off should never hot-reload '{s}'");
        }

        // Hot → always true
        for s in &sections {
            assert!(ReloadMode::Hot.should_hot_reload(s), "Hot should always hot-reload '{s}'");
        }

        // Restart → always false
        for s in &sections {
            assert!(!ReloadMode::Restart.should_hot_reload(s), "Restart should never hot-reload '{s}'");
        }

        // Hybrid → true only for safe sections
        let hybrid_safe = ["ui", "channels", "skills", "workspace", "cron"];
        let hybrid_unsafe = ["agents", "gateway", "providers", "security", "memory"];

        for s in &hybrid_safe {
            assert!(ReloadMode::Hybrid.should_hot_reload(s), "Hybrid should hot-reload '{s}'");
        }
        for s in &hybrid_unsafe {
            assert!(!ReloadMode::Hybrid.should_hot_reload(s), "Hybrid should NOT hot-reload '{s}'");
        }
    }

    #[test]
    fn test_config_validation() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(
            temp_file,
            r#"
[gateway]
port = 18790

[agents.main]
model = "claude-sonnet-4-5"
"#
        )
        .unwrap();

        let config = ConfigWatcherConfig {
            config_path: temp_file.path().to_path_buf(),
            debounce_duration: Duration::from_millis(100),
            channel_capacity: 8,
        };

        let watcher = ConfigWatcher::new(config).unwrap();
        let result = watcher.validate();
        assert!(result.is_ok());
    }
}
