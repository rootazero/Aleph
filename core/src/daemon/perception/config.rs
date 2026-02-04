use crate::daemon::{DaemonError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionConfig {
    pub enabled: bool,
    pub process: ProcessWatcherConfig,
    pub filesystem: FSWatcherConfig,
    pub time: TimeWatcherConfig,
    pub system: SystemWatcherConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessWatcherConfig {
    pub enabled: bool,
    pub poll_interval_secs: u64,
    pub watched_apps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FSWatcherConfig {
    pub enabled: bool,
    pub watched_paths: Vec<String>,
    pub ignore_patterns: Vec<String>,
    pub debounce_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeWatcherConfig {
    pub enabled: bool,
    pub heartbeat_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemWatcherConfig {
    pub enabled: bool,
    pub poll_interval_secs: u64,
    pub track_battery: bool,
    pub track_network: bool,
    pub idle_threshold_secs: u64,
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            process: ProcessWatcherConfig {
                enabled: true,
                poll_interval_secs: 5,
                watched_apps: vec![
                    "Code".to_string(),
                    "Google Chrome".to_string(),
                    "Zoom".to_string(),
                    "Slack".to_string(),
                    "Terminal".to_string(),
                ],
            },
            filesystem: FSWatcherConfig {
                enabled: true,
                watched_paths: vec!["~/Downloads".to_string(), "~/Desktop".to_string()],
                ignore_patterns: vec![
                    "**/.git/**".to_string(),
                    "**/node_modules/**".to_string(),
                    "**/target/**".to_string(),
                    "**/.DS_Store".to_string(),
                ],
                debounce_ms: 500,
            },
            time: TimeWatcherConfig {
                enabled: true,
                heartbeat_interval_secs: 30,
            },
            system: SystemWatcherConfig {
                enabled: true,
                poll_interval_secs: 60,
                track_battery: true,
                track_network: true,
                idle_threshold_secs: 300,
            },
        }
    }
}

impl PerceptionConfig {
    /// Load configuration from ~/.aleph/perception.toml
    pub fn load() -> Result<Self> {
        let path = dirs::home_dir()
            .ok_or_else(|| DaemonError::Config("HOME environment variable not set".into()))?
            .join(".aleph/perception.toml");

        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| DaemonError::Io(e))?;
            toml::from_str(&content)
                .map_err(|e| DaemonError::Config(format!("Invalid TOML: {}", e)))
        } else {
            Ok(Self::default())
        }
    }

    /// Expand tilde in filesystem paths
    pub fn expand_paths(&mut self) -> Result<()> {
        self.filesystem.watched_paths = self
            .filesystem
            .watched_paths
            .iter()
            .map(|p| {
                shellexpand::tilde(p).to_string()
            })
            .collect();
        Ok(())
    }
}
