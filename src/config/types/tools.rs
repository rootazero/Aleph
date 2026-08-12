//! Tools configuration types
//!
//! Contains tool and MCP configuration:
//! - `ToolsConfig`: Legacy system tools configuration
//! - `McpConfig`: MCP server configuration
//! - `McpExternalServerConfig`: External MCP server settings
//! - `UnifiedToolsConfig`: New unified tools configuration
//! - `NativeToolsConfig`: Native tool settings container
//! - Individual tool configs (Fs, Git, Shell, `SystemInfo`, etc.)
//! - `McpServerConfig`: Unified MCP server settings
//! - `ToolServiceConfig`: Phase 2 `ToolService` runtime tunables (timeouts)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

use super::search::default_true;

// =============================================================================
// ToolServiceConfig (Phase 2)
// =============================================================================

/// Runtime configuration for the Phase 2 `ToolService` decorator chain.
///
/// Drives the `TimeoutLayer` default timeout, per-tool overrides, and the
/// Act-phase parallel-dispatch concurrency cap. Future tunables (rate caps)
/// will live here too.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolServiceConfig {
    /// Default timeout applied to every tool invocation (seconds).
    #[serde(default = "default_tool_service_timeout_seconds")]
    pub default_timeout_seconds: u64,

    /// Per-tool overrides keyed by tool name (seconds).
    #[serde(default)]
    pub per_tool_seconds: HashMap<String, u64>,

    /// Max number of concurrent-safe tool calls the harness Act phase
    /// dispatches at once within a single batch. `0` or `1` disables the
    /// parallel fast path (every batch runs serially); `>= 2` enables it.
    /// Unsafe tools (write/exec/send) always serialize regardless. Default
    /// `8` mirrors the production gateway's prior hardcoded value.
    #[serde(default = "default_parallel_tool_concurrency")]
    pub parallel_tool_concurrency: usize,
}

pub const fn default_tool_service_timeout_seconds() -> u64 {
    60
}

pub const fn default_parallel_tool_concurrency() -> usize {
    8
}

impl Default for ToolServiceConfig {
    fn default() -> Self {
        Self {
            default_timeout_seconds: default_tool_service_timeout_seconds(),
            per_tool_seconds: HashMap::new(),
            parallel_tool_concurrency: default_parallel_tool_concurrency(),
        }
    }
}

impl ToolServiceConfig {
    /// Resolve the default timeout as a `Duration`.
    ///
    /// `pub(crate)` — no external caller; the `ToolService` builder that used
    /// to consume this was removed in 2026-05-20 when the Phase 2 facade chain
    /// was deleted. Kept crate-scoped so sibling `config` modules can still
    /// reach the `Duration` without forcing a public API surface.
    pub(crate) const fn default_timeout(&self) -> Duration {
        Duration::from_secs(self.default_timeout_seconds)
    }

    /// Resolve the Act-phase parallel-dispatch cap as the harness's
    /// `Option<usize>` knob: `Some(n)` enables the fast path with concurrency
    /// `n`; the harness itself treats `Some(0..=1)` as disabled.
    pub const fn parallel_tool_concurrency_opt(&self) -> Option<usize> {
        Some(self.parallel_tool_concurrency)
    }

    /// Resolve the per-tool overrides as `Duration` values.
    ///
    /// `pub(crate)` — the only consumer is the `ToolService` constructor that
    /// builds the `TimeoutLayer` overrides; promoting this to the public API
    /// would advertise a `Duration` surface that callers should reach via the
    /// `per_tool_seconds` field directly.
    pub(crate) fn per_tool_durations(&self) -> HashMap<String, Duration> {
        self.per_tool_seconds
            .iter()
            .map(|(k, v)| (k.clone(), Duration::from_secs(*v)))
            .collect()
    }
}

// =============================================================================
// ToolsConfig (Legacy)
// =============================================================================

/// Configuration for System Tools (Tier 1: native Rust tools)
///
/// System Tools are always available and run as native Rust code.
/// They provide file system, git, shell, and system info capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolsConfig {
    /// Enable filesystem service
    #[serde(default = "default_true")]
    pub fs_enabled: bool,

    /// Allowed filesystem roots (paths the fs service can access)
    #[serde(default)]
    pub allowed_roots: Vec<String>,

    /// Enable git service
    #[serde(default = "default_true")]
    pub git_enabled: bool,

    /// Allowed git repositories (paths the git service can access)
    #[serde(default)]
    pub allowed_repos: Vec<String>,

    /// Enable shell service
    #[serde(default)]
    pub shell_enabled: bool,

    /// Allowed shell commands (whitelist for security)
    #[serde(default)]
    pub allowed_commands: Vec<String>,

    /// Shell command timeout in seconds
    #[serde(default = "default_shell_timeout")]
    pub shell_timeout_seconds: u64,

    /// Enable system info service
    #[serde(default = "default_true")]
    pub system_info_enabled: bool,

    /// Tools kept at FULL schema in every request. Every other tool has its
    /// `input_schema` collapsed to an open placeholder (name + description stay
    /// visible); the model calls `get_tool_schema("<name>")` to load full
    /// parameters on demand. `["*"]` or empty = disable collapsing (all tools
    /// full — byte-identical to pre-feature behavior).
    #[serde(default = "default_core_tools")]
    pub core: Vec<String>,

    /// When true, non-core tools also have their description truncated to the
    /// first sentence (extra token savings at some discoverability cost).
    #[serde(default)]
    pub truncate_tool_descriptions: bool,

    /// When true, tools from connected MCP servers are DEFERRED: dropped from
    /// the model's initial tool list and discoverable only via the
    /// `tool_search` meta-tool (which returns matches with their schemas).
    /// Default false = byte-identical tool surface (escape hatch). Independent
    /// of `core` / schema collapsing; a deferred tool is neither listed nor
    /// collapsed until `tool_search` surfaces it.
    #[serde(default)]
    pub defer_mcp_tools: bool,
}

pub const fn default_shell_timeout() -> u64 {
    30
}

/// Default "kept full" tool set — daily single-chat essentials plus the
/// on-demand schema loader. Non-core tools are schema-collapsed. See
/// `ProgressiveDisclosureRewriter`.
///
/// INVARIANT — do not drop `subagent` or `get_tool_schema` from this set.
/// They are the only two tools *in this list* that appear in a request's live
/// tool surface yet are ABSENT from the `get_tool_schema` schema snapshot:
/// `subagent` is attached out-of-band via
/// `ScopedToolService::with_subagent_tool`, and `get_tool_schema` is
/// registered into the loop registry AFTER the snapshot is captured (see
/// `run_loop/inner.rs`). If either were collapsed, the model would be told to
/// call `get_tool_schema(<it>)`, which would miss and mislead on a headline
/// capability. Keeping them core means they are never collapsed, which is
/// what closes the "collapsed-but-unsnapshotted" bug class. Guarded by
/// `snapshot_exempt_tools_must_stay_core`. `tool_search` is the third member
/// of that class (registered after the snapshot, only when a deferred tier
/// exists) — it cannot live in this static list, so `run_loop/inner.rs` adds
/// it to the request's effective core set at registration time instead.
///
/// `moa` is core so a user's "turn on MoA for this session" engages in one
/// step — the activation toggle must not hide behind a get_tool_schema
/// round-trip (R8 conversational management; round-2 W1). `session_set_mode`
/// is core for the same R8 one-step reason: every mode's prompt line points
/// at it, and its schema is a single string field.
pub fn default_core_tools() -> Vec<String> {
    [
        "ask_user",
        "subagent",
        "bash",
        "code_exec",
        "code_check",
        "file_read",
        "file_write",
        "file_edit",
        "file_ops",
        "search",
        "web_fetch",
        "memory_search",
        "remember",
        "skill_read",
        "skill_list",
        "scratchpad",
        "note_manage",
        "system",
        "get_tool_schema",
        "moa",
        "session_set_mode",
        // MCP native discovery+read mechanism — all four kept core (never
        // schema-collapsed) so discovery and read are each one step, mirroring
        // `skill_list`+`skill_read` above. There is deliberately NO
        // `<available MCP resources>` prompt index: the eager index layer was
        // removed 2026-07-17 because its single-`server:`-prefix ids did not
        // round-trip the two-strip read path. The model instead discovers
        // resources/prompts on demand via `mcp_list_resources`/`mcp_list_prompts`,
        // which return verbatim, round-trip-safe ids to feed straight into
        // `mcp_read_resource`/`mcp_get_prompt`. Keeping the *listing* half core
        // matters most — post-index-removal it is the only way to learn which
        // URIs/prompt-names exist, so collapsing it behind a `get_tool_schema`
        // round-trip is exactly the friction that pushes the model toward
        // `cat`-ing `file://` paths. No-op when no MCP server is connected.
        "mcp_read_resource",
        "mcp_get_prompt",
        "mcp_list_resources",
        "mcp_list_prompts",
        // Resource *templates* discovery (parameterized URIs). Kept core for
        // the same reason as `mcp_list_resources`: for a server exposing only
        // templates it is the sole way to learn what can be read.
        "mcp_list_resource_templates",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            fs_enabled: true,
            allowed_roots: Vec::new(), // Empty means current directory only
            git_enabled: true,
            allowed_repos: Vec::new(), // Empty means current directory only
            shell_enabled: false,      // Disabled by default for security
            allowed_commands: vec![
                "ls".to_string(),
                "cat".to_string(),
                "echo".to_string(),
                "pwd".to_string(),
            ],
            shell_timeout_seconds: default_shell_timeout(),
            system_info_enabled: true,
            core: default_core_tools(),
            truncate_tool_descriptions: false,
            defer_mcp_tools: false,
        }
    }
}

// =============================================================================
// McpConfig
// =============================================================================

/// MCP (Model Context Protocol) configuration
///
/// Controls external MCP server connections (Tier 2 Extensions)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpConfig {
    /// Enable MCP capability
    #[serde(default = "default_mcp_enabled")]
    pub enabled: bool,

    /// External servers configuration
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_servers: Vec<McpExternalServerConfig>,
}

pub const fn default_mcp_enabled() -> bool {
    true
}

pub const fn default_mcp_timeout() -> u64 {
    30
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: default_mcp_enabled(),
            external_servers: Vec::new(),
        }
    }
}

// =============================================================================
// McpExternalServerConfig
// =============================================================================

/// Configuration for external MCP servers
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpExternalServerConfig {
    /// Server name (unique identifier)
    pub name: String,

    /// Command to execute
    pub command: String,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Working directory
    #[serde(default)]
    pub cwd: Option<String>,

    /// Required runtime (node, python, bun, deno)
    #[serde(default)]
    pub requires_runtime: Option<String>,

    /// Request timeout in seconds
    #[serde(default = "default_mcp_timeout")]
    pub timeout_seconds: u64,
}

// =============================================================================
// UnifiedToolsConfig
// =============================================================================

/// Unified tools configuration (combines System Tools + MCP External Servers)
///
/// New TOML format:
/// ```toml
/// [unified_tools]
/// enabled = true
///
/// [unified_tools.native.fs]
/// enabled = true
/// allowed_roots = ["~", "/tmp"]
///
/// [unified_tools.native.git]
/// enabled = true
/// allowed_repos = ["~/projects"]
///
/// [unified_tools.native.shell]
/// enabled = false
/// timeout_seconds = 30
/// allowed_commands = ["ls", "cat"]
///
/// [unified_tools.native.system_info]
/// enabled = true
///
/// [unified_tools.mcp.github]
/// command = "node"
/// args = ["~/.mcp/github/index.js"]
/// requires_runtime = "node"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UnifiedToolsConfig {
    /// Master switch for all tools (both native and MCP)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Native system tools configuration
    #[serde(default)]
    pub native: NativeToolsConfig,

    /// MCP external servers configuration (keyed by server name)
    #[serde(default)]
    pub mcp: HashMap<String, McpServerConfig>,
}

impl Default for UnifiedToolsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            native: NativeToolsConfig::default(),
            mcp: HashMap::new(),
        }
    }
}

impl UnifiedToolsConfig {
    /// Create from legacy `ToolsConfig` and `McpConfig` (migration helper)
    pub fn from_legacy(tools: &ToolsConfig, mcp: &McpConfig) -> Self {
        let mut unified = Self {
            enabled: mcp.enabled,
            native: NativeToolsConfig {
                fs: Some(FsToolConfig {
                    enabled: tools.fs_enabled,
                    allowed_roots: tools.allowed_roots.clone(),
                }),
                git: Some(GitToolConfig {
                    enabled: tools.git_enabled,
                    allowed_repos: tools.allowed_repos.clone(),
                }),
                shell: Some(ShellToolConfig {
                    enabled: tools.shell_enabled,
                    timeout_seconds: tools.shell_timeout_seconds,
                    allowed_commands: tools.allowed_commands.clone(),
                }),
                system_info: Some(SystemInfoToolConfig {
                    enabled: tools.system_info_enabled,
                }),
                // New tools use defaults (not in legacy config)
                clipboard: None,
                screen_capture: None,
                search: None,
            },
            mcp: HashMap::new(),
        };

        // Convert external servers to new format
        for server in &mcp.external_servers {
            unified.mcp.insert(
                server.name.clone(),
                McpServerConfig {
                    command: server.command.clone(),
                    args: server.args.clone(),
                    env: server.env.clone(),
                    cwd: server.cwd.clone(),
                    requires_runtime: server.requires_runtime.clone(),
                    timeout_seconds: server.timeout_seconds,
                    enabled: true,
                },
            );
        }

        unified
    }

    /// Check if filesystem service is enabled.
    ///
    /// `pub(crate)` — no production caller; the master-and-per-tool gate is
    /// read by sibling `config` modules via the `native.fs` field directly.
    pub(crate) fn is_fs_enabled(&self) -> bool {
        self.enabled && self.native.fs.as_ref().is_none_or(|c| c.enabled)
    }

    /// Check if git service is enabled.
    ///
    /// `pub(crate)` — same rationale as [`is_fs_enabled`](Self::is_fs_enabled).
    pub(crate) fn is_git_enabled(&self) -> bool {
        self.enabled && self.native.git.as_ref().is_none_or(|c| c.enabled)
    }

    /// Check if shell service is enabled.
    ///
    /// `pub(crate)` — no production caller; native shell gating is read via
    /// `native.shell` directly.
    pub(crate) fn is_shell_enabled(&self) -> bool {
        self.enabled && self.native.shell.as_ref().is_some_and(|c| c.enabled)
    }

    /// Check if system info service is enabled.
    ///
    /// `pub(crate)` — no production caller; native system_info gating is read
    /// via `native.system_info` directly.
    pub(crate) fn is_system_info_enabled(&self) -> bool {
        self.enabled && self.native.system_info.as_ref().is_none_or(|c| c.enabled)
    }

    /// Get filesystem allowed roots.
    ///
    /// `pub(crate)` — no external caller; left at crate scope so the helper
    /// stays useful to other `config` modules without advertising a public
    /// accessor that drifts from the underlying `NativeToolsConfig` shape.
    pub(crate) fn fs_allowed_roots(&self) -> Vec<String> {
        self.native
            .fs
            .as_ref()
            .map_or(Vec::new(), |c| c.allowed_roots.clone())
    }

    /// Get git allowed repos.
    ///
    /// `pub(crate)` for the same reason as [`fs_allowed_roots`](Self::fs_allowed_roots).
    pub(crate) fn git_allowed_repos(&self) -> Vec<String> {
        self.native
            .git
            .as_ref()
            .map_or(Vec::new(), |c| c.allowed_repos.clone())
    }

    /// Get shell configuration.
    ///
    /// `pub(crate)` — no production caller; native shell is read via
    /// `native.shell` directly.
    pub(crate) fn shell_config(&self) -> ShellToolConfig {
        self.native.shell.clone().unwrap_or_default()
    }

    /// Check if clipboard service is enabled
    pub fn is_clipboard_enabled(&self) -> bool {
        self.enabled && self.native.clipboard.as_ref().is_none_or(|c| c.enabled)
    }

    /// Check if screen capture service is enabled.
    ///
    /// `pub(crate)` — there is no production consumer of this accessor in the
    /// current code base; the unified tools surface is consulted per-tool
    /// through the `native.screen_capture` field directly. Kept crate-scoped
    /// so the master-switch / per-tool enable ladder stays available to
    /// sibling `config` modules without inflating the public API.
    pub(crate) fn is_screen_capture_enabled(&self) -> bool {
        self.enabled
            && self
                .native
                .screen_capture
                .as_ref()
                .is_none_or(|c| c.enabled)
    }

    /// Get screen capture configuration.
    ///
    /// `pub(crate)` — same rationale as [`is_screen_capture_enabled`](Self::is_screen_capture_enabled).
    pub(crate) fn screen_capture_config(&self) -> ScreenCaptureToolConfig {
        self.native.screen_capture.clone().unwrap_or_default()
    }

    /// Check if search tool service is enabled.
    ///
    /// `pub(crate)` — the search tool is enabled through
    /// `native.search` directly; this accessor is a convenience for sibling
    /// `config` modules and tests.
    pub(crate) fn is_search_tool_enabled(&self) -> bool {
        self.enabled && self.native.search.as_ref().is_none_or(|c| c.enabled)
    }

    /// Get search tool configuration.
    ///
    /// `pub(crate)` — same rationale as [`is_search_tool_enabled`](Self::is_search_tool_enabled).
    pub(crate) fn search_tool_config(&self) -> SearchToolConfig {
        self.native.search.clone().unwrap_or_default()
    }

    /// Get all enabled MCP servers.
    ///
    /// `pub(crate)` — there is no external caller; the MCP server roster is
    /// read through the `mcp` field directly by the MCP manager. Kept
    /// crate-scoped so the master-switch / per-server enable ladder remains
    /// available to sibling `config` modules.
    pub(crate) fn enabled_mcp_servers(&self) -> Vec<(&String, &McpServerConfig)> {
        self.mcp
            .iter()
            .filter(|(_, config)| config.enabled)
            .collect()
    }
}

// =============================================================================
// NativeToolsConfig
// =============================================================================

/// Configuration for native system tools (Tier 1)
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct NativeToolsConfig {
    /// Filesystem service configuration
    #[serde(default)]
    pub fs: Option<FsToolConfig>,

    /// Git service configuration
    #[serde(default)]
    pub git: Option<GitToolConfig>,

    /// Shell service configuration
    #[serde(default)]
    pub shell: Option<ShellToolConfig>,

    /// System info service configuration
    #[serde(default)]
    pub system_info: Option<SystemInfoToolConfig>,

    /// Clipboard read service configuration
    #[serde(default)]
    pub clipboard: Option<ClipboardToolConfig>,

    /// Screen capture service configuration
    #[serde(default)]
    pub screen_capture: Option<ScreenCaptureToolConfig>,

    /// Search tool service configuration
    #[serde(default)]
    pub search: Option<SearchToolConfig>,
}

// =============================================================================
// Individual Tool Configs
// =============================================================================

/// Filesystem tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FsToolConfig {
    /// Enable filesystem service
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Allowed filesystem roots (paths the fs service can access)
    /// Empty means current directory only
    #[serde(default)]
    pub allowed_roots: Vec<String>,
}

impl Default for FsToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_roots: Vec::new(),
        }
    }
}

/// Git tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitToolConfig {
    /// Enable git service
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Allowed git repositories (paths the git service can access)
    /// Empty means current directory only
    #[serde(default)]
    pub allowed_repos: Vec<String>,
}

impl Default for GitToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_repos: Vec::new(),
        }
    }
}

/// Shell tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShellToolConfig {
    /// Enable shell service (disabled by default for security)
    #[serde(default)]
    pub enabled: bool,

    /// Shell command timeout in seconds
    #[serde(default = "default_shell_timeout")]
    pub timeout_seconds: u64,

    /// Allowed shell commands (whitelist for security)
    #[serde(default = "default_shell_commands")]
    pub allowed_commands: Vec<String>,
}

pub fn default_shell_commands() -> Vec<String> {
    vec![
        "ls".to_string(),
        "cat".to_string(),
        "echo".to_string(),
        "pwd".to_string(),
    ]
}

impl Default for ShellToolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_seconds: default_shell_timeout(),
            allowed_commands: default_shell_commands(),
        }
    }
}

/// System info tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SystemInfoToolConfig {
    /// Enable system info service
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for SystemInfoToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Clipboard tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClipboardToolConfig {
    /// Enable clipboard read service
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for ClipboardToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Screen capture tool configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenCaptureToolConfig {
    /// Enable screen capture service
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum image dimension (width or height)
    #[serde(default = "default_max_dimension")]
    pub max_dimension: u32,

    /// JPEG quality for captured images (0-100)
    #[serde(default = "default_jpeg_quality")]
    pub jpeg_quality: u8,
}

pub const fn default_max_dimension() -> u32 {
    1920
}

pub const fn default_jpeg_quality() -> u8 {
    85
}

impl Default for ScreenCaptureToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_dimension: default_max_dimension(),
            jpeg_quality: default_jpeg_quality(),
        }
    }
}

/// Search tool configuration (wraps existing `SearchRegistry` as tool)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchToolConfig {
    /// Enable search tool
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Default maximum number of search results
    #[serde(default = "default_search_tool_max_results")]
    pub default_max_results: usize,

    /// Default search timeout in seconds
    #[serde(default = "default_search_tool_timeout_seconds")]
    pub default_timeout_seconds: u64,
}

pub const fn default_search_tool_max_results() -> usize {
    5
}

pub const fn default_search_tool_timeout_seconds() -> u64 {
    10
}

impl Default for SearchToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_max_results: default_search_tool_max_results(),
            default_timeout_seconds: default_search_tool_timeout_seconds(),
        }
    }
}

// =============================================================================
// McpServerConfig
// =============================================================================

/// MCP external server configuration (unified format)
///
/// This is similar to `McpExternalServerConfig` but with a cleaner structure
/// where the server name is the TOML table key instead of a field.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpServerConfig {
    /// Command to execute
    pub command: String,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Working directory
    #[serde(default)]
    pub cwd: Option<String>,

    /// Required runtime (node, python, bun, deno)
    #[serde(default)]
    pub requires_runtime: Option<String>,

    /// Request timeout in seconds
    #[serde(default = "default_mcp_timeout")]
    pub timeout_seconds: u64,

    /// Enable this server (allows disabling without removing config)
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[cfg(test)]
mod core_tools_tests {
    use super::*;

    #[test]
    fn defer_mcp_tools_defaults_off() {
        let cfg = ToolsConfig::default();
        assert!(
            !cfg.defer_mcp_tools,
            "deferred tier must default OFF (escape-hatch)"
        );
    }

    #[test]
    fn defer_mcp_tools_deserializes() {
        let cfg: ToolsConfig = toml::from_str("defer_mcp_tools = true").unwrap();
        assert!(cfg.defer_mcp_tools);
    }

    #[test]
    fn default_core_contains_essentials() {
        let c = ToolsConfig::default();
        assert!(c.core.iter().any(|t| t == "bash"));
        assert!(c.core.iter().any(|t| t == "get_tool_schema"));
        assert!(c.core.iter().any(|t| t == "subagent"));
        assert!(c.core.iter().any(|t| t == "moa"));
        assert!(c.core.iter().any(|t| t == "mcp_read_resource"));
        assert!(c.core.iter().any(|t| t == "mcp_get_prompt"));
        // Discovery half kept core too — symmetric with skill_list/skill_read
        // (post index-removal, listing is the only way to learn MCP URIs/prompts).
        assert!(c.core.iter().any(|t| t == "mcp_list_resources"));
        assert!(c.core.iter().any(|t| t == "mcp_list_prompts"));
        assert!(c.core.iter().any(|t| t == "mcp_list_resource_templates"));
        // R8 one-step mode switch — every mode prompt line points at it.
        assert!(c.core.iter().any(|t| t == "session_set_mode"));
        assert_eq!(c.core.len(), 26);
        assert!(!c.truncate_tool_descriptions);
    }

    /// Regression guard for the progressive-disclosure snapshot invariant.
    ///
    /// `subagent` and `get_tool_schema` are the only tools that appear in a
    /// request's live tool surface yet are absent from the `get_tool_schema`
    /// schema snapshot (subagent is attached out-of-band; get_tool_schema is
    /// registered after the snapshot is taken — see `run_loop/inner.rs`). If
    /// either is dropped from `core` it gets collapsed, and calling
    /// `get_tool_schema(<it>)` then misses — a misleading failure on a headline
    /// capability. This test pins them to `core` so a future edit to the list
    /// cannot silently reopen that bug. See `default_core_tools`.
    #[test]
    fn snapshot_exempt_tools_must_stay_core() {
        let core = default_core_tools();
        assert!(
            core.iter().any(|t| t == "subagent"),
            "subagent must stay core: it is attached out-of-band and absent from \
             the get_tool_schema snapshot, so collapsing it makes get_tool_schema miss"
        );
        assert!(
            core.iter().any(|t| t == "get_tool_schema"),
            "get_tool_schema must stay core: it is registered after the snapshot is \
             taken, so it cannot resolve its own collapsed schema"
        );
    }

    #[test]
    fn core_roundtrips_and_supports_wildcard_sentinel() {
        let toml = r#"core = ["*"]
truncate_tool_descriptions = true"#;
        let c: ToolsConfig = toml::from_str(toml).unwrap();
        assert_eq!(c.core, vec!["*".to_string()]);
        assert!(c.truncate_tool_descriptions);
    }
}
