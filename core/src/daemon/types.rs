use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Daemon configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Unix Domain Socket path for IPC
    pub socket_path: String,

    /// Binary path for daemon executable
    pub binary_path: PathBuf,

    /// Log directory
    pub log_dir: PathBuf,

    /// Nice value (process priority, 10 = low priority)
    pub nice_value: i32,

    /// Soft memory limit in bytes (512MB)
    pub soft_mem_limit: u64,

    /// Hard memory limit in bytes (1GB)
    pub hard_mem_limit: u64,
}

fn default_socket_path() -> String {
    dirs::home_dir()
        .map(|h| h.join(".aleph/daemon.sock").to_string_lossy().to_string())
        .unwrap_or_else(|| "~/.aleph/daemon.sock".to_string())
}

fn default_binary_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".aleph/bin/aleph-daemon"))
        .unwrap_or_else(|| PathBuf::from("~/.aleph/bin/aleph-daemon"))
}

fn default_log_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".aleph/logs"))
        .unwrap_or_else(|| PathBuf::from("~/.aleph/logs"))
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            binary_path: default_binary_path(),
            log_dir: default_log_dir(),
            nice_value: 10,
            soft_mem_limit: 512 * 1024 * 1024,  // 512MB
            hard_mem_limit: 1024 * 1024 * 1024, // 1GB
        }
    }
}

/// Daemon runtime status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonStatus {
    Running,
    Stopped,
    Unknown,
}

/// Service installation status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServiceStatus {
    Installed,
    NotInstalled,
    Unknown,
}
