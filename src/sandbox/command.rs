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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// True when either stream was shortened to fit `max_output_bytes`.
    /// Derived from `stdout_truncated_bytes` / `stderr_truncated_bytes` so
    /// older readers that only check this flag keep working.
    pub truncated: bool,
    /// Bytes dropped from stdout to satisfy `max_output_bytes`. Lets the
    /// model see *how much* it lost, not just the boolean.
    #[serde(default)]
    pub stdout_truncated_bytes: u64,
    /// Bytes dropped from stderr to satisfy `max_output_bytes`.
    #[serde(default)]
    pub stderr_truncated_bytes: u64,
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

    /// Wall-clock timeout fired. The child was killed and we drained the
    /// stdout/stderr pipes for up to 2s (codex-parity `IO_DRAIN_TIMEOUT_MS`)
    /// so the model still sees whatever the script printed before the kill —
    /// "started, did X, then sleep 9999" is much more useful than zero bytes.
    /// Either partial buffer may be empty when nothing was captured.
    #[error("timeout after {elapsed_ms}ms")]
    Timeout {
        elapsed_ms: u64,
        partial_stdout: Vec<u8>,
        partial_stderr: Vec<u8>,
    },

    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// The requested policy combination is not implementable on the current
    /// platform with the currently-wired sandbox mechanism. Callers should
    /// either downgrade the policy or wait for the relevant follow-up spec
    /// (Landlock+seccomp on Linux, WFP on Windows, proxy-based hostname
    /// filtering on macOS).
    #[error("sandbox policy unsupported on {platform}: {feature} — {reason}")]
    UnsupportedPolicy {
        platform: &'static str,
        feature: String,
        reason: String,
    },

    /// DNS pre-resolution for `NetworkPolicy::AllowHosts` failed. The hostname
    /// could not be turned into one or more IP literals to feed the OS
    /// sandbox's IP-allowlist mechanism. Fail-closed: the command is refused
    /// rather than running with an empty allowlist (which would deny all
    /// outbound traffic confusingly).
    #[error("dns resolution failed for host '{hostname}': {source}")]
    DnsResolutionFailed {
        hostname: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Other(String),
}
