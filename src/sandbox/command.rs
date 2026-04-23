//! SandboxCommand, SandboxOutput, SandboxError.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::session::service::SessionId;

#[derive(Debug, Clone)]
pub struct SandboxCommand {
    pub session_id: SessionId,
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub stdin: Option<Vec<u8>>,
    pub cwd: Option<PathBuf>,
    pub capabilities: SandboxCapabilities,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub truncated: bool,
    pub duration_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("capability denied: {reason}")]
    CapabilityDenied { reason: String },

    #[error("seatbelt profile generation failed: {0}")]
    ProfileGeneration(String),

    #[error("io error: {0}")]
    Io(String),

    #[error("timeout after {elapsed_ms}ms")]
    Timeout { elapsed_ms: u64 },

    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    #[error("{0}")]
    Other(String),
}
