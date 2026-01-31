//! Extension system type definitions
//!
//! Core data structures for skills, commands, agents, and plugins.

use crate::discovery::DiscoverySource;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// =============================================================================
// Skill Tool Types
// =============================================================================

/// Result of skill tool invocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolResult {
    /// Display title (e.g., "Loaded skill: my-skill")
    pub title: String,

    /// Rendered skill content with templates expanded
    pub content: String,

    /// Base directory for relative path references
    pub base_dir: PathBuf,

    /// Skill metadata
    pub metadata: SkillMetadata,
}

/// Metadata about an invoked skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill name
    pub name: String,

    /// Fully qualified name (plugin:skill or skill)
    pub qualified_name: String,

    /// Discovery source
    pub source: DiscoverySource,
}

/// Context for skill tool invocation (passed from agent loop)
#[derive(Debug, Clone, Default)]
pub struct SkillContext {
    /// Session identifier
    pub session_id: String,

    /// Agent-level permission rules (if any)
    pub agent_permissions: Option<HashMap<String, PermissionRule>>,
}

// =============================================================================
// Skill Types
// =============================================================================

/// Skill type (command vs skill)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SkillType {
    /// Command (from commands/ directory) - user-triggered via /command
    Command,
    /// Skill (from skills/ directory) - can be auto-invoked by LLM
    Skill,
}

/// Extension skill definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionSkill {
    /// Skill name (from directory name or frontmatter)
    pub name: String,

    /// Plugin name (if from a plugin)
    pub plugin_name: Option<String>,

    /// Skill type
    pub skill_type: SkillType,

    /// Description (from frontmatter)
    pub description: String,

    /// Skill content (markdown body after frontmatter)
    pub content: String,

    /// Whether to disable automatic model invocation
    pub disable_model_invocation: bool,

    /// Source path
    pub source_path: PathBuf,

    /// Discovery source
    pub source: DiscoverySource,
}

impl ExtensionSkill {
    /// Get the fully qualified name (plugin:skill or just skill)
    pub fn qualified_name(&self) -> String {
        match &self.plugin_name {
            Some(plugin) => format!("{}:{}", plugin, self.name),
            None => self.name.clone(),
        }
    }

    /// Check if this skill can be auto-invoked by the model
    pub fn is_auto_invocable(&self) -> bool {
        !self.disable_model_invocation && self.skill_type == SkillType::Skill
    }

    /// Substitute $ARGUMENTS placeholder
    pub fn with_arguments(&self, arguments: &str) -> String {
        self.content.replace("$ARGUMENTS", arguments)
    }

    /// Convert to SkillInfo for compatibility with ToolRegistry
    ///
    /// This allows ExtensionSkill to be registered with the existing
    /// tool registration system.
    pub fn to_skill_info(&self) -> crate::skills::SkillInfo {
        crate::skills::SkillInfo {
            id: self.qualified_name(),
            name: self.name.clone(),
            description: self.description.clone(),
            triggers: Vec::new(), // ExtensionSkill doesn't track triggers
            allowed_tools: Vec::new(), // ExtensionSkill doesn't track allowed tools
        }
    }

    /// Get the base directory for this skill (for file references)
    pub fn base_dir(&self) -> PathBuf {
        self.source_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

// =============================================================================
// Command Types (alias for user-triggered skills)
// =============================================================================

/// Extension command (user-triggered skill)
pub type ExtensionCommand = ExtensionSkill;

// =============================================================================
// Agent Types
// =============================================================================

/// Agent mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Primary agent (top-level, can be selected by user)
    Primary,
    /// Sub-agent (delegated to by primary agents)
    Subagent,
    /// Both primary and sub-agent
    #[default]
    All,
}

/// Permission rule for agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PermissionRule {
    /// Simple action for all patterns
    Simple(PermissionAction),
    /// Pattern-based rules
    Patterns(HashMap<String, PermissionAction>),
}

/// Permission action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

/// Extension agent definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionAgent {
    /// Agent name
    pub name: String,

    /// Plugin name (if from a plugin)
    pub plugin_name: Option<String>,

    /// Agent mode
    #[serde(default)]
    pub mode: AgentMode,

    /// Description
    #[serde(default)]
    pub description: Option<String>,

    /// Whether to hide from UI
    #[serde(default)]
    pub hidden: bool,

    /// UI color (hex format)
    #[serde(default)]
    pub color: Option<String>,

    /// Model specification (provider/model)
    #[serde(default)]
    pub model: Option<String>,

    /// Temperature
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Top P
    #[serde(default)]
    pub top_p: Option<f32>,

    /// Maximum iteration steps
    #[serde(default)]
    pub steps: Option<u32>,

    /// Tool permissions
    #[serde(default)]
    pub tools: Option<HashMap<String, bool>>,

    /// Permission rules
    #[serde(default)]
    pub permission: Option<HashMap<String, PermissionRule>>,

    /// Provider-specific options
    #[serde(default)]
    pub options: HashMap<String, serde_json::Value>,

    /// System prompt (markdown body)
    pub system_prompt: String,

    /// Source path
    pub source_path: PathBuf,

    /// Discovery source
    pub source: DiscoverySource,
}

impl ExtensionAgent {
    /// Get the fully qualified name
    pub fn qualified_name(&self) -> String {
        match &self.plugin_name {
            Some(plugin) => format!("{}:{}", plugin, self.name),
            None => self.name.clone(),
        }
    }

    /// Check if agent is a primary agent
    pub fn is_primary(&self) -> bool {
        matches!(self.mode, AgentMode::Primary | AgentMode::All)
    }

    /// Check if agent can be used as a sub-agent
    pub fn is_subagent(&self) -> bool {
        matches!(self.mode, AgentMode::Subagent | AgentMode::All)
    }
}

// =============================================================================
// Plugin Types
// =============================================================================

/// Loaded plugin
#[derive(Debug, Clone)]
pub struct ExtensionPlugin {
    /// Plugin name (from manifest)
    pub name: String,

    /// Plugin version
    pub version: Option<String>,

    /// Plugin description
    pub description: Option<String>,

    /// Plugin root path
    pub path: PathBuf,

    /// Whether plugin is enabled
    pub enabled: bool,

    /// Skills provided by this plugin
    pub skills: Vec<ExtensionSkill>,

    /// Commands provided by this plugin
    pub commands: Vec<ExtensionCommand>,

    /// Agents provided by this plugin
    pub agents: Vec<ExtensionAgent>,

    /// Hook configurations
    pub hooks: Vec<HookConfig>,

    /// MCP server configurations
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

impl ExtensionPlugin {
    /// Get plugin info
    pub fn info(&self) -> PluginInfo {
        PluginInfo {
            name: self.name.clone(),
            version: self.version.clone(),
            description: self.description.clone(),
            enabled: self.enabled,
            path: self.path.to_string_lossy().to_string(),
            skills_count: self.skills.len(),
            commands_count: self.commands.len(),
            agents_count: self.agents.len(),
            hooks_count: self.hooks.len(),
            mcp_servers_count: self.mcp_servers.len(),
        }
    }
}

/// Plugin info for display/FFI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub path: String,
    pub skills_count: usize,
    pub commands_count: usize,
    pub agents_count: usize,
    pub hooks_count: usize,
    pub mcp_servers_count: usize,
}

/// Plugin origin - where the plugin was discovered from
///
/// Higher priority origins override lower priority ones when plugins
/// have the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginOrigin {
    /// From explicit config (highest priority)
    Config,
    /// From workspace .aether/ directory
    Workspace,
    /// From global ~/.aether/ directory
    Global,
    /// Bundled with core (lowest priority)
    Bundled,
}

impl PluginOrigin {
    /// Get the priority of this origin (higher = takes precedence)
    pub fn priority(&self) -> u8 {
        match self {
            PluginOrigin::Config => 4,
            PluginOrigin::Workspace => 3,
            PluginOrigin::Global => 2,
            PluginOrigin::Bundled => 1,
        }
    }
}

/// Plugin kind - the type/format of the plugin
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// WebAssembly plugin (.wasm)
    Wasm,
    /// Node.js plugin (package.json)
    NodeJs,
    /// Static content plugin (markdown files)
    Static,
}

impl PluginKind {
    /// Detect plugin kind from a file path
    ///
    /// Returns `Some(kind)` if the path indicates a known plugin type,
    /// `None` otherwise.
    pub fn detect_from_path(path: &Path) -> Option<Self> {
        let filename = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str());

        match (filename, ext) {
            (_, Some("wasm")) => Some(PluginKind::Wasm),
            ("package.json", _) => Some(PluginKind::NodeJs),
            ("aether.plugin.json", _) => Some(PluginKind::Wasm),
            ("SKILL.md" | "COMMAND.md" | "AGENT.md", _) => Some(PluginKind::Static),
            (_, Some("md")) => Some(PluginKind::Static),
            _ => None,
        }
    }
}

// =============================================================================
// Hook Types
// =============================================================================

/// Hook event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    SessionStart,
    SessionEnd,
    PreCompact,
    UserPromptSubmit,
    PermissionRequest,
    SubagentStart,
    SubagentStop,
    Stop,
    Notification,
    Setup,
    // Enhanced events (for JS plugins)
    ChatMessage,
    ChatParams,
    ChatResponse,
    CommandExecuteBefore,
    CommandExecuteAfter,
}

/// Hook action types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookAction {
    /// Execute a shell command
    Command { command: String },
    /// Provide a prompt for LLM evaluation
    Prompt { prompt: String },
    /// Invoke an agent
    Agent { agent: String },
}

/// Hook configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Event to hook
    pub event: HookEvent,
    /// Regex pattern to match (for tool-based events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// Actions to execute
    pub actions: Vec<HookAction>,
    /// Plugin name (for logging)
    #[serde(default)]
    pub plugin_name: String,
    /// Plugin root (for variable substitution)
    #[serde(skip)]
    pub plugin_root: PathBuf,
}

// =============================================================================
// MCP Types
// =============================================================================

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Command to execute
    pub command: String,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
}

// =============================================================================
// Frontmatter Types
// =============================================================================

/// Skill/Command frontmatter
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}

/// Agent frontmatter
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFrontmatter {
    #[serde(default)]
    pub mode: Option<AgentMode>,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub hidden: Option<bool>,

    #[serde(default)]
    pub color: Option<String>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub temperature: Option<f32>,

    #[serde(default)]
    pub top_p: Option<f32>,

    #[serde(default)]
    pub steps: Option<u32>,

    #[serde(default)]
    pub tools: Option<HashMap<String, bool>>,

    #[serde(default)]
    pub permission: Option<HashMap<String, PermissionRule>>,

    #[serde(default)]
    pub options: Option<HashMap<String, serde_json::Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_qualified_name() {
        let skill = ExtensionSkill {
            name: "hello".to_string(),
            plugin_name: Some("my-plugin".to_string()),
            skill_type: SkillType::Skill,
            description: "Test".to_string(),
            content: "Content".to_string(),
            disable_model_invocation: false,
            source_path: PathBuf::from("/test"),
            source: DiscoverySource::AetherGlobal,
        };

        assert_eq!(skill.qualified_name(), "my-plugin:hello");
    }

    #[test]
    fn test_skill_with_arguments() {
        let skill = ExtensionSkill {
            name: "greet".to_string(),
            plugin_name: None,
            skill_type: SkillType::Command,
            description: "Greet someone".to_string(),
            content: "Hello, $ARGUMENTS!".to_string(),
            disable_model_invocation: false,
            source_path: PathBuf::from("/test"),
            source: DiscoverySource::AetherGlobal,
        };

        assert_eq!(skill.with_arguments("World"), "Hello, World!");
    }

    #[test]
    fn test_agent_mode() {
        let agent = ExtensionAgent {
            name: "test".to_string(),
            plugin_name: None,
            mode: AgentMode::Subagent,
            description: None,
            hidden: false,
            color: None,
            model: None,
            temperature: None,
            top_p: None,
            steps: None,
            tools: None,
            permission: None,
            options: HashMap::new(),
            system_prompt: "Test".to_string(),
            source_path: PathBuf::from("/test"),
            source: DiscoverySource::AetherGlobal,
        };

        assert!(!agent.is_primary());
        assert!(agent.is_subagent());
    }

    #[test]
    fn test_plugin_origin_priority() {
        assert!(PluginOrigin::Config.priority() > PluginOrigin::Workspace.priority());
        assert!(PluginOrigin::Workspace.priority() > PluginOrigin::Global.priority());
        assert!(PluginOrigin::Global.priority() > PluginOrigin::Bundled.priority());
    }

    #[test]
    fn test_plugin_origin_serde() {
        let origin = PluginOrigin::Config;
        let json = serde_json::to_string(&origin).unwrap();
        assert_eq!(json, "\"config\"");

        let parsed: PluginOrigin = serde_json::from_str("\"workspace\"").unwrap();
        assert_eq!(parsed, PluginOrigin::Workspace);
    }

    #[test]
    fn test_plugin_kind_detection() {
        use std::path::Path;

        // Wasm detection
        assert_eq!(
            PluginKind::detect_from_path(Path::new("plugin.wasm")),
            Some(PluginKind::Wasm)
        );
        assert_eq!(
            PluginKind::detect_from_path(Path::new("/path/to/my-plugin.wasm")),
            Some(PluginKind::Wasm)
        );

        // Node.js detection
        assert_eq!(
            PluginKind::detect_from_path(Path::new("package.json")),
            Some(PluginKind::NodeJs)
        );
        assert_eq!(
            PluginKind::detect_from_path(Path::new("/some/dir/package.json")),
            Some(PluginKind::NodeJs)
        );

        // Wasm plugin manifest
        assert_eq!(
            PluginKind::detect_from_path(Path::new("aether.plugin.json")),
            Some(PluginKind::Wasm)
        );

        // Static content detection
        assert_eq!(
            PluginKind::detect_from_path(Path::new("SKILL.md")),
            Some(PluginKind::Static)
        );
        assert_eq!(
            PluginKind::detect_from_path(Path::new("COMMAND.md")),
            Some(PluginKind::Static)
        );
        assert_eq!(
            PluginKind::detect_from_path(Path::new("AGENT.md")),
            Some(PluginKind::Static)
        );
        assert_eq!(
            PluginKind::detect_from_path(Path::new("README.md")),
            Some(PluginKind::Static)
        );

        // Unknown files
        assert_eq!(PluginKind::detect_from_path(Path::new("config.yaml")), None);
        assert_eq!(PluginKind::detect_from_path(Path::new("main.rs")), None);
        assert_eq!(PluginKind::detect_from_path(Path::new("Cargo.toml")), None);
    }

    #[test]
    fn test_plugin_kind_serde() {
        let kind = PluginKind::Wasm;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"wasm\"");

        let parsed: PluginKind = serde_json::from_str("\"nodejs\"").unwrap();
        assert_eq!(parsed, PluginKind::NodeJs);
    }
}
