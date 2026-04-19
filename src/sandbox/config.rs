//! `SandboxConfig` — boot-time tuning for the sandbox subsystem.
//!
//! Defaults reflect the Phase 3 spec: a `WorkspaceSandbox` rooted at
//! `~/.aleph/workspaces/` with a 60s per-command timeout and 1 MB
//! combined stdout+stderr budget. Tests / CI can disable the OS
//! sandbox by setting `enabled = false`, in which case `build_sandbox`
//! returns a `NoopSandbox` stub.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Runtime configuration for the sandbox subsystem.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SandboxConfig {
    /// Root for per-session workspaces. Defaults to `~/.aleph/workspaces`.
    #[serde(default = "default_workspace_root")]
    pub workspace_root: PathBuf,

    /// When `false`, the app boots with `NoopSandbox` (tests / CI).
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Default per-command timeout in seconds.
    #[serde(default = "default_timeout_seconds")]
    pub default_timeout_seconds: u64,

    /// Maximum combined stdout + stderr bytes retained per command.
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

fn default_workspace_root() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        home.join(".aleph").join("workspaces")
    } else {
        PathBuf::from("./.aleph/workspaces")
    }
}

fn default_enabled() -> bool {
    true
}

fn default_timeout_seconds() -> u64 {
    60
}

fn default_max_output_bytes() -> usize {
    1024 * 1024
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            workspace_root: default_workspace_root(),
            enabled: default_enabled(),
            default_timeout_seconds: default_timeout_seconds(),
            max_output_bytes: default_max_output_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_enabled_true_and_sensible_timeout() {
        let cfg = SandboxConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.default_timeout_seconds, 60);
        assert_eq!(cfg.max_output_bytes, 1024 * 1024);
        assert!(
            cfg.workspace_root.ends_with("workspaces"),
            "workspace_root should end in 'workspaces', got {:?}",
            cfg.workspace_root
        );
    }

    #[test]
    fn deserialises_from_empty_toml_table() {
        // TOML back-compat: `[sandbox]` empty block must yield defaults.
        let cfg: SandboxConfig = toml::from_str("").expect("empty table parses");
        assert!(cfg.enabled);
        assert_eq!(cfg.default_timeout_seconds, 60);
    }

    #[test]
    fn deserialises_with_enabled_false() {
        let cfg: SandboxConfig = toml::from_str("enabled = false").expect("enabled = false parses");
        assert!(!cfg.enabled);
        // Other fields still default.
        assert_eq!(cfg.default_timeout_seconds, 60);
    }
}
