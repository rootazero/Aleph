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

use crate::sandbox::command_policy::CommandPolicyConfigSchema;
use crate::sandbox::rate_limit::{SandboxRateLimitConfig, ToolCategory, WindowConfig};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WindowsSandboxConfig {
    /// SP-3a: when `true`, the sandbox-init-windows subcommand attempts
    /// to launch the target via a restricted token at Low IL. Soft-
    /// degrades to plain `CreateProcessW` + `JobObject` if the host lacks
    /// `SE_INCREASE_QUOTA` (`ERROR_PRIVILEGE_NOT_HELD`).
    #[serde(default = "default_windows_use_restricted_token")]
    pub use_restricted_token: bool,

    /// SP-3a: when `true`, refuse to spawn (rather than soft-degrade)
    /// on `ERROR_PRIVILEGE_NOT_HELD`. Default `false`.
    #[serde(default)]
    pub require_restricted_token: bool,

    /// SP-6: try `AppContainer` first (strongest Windows sandbox primitive).
    /// Soft-degrades to SP-3a restricted-token on failure. Default `true`.
    #[serde(default = "default_windows_use_app_container")]
    pub use_app_container: bool,

    /// SP-6: when `true`, refuse to spawn if `AppContainer` setup fails.
    /// Default `false` → soft-degrade.
    #[serde(default)]
    pub require_app_container: bool,

    #[serde(default = "default_windows_use_job_object")]
    pub use_job_object: bool,

    #[serde(default = "default_windows_max_active_processes")]
    pub max_active_processes: u32,
}

const fn default_windows_use_app_container() -> bool {
    true
}

const fn default_windows_use_restricted_token() -> bool {
    true
}

const fn default_windows_use_job_object() -> bool {
    true
}

const fn default_windows_max_active_processes() -> u32 {
    8
}

impl Default for WindowsSandboxConfig {
    fn default() -> Self {
        Self {
            use_restricted_token: default_windows_use_restricted_token(),
            require_restricted_token: false,
            use_app_container: default_windows_use_app_container(),
            require_app_container: false,
            use_job_object: default_windows_use_job_object(),
            max_active_processes: default_windows_max_active_processes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
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
    #[serde(default = "default_rate_limit_read")]
    pub read: WindowConfigSchema,
    #[serde(default = "default_rate_limit_write")]
    pub write: WindowConfigSchema,
    #[serde(default = "default_rate_limit_dangerous")]
    pub dangerous: WindowConfigSchema,
    #[serde(default = "default_rate_limit_admin")]
    pub admin: WindowConfigSchema,
}

const fn default_rate_limit_enabled() -> bool {
    true
}
const fn default_max_requests() -> u32 {
    60
}
const fn default_window_secs() -> u64 {
    60
}
const fn default_burst_allow() -> u32 {
    20
}

const fn default_rate_limit_read() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 60,
        window_secs: 60,
        burst_allow: 20,
    }
}
const fn default_rate_limit_write() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 30,
        window_secs: 60,
        burst_allow: 10,
    }
}
const fn default_rate_limit_dangerous() -> WindowConfigSchema {
    WindowConfigSchema {
        max_requests: 10,
        window_secs: 60,
        burst_allow: 5,
    }
}
const fn default_rate_limit_admin() -> WindowConfigSchema {
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
            read: default_rate_limit_read(),
            write: default_rate_limit_write(),
            dangerous: default_rate_limit_dangerous(),
            admin: default_rate_limit_admin(),
        }
    }
}

/// `[sandbox.resource_governor]` — load-aware admission gate for heavy
/// (Dangerous-class) execution. See [`crate::sandbox::resource_governor`].
///
/// **Disabled by default** so boot behaviour is byte-identical to today;
/// opt in by setting `enabled = true`. When enabled, the default thresholds
/// gate purely on a free-memory floor (CPU gate off) — the robust signal on
/// a headless box.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResourceGovernorConfigSchema {
    #[serde(default = "default_governor_enabled")]
    pub enabled: bool,
    /// Refuse a heavy task while free system memory is below this many MiB.
    /// `None` disables the memory gate.
    #[serde(default = "default_governor_min_memory_mb")]
    pub min_available_memory_mb: Option<u64>,
    /// Refuse a heavy task while global CPU usage exceeds this percentage.
    /// `None` (default) disables the CPU gate — the first `sysinfo` CPU
    /// sample is unreliable, so memory is the safer default signal.
    #[serde(default = "default_governor_max_cpu_percent")]
    pub max_cpu_percent: Option<f32>,
    /// Maximum seconds to wait for load to subside before rejecting the task
    /// (the bounded-wait improvement over the unbounded shell `sleep` loop).
    #[serde(default = "default_governor_max_wait_secs")]
    pub max_wait_secs: u64,
    /// Delay in milliseconds between load re-samples while waiting.
    #[serde(default = "default_governor_backoff_ms")]
    pub backoff_ms: u64,
}

const fn default_governor_enabled() -> bool {
    false
}
const fn default_governor_min_memory_mb() -> Option<u64> {
    Some(512)
}
const fn default_governor_max_cpu_percent() -> Option<f32> {
    None
}
const fn default_governor_max_wait_secs() -> u64 {
    30
}
const fn default_governor_backoff_ms() -> u64 {
    500
}

impl Default for ResourceGovernorConfigSchema {
    fn default() -> Self {
        Self {
            enabled: default_governor_enabled(),
            min_available_memory_mb: default_governor_min_memory_mb(),
            max_cpu_percent: default_governor_max_cpu_percent(),
            max_wait_secs: default_governor_max_wait_secs(),
            backoff_ms: default_governor_backoff_ms(),
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

    /// SP-2: when `true`, sandbox-init exits non-zero if the kernel
    /// lacks landlock ABI ≥ 1. Default `false` → soft-degrade.
    #[serde(default)]
    pub require_landlock: bool,

    /// SP-5: enable cgroup v2 containment. Default `true`; soft-degrade
    /// if the kernel/host does not expose cgroup v2 with delegation.
    #[serde(default = "default_linux_cgroup_enabled")]
    pub cgroup_enabled: bool,

    /// SP-5: when `true`, the sandbox refuses to spawn if cgroup v2
    /// setup fails. Default `false` (degrade to `RLIMIT_AS`).
    #[serde(default)]
    pub require_cgroups: bool,

    /// SP-5: `cpu.max` quota as a percentage of one CPU. `None` →
    /// unlimited CPU (kernel default). `100` = one full core; `200` =
    /// two cores.
    #[serde(default)]
    pub cpu_quota_percent: Option<u32>,

    /// SP-5: `pids.max` ceiling. Default 200 — defends against fork
    /// bombs beyond what bwrap's active-process limit provides.
    #[serde(default = "default_linux_max_pids")]
    pub max_pids: Option<u32>,
}

const fn default_linux_cgroup_enabled() -> bool {
    true
}

const fn default_linux_max_pids() -> Option<u32> {
    Some(200)
}

const fn default_linux_mount_proc() -> bool {
    true
}

const fn default_linux_no_new_privs() -> bool {
    true
}

const fn default_linux_include_platform_defaults() -> bool {
    true
}

impl Default for LinuxSandboxConfig {
    fn default() -> Self {
        Self {
            mount_proc: default_linux_mount_proc(),
            no_new_privs: default_linux_no_new_privs(),
            include_platform_defaults: default_linux_include_platform_defaults(),
            require_landlock: false,
            cgroup_enabled: default_linux_cgroup_enabled(),
            require_cgroups: false,
            cpu_quota_percent: None,
            max_pids: default_linux_max_pids(),
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

    /// Git-style glob patterns whose matches are denied **read** (and
    /// `unlink`) access inside every sandboxed command, even when the path
    /// lies within an otherwise-readable workspace root. A defence-in-depth
    /// security floor for secrets such as `**/.env`, `**/*.pem`,
    /// `**/.ssh/**`. Empty by default → no extra denies (boot unchanged).
    /// Currently enforced by the macOS seatbelt driver; other platforms
    /// ignore it until landlock/WFP glob enforcement lands.
    #[serde(default)]
    pub deny_read_globs: Vec<String>,

    /// Absolute paths to Unix-domain sockets a sandboxed command may
    /// connect to / bind, even under a restricted (or `None`) network
    /// policy. Local IPC over an `AF_UNIX` socket is otherwise swept up by
    /// the `(deny network*)` floor, so tools that talk to `ssh-agent`
    /// (`SSH_AUTH_SOCK`), a PostgreSQL/MySQL `.sock`, `gpg-agent`, or a
    /// language-server socket fail with no TCP/IP intent. Entries grant
    /// access to the socket path *and any socket beneath it* (subpath).
    /// Empty by default → no `AF_UNIX` allowance (boot unchanged, default
    /// deny preserved). macOS-seatbelt only today; mirrors codex's
    /// `network.allow_unix_sockets`.
    #[serde(default)]
    pub allow_unix_sockets: Vec<PathBuf>,

    /// Escape hatch mirroring codex's `dangerously_allow_all_unix_sockets`:
    /// permit *any* `AF_UNIX` socket bind/connect under a restricted network
    /// policy. Dangerous — a local socket can reach privileged daemons
    /// (e.g. `docker.sock` → root). Prefer the `allow_unix_sockets`
    /// allowlist. Default `false`.
    #[serde(default)]
    pub dangerously_allow_all_unix_sockets: bool,

    /// Permit sandboxed commands to bind and listen on loopback sockets even
    /// under a restricted (or `None`) network policy. Dev/test workloads that
    /// spin up a local server (`python -m http.server`, a Vite/webpack dev
    /// server, a test harness binding `127.0.0.1:0`, a TCP language server)
    /// otherwise fail because the restricted-network floor denies
    /// `network-bind`. Grants loopback binding only — no route to the public
    /// internet is opened. macOS-seatbelt only today; mirrors codex's
    /// `network.allow_local_binding`. Default `false` (boot unchanged).
    #[serde(default)]
    pub allow_local_binding: bool,

    #[serde(default)]
    pub linux: LinuxSandboxConfig,

    #[serde(default)]
    pub windows: WindowsSandboxConfig,

    #[serde(default)]
    pub rate_limit: SandboxRateLimitConfigSchema,

    /// Load-aware admission gate for heavy execution
    /// (`[sandbox.resource_governor]`). Disabled by default; see
    /// [`crate::sandbox::resource_governor`].
    #[serde(default)]
    pub resource_governor: ResourceGovernorConfigSchema,

    /// Content-level command hard-filter (`[sandbox.command_policy]`).
    /// Defence-in-depth in front of the OS sandbox; see
    /// [`crate::sandbox::command_policy`].
    #[serde(default)]
    pub command_policy: CommandPolicyConfigSchema,
}

fn default_workspace_root() -> PathBuf {
    dirs::home_dir().map_or_else(
        || {
            // Fall back to a known absolute path to avoid creating workspaces
            // in an arbitrary working directory.
            PathBuf::from("/tmp/.aleph/workspaces")
        },
        |home| home.join(".aleph").join("workspaces"),
    )
}

const fn default_enabled() -> bool {
    true
}

const fn default_timeout_seconds() -> u64 {
    60
}

const fn default_max_output_bytes() -> usize {
    1024 * 1024
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            workspace_root: default_workspace_root(),
            enabled: default_enabled(),
            default_timeout_seconds: default_timeout_seconds(),
            max_output_bytes: default_max_output_bytes(),
            deny_read_globs: Vec::new(),
            allow_unix_sockets: Vec::new(),
            dangerously_allow_all_unix_sockets: false,
            allow_local_binding: false,
            linux: LinuxSandboxConfig::default(),
            windows: WindowsSandboxConfig::default(),
            rate_limit: SandboxRateLimitConfigSchema {
                enabled: default_rate_limit_enabled(),
                read: default_rate_limit_read(),
                write: default_rate_limit_write(),
                dangerous: default_rate_limit_dangerous(),
                admin: default_rate_limit_admin(),
            },
            resource_governor: ResourceGovernorConfigSchema::default(),
            command_policy: CommandPolicyConfigSchema::default(),
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
    fn windows_config_default_has_job_object_settings() {
        // BUG-2 regression: these two fields must exist with sane
        // defaults and be threaded into the driver (see
        // platforms::mod::create_platform_driver_with_config).
        let cfg = WindowsSandboxConfig::default();
        assert!(cfg.use_job_object);
        assert_eq!(cfg.max_active_processes, 8);
    }

    #[test]
    fn windows_config_job_object_fields_deserialize_from_toml() {
        let toml = r#"
            [windows]
            use_job_object = false
            max_active_processes = 4
        "#;
        let cfg: SandboxConfig = toml::from_str(toml).expect("parses");
        assert!(!cfg.windows.use_job_object);
        assert_eq!(cfg.windows.max_active_processes, 4);
    }

    #[test]
    fn deny_read_globs_default_empty_and_parses() {
        // Back-compat: absent key → empty list (boot unchanged).
        let cfg = SandboxConfig::default();
        assert!(cfg.deny_read_globs.is_empty());

        let toml = r#"
            deny_read_globs = ["**/.env", "**/*.pem"]
        "#;
        let cfg: SandboxConfig = toml::from_str(toml).expect("parses");
        assert_eq!(cfg.deny_read_globs, vec!["**/.env", "**/*.pem"]);
    }

    #[test]
    fn allow_unix_sockets_default_empty_and_parses() {
        // Back-compat: absent keys → empty list + false (boot unchanged).
        let cfg = SandboxConfig::default();
        assert!(cfg.allow_unix_sockets.is_empty());
        assert!(!cfg.dangerously_allow_all_unix_sockets);

        let toml = r#"
            allow_unix_sockets = ["/tmp/agent.sock", "/var/run/db"]
            dangerously_allow_all_unix_sockets = true
        "#;
        let cfg: SandboxConfig = toml::from_str(toml).expect("parses");
        assert_eq!(
            cfg.allow_unix_sockets,
            vec![
                PathBuf::from("/tmp/agent.sock"),
                PathBuf::from("/var/run/db")
            ]
        );
        assert!(cfg.dangerously_allow_all_unix_sockets);
    }

    #[test]
    fn allow_local_binding_default_false_and_parses() {
        // Back-compat: absent key → false (default-deny floor preserved).
        let cfg = SandboxConfig::default();
        assert!(!cfg.allow_local_binding);

        let toml = r#"
            allow_local_binding = true
        "#;
        let cfg: SandboxConfig = toml::from_str(toml).expect("parses");
        assert!(cfg.allow_local_binding);
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
