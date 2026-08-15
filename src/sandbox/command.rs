//! `SandboxCommand`, `SandboxOutput`, `SandboxError`.

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
    /// Set when the OS backend's own denial dialect appeared in `stderr` —
    /// see [`SandboxDenialHint`]. `None` on every path that runs without an
    /// OS driver (`WorktreeSandbox`, `NoopSandbox`) and under every backend
    /// that declares no dialect.
    #[serde(default)]
    pub denial_hint: Option<SandboxDenialHint>,
}

/// Evidence that the OS sandbox — rather than the command's own logic — may
/// have refused an effect: the running backend's denial dialect appeared in
/// stderr.
///
/// Recorded by `WorkspaceSandbox` right after the OS driver returns, because
/// that is the only place the *active* driver is in hand: every consumer
/// downstream holds a plain [`SandboxOutput`] and cannot tell which backend
/// produced the bytes.
///
/// This is a substring match, not a verdict. An application's own refusal (an
/// `ssh` publickey rejection, a `sudo` prompt) is byte-identical to a Landlock
/// one, so consumers must phrase it as a possibility and never as a cause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxDenialHint {
    /// `OsSandboxDriverTrait::platform()` of the driver that ran the command.
    pub platform: String,
    /// The entry from that backend's `denial_signatures()` that matched,
    /// verbatim — evidence a consumer can quote rather than paraphrase.
    pub signature: String,
}

impl SandboxDenialHint {
    /// Match `stderr` against one backend's denial dialect, case-insensitively.
    ///
    /// `signatures` must come from the driver that actually ran the command: a
    /// union across backends would claim a denial in a dialect the running
    /// backend cannot emit.
    #[must_use]
    pub fn detect(platform: &str, signatures: &[&'static str], stderr: &[u8]) -> Option<Self> {
        if signatures.is_empty() {
            return None;
        }
        let haystack = String::from_utf8_lossy(stderr).to_lowercase();
        signatures
            .iter()
            .find(|sig| haystack.contains(&sig.to_lowercase()))
            .map(|sig| Self {
                platform: platform.to_string(),
                signature: (*sig).to_string(),
            })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const SEATBELT: &[&str] = &["operation not permitted"];

    #[test]
    fn detect_quotes_the_signature_that_matched() {
        let hint = SandboxDenialHint::detect(
            "macos/seatbelt",
            SEATBELT,
            b"open: /etc/hosts: Operation not permitted\n",
        )
        .expect("seatbelt dialect present in stderr");
        assert_eq!(hint.platform, "macos/seatbelt");
        // Verbatim from the signature table, not from the stderr casing — the
        // consumer quotes a declared dialect entry, not whatever the OS typed.
        assert_eq!(hint.signature, "operation not permitted");
    }

    #[test]
    fn detect_is_silent_when_the_backend_declares_no_dialect() {
        // The defaulted `denial_signatures()` returns `&[]`; a driver that has
        // not declared a dialect must stay silent even on stderr that another
        // backend would have matched. This is the "never a union" invariant
        // enforced at the matcher rather than only at the call site.
        assert!(SandboxDenialHint::detect(
            "fake",
            &[],
            b"open: /etc/hosts: Operation not permitted\n"
        )
        .is_none());
    }

    #[test]
    fn detect_is_silent_on_a_foreign_dialect() {
        // bwrap's EROFS text under a seatbelt signature table: no match. A
        // union table would have reported a denial macOS never emits.
        assert!(SandboxDenialHint::detect(
            "macos/seatbelt",
            SEATBELT,
            b"touch: Read-only file system"
        )
        .is_none());
    }

    #[test]
    fn detect_survives_non_utf8_stderr() {
        let mut stderr = vec![0xff, 0xfe];
        stderr.extend_from_slice(b" bash: fork: Operation not permitted");
        assert!(SandboxDenialHint::detect("macos/seatbelt", SEATBELT, &stderr).is_some());
    }

    #[test]
    fn denial_hint_defaults_to_absent_on_older_records() {
        // `SandboxOutput` is persisted; a record written before this field
        // existed must still deserialize.
        let out: SandboxOutput = serde_json::from_str(
            r#"{"stdout":[],"stderr":[],"exit_code":0,"signal":null,"truncated":false,"duration_ms":1}"#,
        )
        .expect("legacy record deserializes");
        assert!(out.denial_hint.is_none());
    }
}
