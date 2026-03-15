//! Type definitions for code execution
//!
//! Error types, result types, and runtime information.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Error types for code execution
#[derive(Debug, Clone, thiserror::Error)]
pub enum CodeExecError {
    #[error("Code execution is disabled")]
    Disabled,

    #[error("Runtime not found: {0}")]
    RuntimeNotFound(String),

    #[error("Runtime not allowed: {0}")]
    RuntimeNotAllowed(String),

    #[error("Execution timeout after {0} seconds")]
    Timeout(u64),

    #[error("Command blocked: {reason}")]
    Blocked { reason: String },

    #[error("Sandbox error: {0}")]
    SandboxError(String),

    #[error("Execution failed with exit code {code}: {message}")]
    ExecutionFailed { code: i32, message: String },

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Path not allowed: {0}")]
    PathNotAllowed(PathBuf),
}

impl From<std::io::Error> for CodeExecError {
    fn from(err: std::io::Error) -> Self {
        CodeExecError::IoError(err.to_string())
    }
}

/// Result of code execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecResult {
    /// Exit code (0 = success)
    pub exit_code: i32,

    /// Captured stdout
    pub stdout: String,

    /// Captured stderr
    pub stderr: String,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Whether stdout was truncated
    pub stdout_truncated: bool,

    /// Whether stderr was truncated
    pub stderr_truncated: bool,

    /// Runtime used
    pub runtime: String,
}

/// Information about an available runtime
#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    /// Runtime name (e.g., "python", "node")
    pub name: String,

    /// Path to the runtime executable
    pub path: PathBuf,

    /// Runtime version (if detected)
    pub version: Option<String>,

    /// Whether the runtime is available
    pub available: bool,
}

impl RuntimeInfo {
    /// Detect a runtime by name
    pub async fn detect(runtime: &str) -> Self {
        let cmd = match std::env::consts::OS {
            "windows" => "where",
            _ => "which",
        };

        let output = Command::new(cmd).arg(runtime).output().await;

        match output {
            Ok(out) if out.status.success() => {
                let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let path = PathBuf::from(&path_str);

                // Try to get version
                let version = Self::get_version(runtime, &path).await;

                Self {
                    name: runtime.to_string(),
                    path,
                    version,
                    available: true,
                }
            }
            _ => Self {
                name: runtime.to_string(),
                path: PathBuf::new(),
                version: None,
                available: false,
            },
        }
    }

    async fn get_version(runtime: &str, path: &Path) -> Option<String> {
        let version_flag = match runtime {
            "python" | "python3" => "--version",
            "node" => "--version",
            "bash" | "zsh" => "--version",
            _ => return None,
        };

        let output = Command::new(path).arg(version_flag).output().await.ok()?;

        if output.status.success() {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if version.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if !stderr.is_empty() {
                    return Some(stderr);
                }
            }
            Some(version)
        } else {
            None
        }
    }
}
