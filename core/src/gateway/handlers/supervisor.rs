//! Supervisor RPC handlers.
//!
//! Provides RPC methods for controlling external processes via PtySupervisor.
//!
//! ## Methods
//!
//! - `supervisor.spawn` - Spawn a supervised process
//! - `supervisor.write` - Write input to the process
//! - `supervisor.status` - Get process status

use serde::{Deserialize, Serialize};

/// Parameters for supervisor.spawn
#[derive(Debug, Deserialize)]
pub struct SupervisorSpawnParams {
    /// Working directory
    pub workspace: String,
    /// Command to execute (default: "claude")
    #[serde(default = "default_command")]
    pub command: String,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
}

fn default_command() -> String {
    "claude".to_string()
}

/// Result of supervisor.spawn
#[derive(Debug, Serialize)]
pub struct SupervisorSpawnResult {
    /// Unique session ID for this supervisor instance
    pub session_id: String,
    /// Whether spawn was successful
    pub success: bool,
}

/// Parameters for supervisor.write
#[derive(Debug, Deserialize)]
pub struct SupervisorWriteParams {
    /// Session ID from spawn
    pub session_id: String,
    /// Input to write
    pub input: String,
    /// Whether to append newline
    #[serde(default)]
    pub newline: bool,
}

/// Result of supervisor.write
#[derive(Debug, Serialize)]
pub struct SupervisorWriteResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parameters for supervisor.status
#[derive(Debug, Deserialize)]
pub struct SupervisorStatusParams {
    pub session_id: String,
}

/// Result of supervisor.status
#[derive(Debug, Serialize)]
pub struct SupervisorStatusResult {
    pub running: bool,
    pub session_id: String,
}

// TODO: Implement actual handlers in Milestone 2 when integrating with Gateway
