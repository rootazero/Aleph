//! Tools configuration types
//!
//! Contains tool and MCP configuration:
//! - ToolsConfig: Legacy system tools configuration
//! - McpConfig: MCP server configuration
//! - McpExternalServerConfig: External MCP server settings
//! - UnifiedToolsConfig: New unified tools configuration
//! - NativeToolsConfig: Native tool settings container
//! - Individual tool configs (Fs, Git, Shell, SystemInfo, etc.)
//! - McpServerConfig: Unified MCP server settings

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::search::default_true;

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
}

pub fn default_shell_timeout() -> u64 {
    30
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

pub fn default_mcp_enabled() -> bool {
    true
}

pub fn default_mcp_timeout() -> u64 {
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
    pub env: std::collections::HashMap<String, String>,

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
    /// Create from legacy ToolsConfig and McpConfig (migration helper)
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
                    triggers: None,
                },
            );
        }

        unified
    }

    /// Check if filesystem service is enabled
    pub fn is_fs_enabled(&self) -> bool {
        self.enabled && self.native.fs.as_ref().is_none_or(|c| c.enabled)
    }

    /// Check if git service is enabled
    pub fn is_git_enabled(&self) -> bool {
        self.enabled && self.native.git.as_ref().is_none_or(|c| c.enabled)
    }

    /// Check if shell service is enabled
    pub fn is_shell_enabled(&self) -> bool {
        self.enabled && self.native.shell.as_ref().is_some_and(|c| c.enabled)
    }

    /// Check if system info service is enabled
    pub fn is_system_info_enabled(&self) -> bool {
        self.enabled && self.native.system_info.as_ref().is_none_or(|c| c.enabled)
    }

    /// Get filesystem allowed roots
    pub fn fs_allowed_roots(&self) -> Vec<String> {
        self.native
            .fs
            .as_ref()
            .map_or(Vec::new(), |c| c.allowed_roots.clone())
    }

    /// Get git allowed repos
    pub fn git_allowed_repos(&self) -> Vec<String> {
        self.native
            .git
            .as_ref()
            .map_or(Vec::new(), |c| c.allowed_repos.clone())
    }

    /// Get shell configuration
    pub fn shell_config(&self) -> ShellToolConfig {
        self.native.shell.clone().unwrap_or_default()
    }

    /// Check if clipboard service is enabled
    pub fn is_clipboard_enabled(&self) -> bool {
        self.enabled && self.native.clipboard.as_ref().is_none_or(|c| c.enabled)
    }

    /// Check if screen capture service is enabled
    pub fn is_screen_capture_enabled(&self) -> bool {
        self.enabled
            && self
                .native
                .screen_capture
                .as_ref()
                .is_none_or(|c| c.enabled)
    }

    /// Get screen capture configuration
    pub fn screen_capture_config(&self) -> ScreenCaptureToolConfig {
        self.native.screen_capture.clone().unwrap_or_default()
    }

    /// Check if search tool service is enabled
    pub fn is_search_tool_enabled(&self) -> bool {
        self.enabled && self.native.search.as_ref().is_none_or(|c| c.enabled)
    }

    /// Get search tool configuration
    pub fn search_tool_config(&self) -> SearchToolConfig {
        self.native.search.clone().unwrap_or_default()
    }

    /// Get all enabled MCP servers
    pub fn enabled_mcp_servers(&self) -> Vec<(&String, &McpServerConfig)> {
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

pub fn default_max_dimension() -> u32 {
    1920
}

pub fn default_jpeg_quality() -> u8 {
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

/// Search tool configuration (wraps existing SearchRegistry as tool)
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

pub fn default_search_tool_max_results() -> usize {
    5
}

pub fn default_search_tool_timeout_seconds() -> u64 {
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
/// This is similar to McpExternalServerConfig but with a cleaner structure
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

    /// Trigger keywords for natural language command detection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggers: Option<Vec<String>>,
}
