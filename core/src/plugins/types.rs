//! Plugin system shared types
//!
//! Core data structures used throughout the plugin system.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ============================================================================
// Plugin Manifest Types (compatible with Claude Code plugin.json)
// ============================================================================

/// Plugin manifest (parsed from .claude-plugin/plugin.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name (required, used as namespace)
    pub name: String,

    /// Plugin version (semver format)
    #[serde(default)]
    pub version: Option<String>,

    /// Plugin description
    #[serde(default)]
    pub description: Option<String>,

    /// Plugin author information
    #[serde(default)]
    pub author: Option<PluginAuthor>,

    /// Plugin homepage URL
    #[serde(default)]
    pub homepage: Option<String>,

    /// Plugin repository URL
    #[serde(default)]
    pub repository: Option<PluginRepository>,

    /// Plugin license
    #[serde(default)]
    pub license: Option<String>,

    /// Keywords for search
    #[serde(default)]
    pub keywords: Option<Vec<String>>,

    // Custom paths (optional, override default locations)
    /// Custom commands directory path
    #[serde(default)]
    pub commands: Option<PathBuf>,

    /// Custom skills directory path
    #[serde(default)]
    pub skills: Option<PathBuf>,

    /// Custom agents directory path
    #[serde(default)]
    pub agents: Option<PathBuf>,

    /// Custom hooks file path
    #[serde(default)]
    pub hooks: Option<PathBuf>,

    /// Custom MCP servers file path
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: Option<PathBuf>,

    /// Custom LSP servers file path
    #[serde(rename = "lspServers", default)]
    pub lsp_servers: Option<PathBuf>,
}

/// Plugin author information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthor {
    /// Author name
    pub name: String,

    /// Author email
    #[serde(default)]
    pub email: Option<String>,

    /// Author URL
    #[serde(default)]
    pub url: Option<String>,
}

/// Plugin repository information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginRepository {
    /// Simple URL string
    Url(String),
    /// Detailed repository info
    Detailed {
        #[serde(rename = "type", default)]
        repo_type: Option<String>,
        url: String,
    },
}

// ============================================================================
// Skill Types (parsed from SKILL.md files)
// ============================================================================

/// Plugin skill (parsed from commands/ or skills/ SKILL.md)
#[derive(Debug, Clone)]
pub struct PluginSkill {
    /// Source plugin name
    pub plugin_name: String,

    /// Skill name (directory name)
    pub skill_name: String,

    /// Skill type (command or skill)
    pub skill_type: SkillType,

    /// Skill description (from frontmatter)
    pub description: String,

    /// Skill content (markdown body after frontmatter)
    pub content: String,

    /// Whether to disable automatic model invocation
    pub disable_model_invocation: bool,
}

impl PluginSkill {
    /// Get the fully qualified skill name (plugin:skill)
    pub fn qualified_name(&self) -> String {
        format!("{}:{}", self.plugin_name, self.skill_name)
    }

    /// Check if this skill can be auto-invoked by the model
    pub fn is_auto_invocable(&self) -> bool {
        !self.disable_model_invocation && self.skill_type == SkillType::Skill
    }
}

/// Skill type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillType {
    /// Command (from commands/ directory) - user-triggered
    Command,
    /// Skill (from skills/ directory) - can be auto-invoked
    Skill,
}

/// SKILL.md frontmatter
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill name (optional, defaults to directory name)
    #[serde(default)]
    pub name: Option<String>,

    /// Skill description
    #[serde(default)]
    pub description: Option<String>,

    /// Disable automatic model invocation
    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}

// ============================================================================
// Hook Types (parsed from hooks/hooks.json)
// ============================================================================

/// Plugin hooks configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginHooksConfig {
    /// Hooks by event type
    #[serde(default)]
    pub hooks: HashMap<HookEvent, Vec<HookMatcher>>,
}

/// Hook matcher with actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookMatcher {
    /// Regex pattern to match (e.g., "Write|Edit" for tool names)
    #[serde(default)]
    pub matcher: Option<String>,

    /// Actions to execute when matched
    pub hooks: Vec<HookAction>,
}

/// Hook action types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookAction {
    /// Execute a shell command
    Command {
        /// Command to execute (supports ${CLAUDE_PLUGIN_ROOT})
        command: String,
    },
    /// Evaluate a prompt with LLM
    Prompt {
        /// Prompt text (supports $ARGUMENTS)
        prompt: String,
    },
    /// Invoke an agent
    Agent {
        /// Agent name to invoke
        agent: String,
    },
}

/// Hook event types (Claude Code compatible)
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    /// Before tool use
    PreToolUse,
    /// After successful tool use
    PostToolUse,
    /// After failed tool use
    PostToolUseFailure,
    /// Permission request shown
    PermissionRequest,
    /// User submits prompt
    UserPromptSubmit,
    /// Notification sent
    Notification,
    /// Claude attempts to stop
    Stop,
    /// Subagent started
    SubagentStart,
    /// Subagent stopped
    SubagentStop,
    /// Setup/initialization
    Setup,
    /// Session started
    SessionStart,
    /// Session ended
    SessionEnd,
    /// Before context compaction
    PreCompact,
}

// ============================================================================
// Agent Types (parsed from agents/*.md)
// ============================================================================

/// Plugin agent definition
#[derive(Debug, Clone)]
pub struct PluginAgent {
    /// Source plugin name
    pub plugin_name: String,

    /// Agent name (directory/file name)
    pub agent_name: String,

    /// Agent description
    pub description: String,

    /// Agent capabilities
    pub capabilities: Vec<String>,

    /// System prompt (markdown body)
    pub system_prompt: String,
}

impl PluginAgent {
    /// Get the fully qualified agent name (plugin:agent)
    pub fn qualified_name(&self) -> String {
        format!("{}:{}", self.plugin_name, self.agent_name)
    }
}

/// Agent frontmatter
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFrontmatter {
    /// Agent description
    #[serde(default)]
    pub description: Option<String>,

    /// Agent capabilities
    #[serde(default)]
    pub capabilities: Vec<String>,
}

// ============================================================================
// MCP Types (parsed from .mcp.json)
// ============================================================================

/// Plugin MCP servers configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginMcpConfig {
    /// MCP servers by name
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, PluginMcpServer>,
}

/// Plugin MCP server definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMcpServer {
    /// Command to execute (e.g., "npx", "node", "uvx")
    pub command: String,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
}

// ============================================================================
// Plugin State Types
// ============================================================================

/// Loaded plugin information
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    /// Plugin manifest
    pub manifest: PluginManifest,

    /// Plugin root path
    pub path: PathBuf,

    /// Whether plugin is enabled
    pub enabled: bool,

    /// Loaded skills
    pub skills: Vec<PluginSkill>,

    /// Loaded hooks
    pub hooks: PluginHooksConfig,

    /// Loaded agents
    pub agents: Vec<PluginAgent>,

    /// MCP server configurations
    pub mcp_servers: PluginMcpConfig,
}

impl LoadedPlugin {
    /// Get plugin name
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// Get plugin version
    pub fn version(&self) -> Option<&str> {
        self.manifest.version.as_deref()
    }
}

/// Plugin info for FFI/display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Plugin name
    pub name: String,

    /// Plugin version
    pub version: Option<String>,

    /// Plugin description
    pub description: Option<String>,

    /// Whether plugin is enabled
    pub enabled: bool,

    /// Plugin root path
    pub path: String,

    /// Number of skills
    pub skills_count: usize,

    /// Number of agents
    pub agents_count: usize,

    /// Number of hook events
    pub hooks_count: usize,

    /// Number of MCP servers
    pub mcp_servers_count: usize,
}

impl From<&LoadedPlugin> for PluginInfo {
    fn from(plugin: &LoadedPlugin) -> Self {
        Self {
            name: plugin.manifest.name.clone(),
            version: plugin.manifest.version.clone(),
            description: plugin.manifest.description.clone(),
            enabled: plugin.enabled,
            path: plugin.path.to_string_lossy().to_string(),
            skills_count: plugin.skills.len(),
            agents_count: plugin.agents.len(),
            hooks_count: plugin.hooks.hooks.len(),
            mcp_servers_count: plugin.mcp_servers.mcp_servers.len(),
        }
    }
}

/// Persisted plugin state
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginStateFile {
    /// Plugin states by name
    #[serde(default)]
    pub plugins: HashMap<String, PluginState>,
}

/// Individual plugin state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginState {
    /// Whether plugin is enabled
    pub enabled: bool,

    /// Plugin version (for upgrade detection)
    #[serde(default)]
    pub version: Option<String>,
}
