//! Extension system type definitions
//!
//! Core data structures for skills, commands, agents, and plugins.

use crate::discovery::DiscoverySource;
use schemars::JsonSchema;
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

/// Direct command execution result
///
/// Used by commands that execute immediately without LLM involvement
/// (e.g., `/status`, `/clear`, `/version`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectCommandResult {
    /// Command output to display to user
    pub content: String,
    /// Optional structured data
    pub data: Option<serde_json::Value>,
    /// Whether command was successful
    pub success: bool,
}

impl DirectCommandResult {
    /// Create a successful result with content only
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            data: None,
            success: true,
        }
    }

    /// Create a successful result with content and structured data
    pub fn with_data(content: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            content: content.into(),
            data: Some(data),
            success: true,
        }
    }

    /// Create an error result
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            data: None,
            success: false,
        }
    }
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

    /// V2: Prompt injection scope
    #[serde(default)]
    pub scope: PromptScope,

    /// V2: Bound tool name (for Tool scope)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_tool: Option<String>,

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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PermissionRule {
    /// Simple action for all patterns
    Simple(PermissionAction),
    /// Pattern-based rules
    Patterns(HashMap<String, PermissionAction>),
}

/// Permission action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// Plugin info for display
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

/// Plugin status - the runtime state of a plugin
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus {
    /// Plugin is loaded and active
    Loaded,
    /// Plugin is disabled by user
    Disabled,
    /// Plugin is overridden by a higher-priority plugin with the same name
    Overridden,
    /// Plugin failed to load with an error
    Error(String),
}

impl PluginStatus {
    /// Check if this plugin is actively running
    pub fn is_active(&self) -> bool {
        matches!(self, PluginStatus::Loaded)
    }
}

/// Plugin record - comprehensive plugin information for registry tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    /// Unique plugin identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Version string (semver)
    pub version: Option<String>,
    /// Plugin description
    pub description: Option<String>,
    /// Plugin type/format
    pub kind: PluginKind,
    /// Discovery origin
    pub origin: PluginOrigin,
    /// Current status
    pub status: PluginStatus,
    /// Error message if status is Error
    pub error: Option<String>,
    /// Root directory of the plugin
    pub root_dir: PathBuf,
    // Registration tracking
    /// Tool names registered by this plugin
    pub tool_names: Vec<String>,
    /// Number of hooks registered
    pub hook_count: usize,
    /// Channel IDs registered by this plugin
    pub channel_ids: Vec<String>,
    /// Provider IDs registered by this plugin
    pub provider_ids: Vec<String>,
    /// Gateway RPC methods registered by this plugin
    pub gateway_methods: Vec<String>,
    /// Service IDs registered by this plugin
    pub service_ids: Vec<String>,
}

impl PluginRecord {
    /// Create a new plugin record with default values
    pub fn new(id: String, name: String, kind: PluginKind, origin: PluginOrigin) -> Self {
        Self {
            id,
            name,
            version: None,
            description: None,
            kind,
            origin,
            status: PluginStatus::Loaded,
            error: None,
            root_dir: PathBuf::new(),
            tool_names: Vec::new(),
            hook_count: 0,
            channel_ids: Vec::new(),
            provider_ids: Vec::new(),
            gateway_methods: Vec::new(),
            service_ids: Vec::new(),
        }
    }

    /// Set an error status with message
    pub fn with_error(mut self, error: String) -> Self {
        self.status = PluginStatus::Error(error.clone());
        self.error = Some(error);
        self
    }

    /// Set the root directory
    pub fn with_root_dir(mut self, path: PathBuf) -> Self {
        self.root_dir = path;
        self
    }
}

// =============================================================================
// Hook Types
// =============================================================================

/// Hook event types for shell-based hooks (Claude Code compatible).
///
/// This enum is used by the shell hook system where external commands are
/// executed in response to events. It uses **PascalCase** serialization for
/// compatibility with Claude Code's hook configuration format.
///
/// # Difference from PluginHookEvent
///
/// **`HookEvent`** (this enum):
/// - For shell command hooks configured in CLAUDE.md or config files
/// - Uses PascalCase serialization (`"PreToolUse"`, `"SessionStart"`)
/// - Oriented toward CLI/shell integration
///
/// **[`PluginHookEvent`](crate::extension::registry::PluginHookEvent)**:
/// - For WASM/Node.js plugin hooks registered via Plugin API
/// - Uses snake_case serialization (`"before_tool_call"`, `"session_start"`)
/// - Oriented toward plugin lifecycle and inter-process communication
///
/// # Example (hooks config in CLAUDE.md)
/// ```json
/// {
///   "hooks": {
///     "PreToolUse": [{ "command": "my-hook.sh" }],
///     "SessionStart": [{ "command": "setup.sh" }]
///   }
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    /// Before a tool is used
    PreToolUse,
    /// After a tool completes successfully
    PostToolUse,
    /// After a tool fails
    PostToolUseFailure,
    /// When a session starts
    SessionStart,
    /// When a session ends
    SessionEnd,
    /// Before session compaction
    PreCompact,
    /// When user submits a prompt
    UserPromptSubmit,
    /// When a permission is requested
    PermissionRequest,
    /// When a subagent starts
    SubagentStart,
    /// When a subagent stops
    SubagentStop,
    /// When processing stops
    Stop,
    /// When a notification is sent
    Notification,
    /// During initial setup
    Setup,
    // Enhanced events (for JS plugins)
    /// When a chat message is received
    ChatMessage,
    /// When chat parameters are configured
    ChatParams,
    /// When a chat response is generated
    ChatResponse,
    /// Before a command executes
    CommandExecuteBefore,
    /// After a command executes
    CommandExecuteAfter,
}

/// Hook execution kind - determines how the hook is executed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HookKind {
    /// Interceptor: Pipeline execution, can modify context or block
    /// Execution: Sequential by priority, short-circuit on block
    Interceptor,

    /// Observer: Fire-and-forget, read-only context
    /// Execution: Parallel, errors logged but not propagated
    #[default]
    Observer,

    /// Resolver: First-win competition
    /// Execution: Sequential by priority, stops when one returns Some
    Resolver,
}

impl HookKind {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "interceptor" => HookKind::Interceptor,
            "resolver" => HookKind::Resolver,
            _ => HookKind::Observer,
        }
    }
}

/// Hook priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum HookPriority {
    /// System-level hooks (security, audit) - runs first
    System = -1000,
    /// High priority business logic
    High = -100,
    /// Default priority
    #[default]
    Normal = 0,
    /// Low priority extensions
    Low = 100,
}


impl HookPriority {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "system" => HookPriority::System,
            "high" => HookPriority::High,
            "low" => HookPriority::Low,
            _ => HookPriority::Normal,
        }
    }

    pub fn as_i32(&self) -> i32 {
        match self {
            HookPriority::System => -1000,
            HookPriority::High => -100,
            HookPriority::Normal => 0,
            HookPriority::Low => 100,
        }
    }
}

/// Prompt injection scope
///
/// Determines when and how a prompt is injected into the conversation context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptScope {
    /// System-level: Always injected when plugin is active
    #[default]
    System,

    /// Tool-bound: Injected when specific tool is available
    Tool,

    /// Standalone: User must explicitly invoke (command)
    Standalone,

    /// Disabled: Not injected
    Disabled,
}

impl PromptScope {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "system" => PromptScope::System,
            "tool" => PromptScope::Tool,
            "standalone" => PromptScope::Standalone,
            "disabled" => PromptScope::Disabled,
            _ => PromptScope::System,
        }
    }
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

/// Hook configuration - defines when and how a hook executes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Event to hook
    pub event: HookEvent,

    /// Hook execution kind (V2)
    #[serde(default)]
    pub kind: HookKind,

    /// Hook priority (V2)
    #[serde(default)]
    pub priority: HookPriority,

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

    /// Handler function name (for runtime plugins)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
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
// Service Types (V2 Background Services)
// =============================================================================

/// Service state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ServiceState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}


/// Running service information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub id: String,
    pub plugin_id: String,
    pub name: String,
    pub state: ServiceState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

/// Service lifecycle result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResult {
    pub success: bool,
    pub message: Option<String>,
    pub data: Option<serde_json::Value>,
}

impl ServiceResult {
    pub fn ok() -> Self {
        Self {
            success: true,
            message: None,
            data: None,
        }
    }

    pub fn ok_with_message(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(msg.into()),
            data: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(msg.into()),
            data: None,
        }
    }
}

// =============================================================================
// Channel Types (V2 Plugin Channels)
// =============================================================================

/// Channel message from external platform
///
/// Represents an incoming message from a plugin-provided messaging channel
/// (e.g., Telegram, Discord, Slack, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    /// Unique channel identifier (e.g., "telegram", "discord")
    pub channel_id: String,
    /// Conversation/chat identifier within the channel
    pub conversation_id: String,
    /// Sender identifier (user ID on the platform)
    pub sender_id: String,
    /// Message content
    pub content: String,
    /// When the message was sent
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Platform-specific metadata (e.g., message_id, attachments)
    pub metadata: Option<serde_json::Value>,
}

/// Channel send request
///
/// Request to send a message through a plugin-provided channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSendRequest {
    /// Conversation/chat identifier to send to
    pub conversation_id: String,
    /// Message content to send
    pub content: String,
    /// Optional message ID to reply to
    pub reply_to: Option<String>,
    /// Platform-specific options (e.g., parse_mode, disable_notification)
    pub metadata: Option<serde_json::Value>,
}

/// Channel connection state
///
/// Represents the current connection status of a plugin channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ChannelState {
    /// Channel is not connected
    #[default]
    Disconnected,
    /// Channel is attempting to connect
    Connecting,
    /// Channel is connected and operational
    Connected,
    /// Channel lost connection and is attempting to reconnect
    Reconnecting,
    /// Channel connection failed
    Failed,
}


/// Channel info
///
/// Describes a plugin-provided messaging channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    /// Unique channel identifier
    pub id: String,
    /// Plugin that provides this channel
    pub plugin_id: String,
    /// Human-readable label (e.g., "Telegram Bot")
    pub label: String,
    /// Current connection state
    pub state: ChannelState,
    /// Error message if state is Failed
    pub error: Option<String>,
}

// =============================================================================
// Provider Types (V2 Plugin Providers)
// =============================================================================

/// Provider chat request
///
/// Represents a chat completion request to a plugin-provided AI model provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderChatRequest {
    /// Model identifier (e.g., "gpt-4", "claude-3-opus")
    pub model: String,
    /// Conversation messages
    pub messages: Vec<ProviderMessage>,
    /// Sampling temperature (0.0 - 2.0)
    pub temperature: Option<f32>,
    /// Maximum tokens to generate
    pub max_tokens: Option<u32>,
    /// Whether to stream the response
    pub stream: bool,
}

/// Provider message
///
/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderMessage {
    /// Message role (e.g., "system", "user", "assistant")
    pub role: String,
    /// Message content
    pub content: String,
}

/// Provider chat response (non-streaming)
///
/// Complete response from a chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderChatResponse {
    /// Generated response content
    pub content: String,
    /// Reason the generation stopped (e.g., "stop", "length", "tool_calls")
    pub finish_reason: Option<String>,
    /// Token usage statistics
    pub usage: Option<ProviderUsage>,
}

/// Provider usage info
///
/// Token usage statistics for a completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    /// Number of tokens in the prompt
    pub prompt_tokens: u32,
    /// Number of tokens in the completion
    pub completion_tokens: u32,
    /// Total tokens used (prompt + completion)
    pub total_tokens: u32,
}

/// Provider streaming chunk
///
/// A chunk of data in a streaming response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderStreamChunk {
    /// Content delta - partial response text
    #[serde(rename = "delta")]
    Delta { content: String },
    /// Stream completed
    #[serde(rename = "done")]
    Done { usage: Option<ProviderUsage> },
    /// Error occurred during streaming
    #[serde(rename = "error")]
    Error { message: String },
}

/// Provider model info
///
/// Describes a model available from a plugin-provided AI provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelInfo {
    /// Model identifier
    pub id: String,
    /// Human-readable display name
    pub display_name: String,
    /// Context window size in tokens
    pub context_window: Option<u32>,
    /// Whether the model supports tool/function calling
    pub supports_tools: bool,
    /// Whether the model supports vision/image inputs
    pub supports_vision: bool,
}

// =============================================================================
// HTTP Route Types (V2 Plugin HTTP Endpoints)
// =============================================================================

/// HTTP request from plugin route
///
/// Represents an incoming HTTP request to a plugin-provided endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    /// HTTP method (e.g., "GET", "POST", "PUT", "DELETE")
    pub method: String,
    /// Request path (e.g., "/api/webhook")
    pub path: String,
    /// HTTP headers as key-value pairs
    pub headers: HashMap<String, String>,
    /// Query string parameters
    pub query: HashMap<String, String>,
    /// Request body (for POST/PUT/PATCH requests)
    pub body: Option<serde_json::Value>,
    /// Path parameters extracted from route patterns (e.g., ":id" -> "123")
    pub path_params: HashMap<String, String>,
}

/// HTTP response from plugin handler
///
/// Response to send back to the HTTP client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    /// HTTP status code (e.g., 200, 404, 500)
    pub status: u16,
    /// HTTP response headers
    pub headers: HashMap<String, String>,
    /// Response body
    pub body: Option<serde_json::Value>,
}

impl HttpResponse {
    /// Create a 200 OK response with no body
    pub fn ok() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: None,
        }
    }

    /// Create a 200 OK response with JSON body
    pub fn json(data: serde_json::Value) -> Self {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        Self {
            status: 200,
            headers,
            body: Some(data),
        }
    }

    /// Create an error response with the given status code and message
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: Some(serde_json::json!({"error": message.into()})),
        }
    }

    /// Create a 404 Not Found response
    pub fn not_found() -> Self {
        Self::error(404, "Not Found")
    }

    /// Create a 400 Bad Request response
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::error(400, message)
    }

    /// Create a 500 Internal Server Error response
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::error(500, message)
    }
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

    /// V2: Prompt injection scope
    #[serde(default)]
    pub scope: Option<PromptScope>,

    /// V2: Bound tool name (for Tool scope)
    #[serde(rename = "bound-tool", default)]
    pub bound_tool: Option<String>,
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
            scope: PromptScope::System,
            bound_tool: None,
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
            scope: PromptScope::System,
            bound_tool: None,
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

    #[test]
    fn test_plugin_record_creation() {
        let record = PluginRecord::new(
            "test-plugin".to_string(),
            "Test Plugin".to_string(),
            PluginKind::Wasm,
            PluginOrigin::Global,
        );
        assert_eq!(record.id, "test-plugin");
        assert_eq!(record.name, "Test Plugin");
        assert_eq!(record.kind, PluginKind::Wasm);
        assert_eq!(record.origin, PluginOrigin::Global);
        assert_eq!(record.status, PluginStatus::Loaded);
        assert!(record.tool_names.is_empty());
        assert!(record.channel_ids.is_empty());
        assert!(record.provider_ids.is_empty());
        assert!(record.gateway_methods.is_empty());
        assert!(record.service_ids.is_empty());
        assert_eq!(record.hook_count, 0);
    }

    #[test]
    fn test_plugin_record_with_error() {
        let record = PluginRecord::new(
            "broken-plugin".to_string(),
            "Broken Plugin".to_string(),
            PluginKind::NodeJs,
            PluginOrigin::Workspace,
        )
        .with_error("Failed to load".to_string());

        assert_eq!(record.status, PluginStatus::Error("Failed to load".to_string()));
        assert_eq!(record.error, Some("Failed to load".to_string()));
    }

    #[test]
    fn test_plugin_record_with_root_dir() {
        let record = PluginRecord::new(
            "my-plugin".to_string(),
            "My Plugin".to_string(),
            PluginKind::Static,
            PluginOrigin::Config,
        )
        .with_root_dir(PathBuf::from("/path/to/plugin"));

        assert_eq!(record.root_dir, PathBuf::from("/path/to/plugin"));
    }

    #[test]
    fn test_plugin_status_is_active() {
        assert!(PluginStatus::Loaded.is_active());
        assert!(!PluginStatus::Disabled.is_active());
        assert!(!PluginStatus::Overridden.is_active());
        assert!(!PluginStatus::Error("test".to_string()).is_active());
    }

    #[test]
    fn test_plugin_status_serde() {
        // Loaded
        let status = PluginStatus::Loaded;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"loaded\"");

        // Disabled
        let status = PluginStatus::Disabled;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"disabled\"");

        // Overridden
        let status = PluginStatus::Overridden;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"overridden\"");

        // Error with message
        let status = PluginStatus::Error("something went wrong".to_string());
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("something went wrong"));

        // Parse back
        let parsed: PluginStatus = serde_json::from_str("\"loaded\"").unwrap();
        assert_eq!(parsed, PluginStatus::Loaded);
    }

    #[test]
    fn test_direct_command_result_success() {
        let result = DirectCommandResult::success("Operation completed");
        assert_eq!(result.content, "Operation completed");
        assert!(result.success);
        assert!(result.data.is_none());
    }

    #[test]
    fn test_direct_command_result_with_data() {
        let data = serde_json::json!({"count": 42, "items": ["a", "b"]});
        let result = DirectCommandResult::with_data("Found items", data.clone());
        assert_eq!(result.content, "Found items");
        assert!(result.success);
        assert_eq!(result.data, Some(data));
    }

    #[test]
    fn test_direct_command_result_error() {
        let result = DirectCommandResult::error("Something went wrong");
        assert_eq!(result.content, "Something went wrong");
        assert!(!result.success);
        assert!(result.data.is_none());
    }

    #[test]
    fn test_direct_command_result_serde() {
        let result = DirectCommandResult::with_data(
            "Test output",
            serde_json::json!({"key": "value"}),
        );
        let json = serde_json::to_string(&result).unwrap();
        let parsed: DirectCommandResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content, "Test output");
        assert!(parsed.success);
        assert!(parsed.data.is_some());
    }

    // =========================================================================
    // Service Types Tests
    // =========================================================================

    #[test]
    fn test_service_state_default() {
        let state = ServiceState::default();
        assert_eq!(state, ServiceState::Stopped);
    }

    #[test]
    fn test_service_state_serde() {
        // All states serialize to lowercase
        assert_eq!(serde_json::to_string(&ServiceState::Stopped).unwrap(), "\"stopped\"");
        assert_eq!(serde_json::to_string(&ServiceState::Starting).unwrap(), "\"starting\"");
        assert_eq!(serde_json::to_string(&ServiceState::Running).unwrap(), "\"running\"");
        assert_eq!(serde_json::to_string(&ServiceState::Stopping).unwrap(), "\"stopping\"");
        assert_eq!(serde_json::to_string(&ServiceState::Failed).unwrap(), "\"failed\"");

        // Parse back
        let parsed: ServiceState = serde_json::from_str("\"running\"").unwrap();
        assert_eq!(parsed, ServiceState::Running);
    }

    #[test]
    fn test_service_info_serde() {
        let info = ServiceInfo {
            id: "svc-123".to_string(),
            plugin_id: "my-plugin".to_string(),
            name: "background-worker".to_string(),
            state: ServiceState::Running,
            started_at: Some(chrono::Utc::now()),
            error: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: ServiceInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "svc-123");
        assert_eq!(parsed.plugin_id, "my-plugin");
        assert_eq!(parsed.name, "background-worker");
        assert_eq!(parsed.state, ServiceState::Running);
        assert!(parsed.started_at.is_some());
        assert!(parsed.error.is_none());
    }

    #[test]
    fn test_service_info_with_error() {
        let info = ServiceInfo {
            id: "svc-456".to_string(),
            plugin_id: "broken-plugin".to_string(),
            name: "failing-service".to_string(),
            state: ServiceState::Failed,
            started_at: None,
            error: Some("Connection refused".to_string()),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("Connection refused"));

        let parsed: ServiceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.state, ServiceState::Failed);
        assert_eq!(parsed.error, Some("Connection refused".to_string()));
    }

    #[test]
    fn test_service_result_ok() {
        let result = ServiceResult::ok();
        assert!(result.success);
        assert!(result.message.is_none());
        assert!(result.data.is_none());
    }

    #[test]
    fn test_service_result_ok_with_message() {
        let result = ServiceResult::ok_with_message("Service started successfully");
        assert!(result.success);
        assert_eq!(result.message, Some("Service started successfully".to_string()));
        assert!(result.data.is_none());
    }

    #[test]
    fn test_service_result_error() {
        let result = ServiceResult::error("Failed to start service");
        assert!(!result.success);
        assert_eq!(result.message, Some("Failed to start service".to_string()));
        assert!(result.data.is_none());
    }

    #[test]
    fn test_service_result_serde() {
        let result = ServiceResult::ok_with_message("Done");
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ServiceResult = serde_json::from_str(&json).unwrap();

        assert!(parsed.success);
        assert_eq!(parsed.message, Some("Done".to_string()));
    }

    // =========================================================================
    // Channel Types Tests
    // =========================================================================

    #[test]
    fn test_channel_state_default() {
        let state = ChannelState::default();
        assert_eq!(state, ChannelState::Disconnected);
    }

    #[test]
    fn test_channel_state_serde() {
        // All states serialize to lowercase
        assert_eq!(serde_json::to_string(&ChannelState::Disconnected).unwrap(), "\"disconnected\"");
        assert_eq!(serde_json::to_string(&ChannelState::Connecting).unwrap(), "\"connecting\"");
        assert_eq!(serde_json::to_string(&ChannelState::Connected).unwrap(), "\"connected\"");
        assert_eq!(serde_json::to_string(&ChannelState::Reconnecting).unwrap(), "\"reconnecting\"");
        assert_eq!(serde_json::to_string(&ChannelState::Failed).unwrap(), "\"failed\"");

        // Parse back
        let parsed: ChannelState = serde_json::from_str("\"connected\"").unwrap();
        assert_eq!(parsed, ChannelState::Connected);
    }

    #[test]
    fn test_channel_message_serde() {
        let msg = ChannelMessage {
            channel_id: "telegram".to_string(),
            conversation_id: "chat-12345".to_string(),
            sender_id: "user-67890".to_string(),
            content: "Hello, Aether!".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: Some(serde_json::json!({"message_id": 42})),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ChannelMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.channel_id, "telegram");
        assert_eq!(parsed.conversation_id, "chat-12345");
        assert_eq!(parsed.sender_id, "user-67890");
        assert_eq!(parsed.content, "Hello, Aether!");
        assert!(parsed.metadata.is_some());
    }

    #[test]
    fn test_channel_message_without_metadata() {
        let msg = ChannelMessage {
            channel_id: "discord".to_string(),
            conversation_id: "guild-123#channel-456".to_string(),
            sender_id: "user-789".to_string(),
            content: "Test message".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ChannelMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.channel_id, "discord");
        assert!(parsed.metadata.is_none());
    }

    #[test]
    fn test_channel_send_request_serde() {
        let req = ChannelSendRequest {
            conversation_id: "chat-12345".to_string(),
            content: "Hello back!".to_string(),
            reply_to: Some("msg-999".to_string()),
            metadata: Some(serde_json::json!({"parse_mode": "HTML"})),
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ChannelSendRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.conversation_id, "chat-12345");
        assert_eq!(parsed.content, "Hello back!");
        assert_eq!(parsed.reply_to, Some("msg-999".to_string()));
        assert!(parsed.metadata.is_some());
    }

    #[test]
    fn test_channel_send_request_minimal() {
        let req = ChannelSendRequest {
            conversation_id: "chat-abc".to_string(),
            content: "Simple message".to_string(),
            reply_to: None,
            metadata: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ChannelSendRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.conversation_id, "chat-abc");
        assert_eq!(parsed.content, "Simple message");
        assert!(parsed.reply_to.is_none());
        assert!(parsed.metadata.is_none());
    }

    #[test]
    fn test_channel_info_serde() {
        let info = ChannelInfo {
            id: "telegram-bot".to_string(),
            plugin_id: "telegram-plugin".to_string(),
            label: "Telegram Bot".to_string(),
            state: ChannelState::Connected,
            error: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: ChannelInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "telegram-bot");
        assert_eq!(parsed.plugin_id, "telegram-plugin");
        assert_eq!(parsed.label, "Telegram Bot");
        assert_eq!(parsed.state, ChannelState::Connected);
        assert!(parsed.error.is_none());
    }

    #[test]
    fn test_channel_info_with_error() {
        let info = ChannelInfo {
            id: "discord-bot".to_string(),
            plugin_id: "discord-plugin".to_string(),
            label: "Discord Bot".to_string(),
            state: ChannelState::Failed,
            error: Some("Invalid bot token".to_string()),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("Invalid bot token"));

        let parsed: ChannelInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.state, ChannelState::Failed);
        assert_eq!(parsed.error, Some("Invalid bot token".to_string()));
    }

    // =========================================================================
    // Provider Types Tests
    // =========================================================================

    #[test]
    fn test_provider_message_serde() {
        let msg = ProviderMessage {
            role: "user".to_string(),
            content: "Hello, AI!".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ProviderMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.role, "user");
        assert_eq!(parsed.content, "Hello, AI!");
    }

    #[test]
    fn test_provider_chat_request_serde() {
        let req = ProviderChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                ProviderMessage {
                    role: "system".to_string(),
                    content: "You are a helpful assistant.".to_string(),
                },
                ProviderMessage {
                    role: "user".to_string(),
                    content: "Hello!".to_string(),
                },
            ],
            temperature: Some(0.7),
            max_tokens: Some(1000),
            stream: false,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ProviderChatRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.model, "gpt-4");
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.temperature, Some(0.7));
        assert_eq!(parsed.max_tokens, Some(1000));
        assert!(!parsed.stream);
    }

    #[test]
    fn test_provider_chat_request_minimal() {
        let req = ProviderChatRequest {
            model: "claude-3".to_string(),
            messages: vec![ProviderMessage {
                role: "user".to_string(),
                content: "Hi".to_string(),
            }],
            temperature: None,
            max_tokens: None,
            stream: true,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ProviderChatRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.model, "claude-3");
        assert_eq!(parsed.messages.len(), 1);
        assert!(parsed.temperature.is_none());
        assert!(parsed.max_tokens.is_none());
        assert!(parsed.stream);
    }

    #[test]
    fn test_provider_usage_serde() {
        let usage = ProviderUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };

        let json = serde_json::to_string(&usage).unwrap();
        let parsed: ProviderUsage = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.prompt_tokens, 100);
        assert_eq!(parsed.completion_tokens, 50);
        assert_eq!(parsed.total_tokens, 150);
    }

    #[test]
    fn test_provider_chat_response_serde() {
        let resp = ProviderChatResponse {
            content: "Hello! How can I help you?".to_string(),
            finish_reason: Some("stop".to_string()),
            usage: Some(ProviderUsage {
                prompt_tokens: 10,
                completion_tokens: 8,
                total_tokens: 18,
            }),
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ProviderChatResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.content, "Hello! How can I help you?");
        assert_eq!(parsed.finish_reason, Some("stop".to_string()));
        assert!(parsed.usage.is_some());
        assert_eq!(parsed.usage.unwrap().total_tokens, 18);
    }

    #[test]
    fn test_provider_chat_response_minimal() {
        let resp = ProviderChatResponse {
            content: "Response text".to_string(),
            finish_reason: None,
            usage: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ProviderChatResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.content, "Response text");
        assert!(parsed.finish_reason.is_none());
        assert!(parsed.usage.is_none());
    }

    #[test]
    fn test_provider_stream_chunk_delta() {
        let chunk = ProviderStreamChunk::Delta {
            content: "Hello".to_string(),
        };

        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"type\":\"delta\""));
        assert!(json.contains("\"content\":\"Hello\""));

        let parsed: ProviderStreamChunk = serde_json::from_str(&json).unwrap();
        match parsed {
            ProviderStreamChunk::Delta { content } => {
                assert_eq!(content, "Hello");
            }
            _ => panic!("Expected Delta variant"),
        }
    }

    #[test]
    fn test_provider_stream_chunk_done() {
        let chunk = ProviderStreamChunk::Done {
            usage: Some(ProviderUsage {
                prompt_tokens: 50,
                completion_tokens: 25,
                total_tokens: 75,
            }),
        };

        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"type\":\"done\""));

        let parsed: ProviderStreamChunk = serde_json::from_str(&json).unwrap();
        match parsed {
            ProviderStreamChunk::Done { usage } => {
                assert!(usage.is_some());
                assert_eq!(usage.unwrap().total_tokens, 75);
            }
            _ => panic!("Expected Done variant"),
        }
    }

    #[test]
    fn test_provider_stream_chunk_done_without_usage() {
        let chunk = ProviderStreamChunk::Done { usage: None };

        let json = serde_json::to_string(&chunk).unwrap();
        let parsed: ProviderStreamChunk = serde_json::from_str(&json).unwrap();

        match parsed {
            ProviderStreamChunk::Done { usage } => {
                assert!(usage.is_none());
            }
            _ => panic!("Expected Done variant"),
        }
    }

    #[test]
    fn test_provider_stream_chunk_error() {
        let chunk = ProviderStreamChunk::Error {
            message: "Rate limit exceeded".to_string(),
        };

        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"type\":\"error\""));
        assert!(json.contains("Rate limit exceeded"));

        let parsed: ProviderStreamChunk = serde_json::from_str(&json).unwrap();
        match parsed {
            ProviderStreamChunk::Error { message } => {
                assert_eq!(message, "Rate limit exceeded");
            }
            _ => panic!("Expected Error variant"),
        }
    }

    #[test]
    fn test_provider_model_info_serde() {
        let info = ProviderModelInfo {
            id: "gpt-4-turbo".to_string(),
            display_name: "GPT-4 Turbo".to_string(),
            context_window: Some(128000),
            supports_tools: true,
            supports_vision: true,
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: ProviderModelInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "gpt-4-turbo");
        assert_eq!(parsed.display_name, "GPT-4 Turbo");
        assert_eq!(parsed.context_window, Some(128000));
        assert!(parsed.supports_tools);
        assert!(parsed.supports_vision);
    }

    #[test]
    fn test_provider_model_info_minimal() {
        let info = ProviderModelInfo {
            id: "llama-7b".to_string(),
            display_name: "Llama 7B".to_string(),
            context_window: None,
            supports_tools: false,
            supports_vision: false,
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: ProviderModelInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "llama-7b");
        assert_eq!(parsed.display_name, "Llama 7B");
        assert!(parsed.context_window.is_none());
        assert!(!parsed.supports_tools);
        assert!(!parsed.supports_vision);
    }

    // =========================================================================
    // HTTP Route Types Tests
    // =========================================================================

    #[test]
    fn test_http_request_serde() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());

        let mut query = HashMap::new();
        query.insert("page".to_string(), "1".to_string());
        query.insert("limit".to_string(), "10".to_string());

        let mut path_params = HashMap::new();
        path_params.insert("id".to_string(), "42".to_string());

        let req = HttpRequest {
            method: "POST".to_string(),
            path: "/api/users/42".to_string(),
            headers,
            query,
            body: Some(serde_json::json!({"name": "Alice", "email": "alice@example.com"})),
            path_params,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: HttpRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.path, "/api/users/42");
        assert_eq!(parsed.headers.get("Content-Type"), Some(&"application/json".to_string()));
        assert_eq!(parsed.query.get("page"), Some(&"1".to_string()));
        assert_eq!(parsed.path_params.get("id"), Some(&"42".to_string()));
        assert!(parsed.body.is_some());
    }

    #[test]
    fn test_http_request_minimal() {
        let req = HttpRequest {
            method: "GET".to_string(),
            path: "/health".to_string(),
            headers: HashMap::new(),
            query: HashMap::new(),
            body: None,
            path_params: HashMap::new(),
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: HttpRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.path, "/health");
        assert!(parsed.headers.is_empty());
        assert!(parsed.query.is_empty());
        assert!(parsed.body.is_none());
        assert!(parsed.path_params.is_empty());
    }

    #[test]
    fn test_http_response_ok() {
        let resp = HttpResponse::ok();
        assert_eq!(resp.status, 200);
        assert!(resp.headers.is_empty());
        assert!(resp.body.is_none());
    }

    #[test]
    fn test_http_response_json() {
        let data = serde_json::json!({"id": 1, "name": "Test"});
        let resp = HttpResponse::json(data.clone());

        assert_eq!(resp.status, 200);
        assert_eq!(resp.headers.get("Content-Type"), Some(&"application/json".to_string()));
        assert_eq!(resp.body, Some(data));
    }

    #[test]
    fn test_http_response_error() {
        let resp = HttpResponse::error(403, "Access denied");

        assert_eq!(resp.status, 403);
        assert!(resp.body.is_some());
        let body = resp.body.unwrap();
        assert_eq!(body.get("error"), Some(&serde_json::json!("Access denied")));
    }

    #[test]
    fn test_http_response_not_found() {
        let resp = HttpResponse::not_found();

        assert_eq!(resp.status, 404);
        assert!(resp.body.is_some());
        let body = resp.body.unwrap();
        assert_eq!(body.get("error"), Some(&serde_json::json!("Not Found")));
    }

    #[test]
    fn test_http_response_bad_request() {
        let resp = HttpResponse::bad_request("Invalid input");

        assert_eq!(resp.status, 400);
        assert!(resp.body.is_some());
        let body = resp.body.unwrap();
        assert_eq!(body.get("error"), Some(&serde_json::json!("Invalid input")));
    }

    #[test]
    fn test_http_response_internal_error() {
        let resp = HttpResponse::internal_error("Database connection failed");

        assert_eq!(resp.status, 500);
        assert!(resp.body.is_some());
        let body = resp.body.unwrap();
        assert_eq!(body.get("error"), Some(&serde_json::json!("Database connection failed")));
    }

    #[test]
    fn test_http_response_serde() {
        let mut headers = HashMap::new();
        headers.insert("X-Custom-Header".to_string(), "custom-value".to_string());

        let resp = HttpResponse {
            status: 201,
            headers,
            body: Some(serde_json::json!({"created": true})),
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: HttpResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.status, 201);
        assert_eq!(parsed.headers.get("X-Custom-Header"), Some(&"custom-value".to_string()));
        assert!(parsed.body.is_some());
    }

    #[test]
    fn test_http_response_from_json_string() {
        // Test parsing from a JSON string (as would come from a plugin)
        let json = r#"{"status": 200, "headers": {"Content-Type": "text/plain"}, "body": "Hello, World!"}"#;
        let parsed: HttpResponse = serde_json::from_str(json).unwrap();

        assert_eq!(parsed.status, 200);
        assert_eq!(parsed.headers.get("Content-Type"), Some(&"text/plain".to_string()));
        assert_eq!(parsed.body, Some(serde_json::json!("Hello, World!")));
    }
}
