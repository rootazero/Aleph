//! `SandboxConfig` — boot-time tuning for the sandbox subsystem.
//!
//! Defaults reflect the Phase 3 spec: a `WorkspaceSandbox` rooted at
//! `~/.aleph/workspaces/` with a 60s per-command timeout and 1 MB
//! combined stdout+stderr budget. Tests / CI can disable the OS
//! sandbox by setting `enabled = false`, in which case `build_sandbox`
//! returns a `NoopSandbox` stub.

use std::collections::HashMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::sandbox::rate_limit::{SandboxRateLimitConfig, ToolCategory, WindowConfig};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WindowsSandboxConfig {
    #[serde(default = "default_windows_use_restricted_token")]
    pub use_restricted_token: bool,

    #[serde(default = "default_windows_use_job_object")]
    pub use_job_object: bool,

    #[serde(default = "default_windows_max_active_processes")]
    pub max_active_processes: u32,
}

fn default_windows_use_restricted_token() -> bool {
    true
}

fn default_windows_use_job_object() -> bool {
    true
}

fn default_windows_max_active_processes() -> u32 {
    8
}

impl Default for WindowsSandboxConfig {
    fn default() -> Self {
        Self {
            use_restricted_token: default_windows_use_restricted_token(),
            use_job_object: default_windows_use_job_object(),
            max_active_processes: default_windows_max_active_processes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WindowConfigSchema {
    #[serde(default = "default_max_requests")]
    pub max_requests: u32,
    #[serde(default = "default_window_secs")]
    pub window_secs: u64,
    #[serde(default = "default_burst_allow")]
    pub burst_allow: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SandboxRateLimitConfigSchema {
    #[serde(default = "default_rate_limit_enabled")]
    pub enabled: bool,
    #[serde(default = "default_rate_limit_exempt_loopback")]
    pub exempt_loopback: bool,
    #[serde(default = "default_rate_limit_read")]
    pub read: WindowConfigSchema,
    #[serde(default = "default_rate_limit_write")]
    pub write: WindowConfigSchema,
    #[serde(default = "default_rate_limit_dangerous")]
    pub dangerous: WindowConfigSchema,
    #[serde(default = "default_rate_limit_admin")]
    pub admin: WindowConfigSchema,
}

fn default_rate_limit_enabled() -> bool {
    true
}
fn default_rate_limit_exempt_loopback() -> bool {
    true
}
fn default_max_requests() -> u32 {
    60
}
fn default_window_secs() -> u64 {
    60
}
fn default_burst_allow() -> u32 {
    20
}

fn default_rate_limit_read() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 60,
        window_secs: 60,
        burst_allow: 20,
    }
}
fn default_rate_limit_write() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 30,
        window_secs: 60,
        burst_allow: 10,
    }
}
fn default_rate_limit_dangerous() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 10,
        window_secs: 60,
        burst_allow: 5,
    }
}
fn default_rate_limit_admin() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 5,
        window_secs: 60,
        burst_allow: 2,
    }
}

impl Default for SandboxRateLimitConfigSchema {
    fn default() -> Self {
        Self {
            enabled: default_rate_limit_enabled(),
            exempt_loopback: default_rate_limit_exempt_loopback(),
            read: default_rate_limit_read(),
            write: default_rate_limit_write(),
            dangerous: default_rate_limit_dangerous(),
            admin: default_rate_limit_admin(),
        }
    }
}

impl From<SandboxRateLimitConfigSchema> for SandboxRateLimitConfig {
    fn from(schema: SandboxRateLimitConfigSchema) -> Self {
        let mut per_category = HashMap::new();
        per_category.insert(
            ToolCategory::Read,
            WindowConfig {
                max_requests: schema.read.max_requests,
                window_secs: schema.read.window_secs,
                burst_allow: schema.read.burst_allow,
            },
        );
        per_category.insert(
            ToolCategory::Write,
            WindowConfig {
                max_requests: schema.write.max_requests,
                window_secs: schema.write.window_secs,
                burst_allow: schema.write.burst_allow,
            },
        );
        per_category.insert(
            ToolCategory::Dangerous,
            WindowConfig {
                max_requests: schema.dangerous.max_requests,
                window_secs: schema.dangerous.window_secs,
                burst_allow: schema.dangerous.burst_allow,
            },
        );
        per_category.insert(
            ToolCategory::Admin,
            WindowConfig {
                max_requests: schema.admin.max_requests,
                window_secs: schema.admin.window_secs,
                burst_allow: schema.admin.burst_allow,
            },
        );
        Self {
            enabled: schema.enabled,
            exempt_loopback: schema.exempt_loopback,
            per_category,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LinuxSandboxConfig {
    #[serde(default = "default_linux_mount_proc")]
    pub mount_proc: bool,

    #[serde(default = "default_linux_no_new_privs")]
    pub no_new_privs: bool,

    #[serde(default = "default_linux_include_platform_defaults")]
    pub include_platform_defaults: bool,
}

fn default_linux_mount_proc() -> bool {
    true
}

fn default_linux_no_new_privs() -> bool {
    true
}

fn default_linux_include_platform_defaults() -> bool {
    true
}

impl Default for LinuxSandboxConfig {
    fn default() -> Self {
        Self {
            mount_proc: default_linux_mount_proc(),
            no_new_privs: default_linux_no_new_privs(),
            include_platform_defaults: default_linux_include_platform_defaults(),
        }
    }
}

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

    #[serde(default)]
    pub linux: LinuxSandboxConfig,

    #[serde(default)]
    pub windows: WindowsSandboxConfig,

    #[serde(default)]
    pub rate_limit: SandboxRateLimitConfigSchema,
}

fn default_workspace_root() -> PathBuf {
    dirs::home_dir()
        .map(|home| home.join(".aleph").join("workspaces"))
        .unwrap_or_else(|| {
            // Fall back to a known absolute path to avoid creating workspaces
            // in an arbitrary working directory.
            PathBuf::from("/tmp/.aleph/workspaces")
        })
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
            linux: LinuxSandboxConfig::default(),
            windows: WindowsSandboxConfig::default(),
            rate_limit: SandboxRateLimitConfigSchema {
                enabled: default_rate_limit_enabled(),
                exempt_loopback: default_rate_limit_exempt_loopback(),
                read: default_rate_limit_read(),
                write: default_rate_limit_write(),
                dangerous: default_rate_limit_dangerous(),
                admin: default_rate_limit_admin(),
            },
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
        assert_eq!(cfg.default_timeout_seconds, 60);
    }

    #[test]
    fn rate_limit_config_deserializes_from_toml() {
        let toml = r#"
            [rate_limit]
            enabled = true
            read = { max_requests = 100, window_secs = 30, burst_allow = 50 }
        "#;
        let cfg: SandboxConfig = toml::from_str(toml).expect("parses");
        assert!(cfg.rate_limit.enabled);
        assert_eq!(cfg.rate_limit.read.max_requests, 100);
        assert_eq!(cfg.rate_limit.read.window_secs, 30);
        assert_eq!(cfg.rate_limit.read.burst_allow, 50);
    }
}
