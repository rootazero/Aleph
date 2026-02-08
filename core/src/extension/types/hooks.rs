//! Hook and MCP configuration types
//!
//! Types for shell-based hooks, plugin hooks, and MCP server configurations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
