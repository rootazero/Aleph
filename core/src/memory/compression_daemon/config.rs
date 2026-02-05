//! Configuration for compression daemon

use serde::{Deserialize, Serialize};

/// Configuration for the compression daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionDaemonConfig {
    /// Interval between compression checks (in seconds)
    pub check_interval_seconds: u64,

    /// Minimum idle time before running compression (in seconds)
    pub idle_threshold_seconds: u64,

    /// Whether the daemon is enabled
    pub enabled: bool,
}

impl Default for CompressionDaemonConfig {
    fn default() -> Self {
        Self {
            check_interval_seconds: 3600,  // 1 hour
            idle_threshold_seconds: 300,   // 5 minutes
            enabled: true,
        }
    }
}

impl CompressionDaemonConfig {
    /// Create a new config with custom values
    pub fn new(check_interval_seconds: u64, idle_threshold_seconds: u64) -> Self {
        Self {
            check_interval_seconds,
            idle_threshold_seconds,
            enabled: true,
        }
    }

    /// Disable the daemon
    pub fn disabled() -> Self {
        Self {
            check_interval_seconds: 3600,
            idle_threshold_seconds: 300,
            enabled: false,
        }
    }
}
