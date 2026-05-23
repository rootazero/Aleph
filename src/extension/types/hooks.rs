//! Hook and MCP configuration types
//!
//! Types for shell-based hooks, plugin hooks, and MCP server configurations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// =============================================================================
// Hook Types
// =============================================================================

/// Unified hook event types for both shell-based hooks and plugin hooks.
///
/// This enum is the single source of truth for all hook events in Aleph.
/// It uses **snake_case** serialization for JSON-RPC IPC with plugins,
/// with PascalCase aliases for backward compatibility with hooks.json files.
///
/// # Example (hooks config in CLAUDE.md)
/// ```json
/// {
///   "hooks": {
///     "PreToolUse": [{ "command": "my-hook.sh" }],
///     "before_tool_call": [{ "command": "my-hook.sh" }]
///   }
/// }
/// ```
///
/// # Example (plugin registration via JSON-RPC)
/// ```json
/// {
///   "hooks": [
///     { "event": "before_tool_call", "handler": "onBeforeToolCall", "priority": 0 }
///   ]
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Before agent starts processing
    #[serde(alias = "BeforeAgentStart")]
    BeforeAgentStart,
    /// After agent completes processing
    #[serde(alias = "AgentEnd")]
    AgentEnd,
    /// Before a tool is called
    #[serde(alias = "PreToolUse", alias = "BeforeToolCall")]
    BeforeToolCall,
    /// After a tool call completes
    #[serde(alias = "PostToolUse", alias = "AfterToolCall")]
    AfterToolCall,
    /// After a tool call fails
    #[serde(alias = "PostToolUseFailure", alias = "AfterToolCallFailure")]
    AfterToolCallFailure,
    /// When tool result is being persisted
    #[serde(alias = "ToolResultPersist")]
    ToolResultPersist,
    /// When a message is received from a channel
    #[serde(alias = "MessageReceived")]
    MessageReceived,
    /// Before a message is sent to a channel
    #[serde(alias = "MessageSending")]
    MessageSending,
    /// After a message has been sent
    #[serde(alias = "MessageSent")]
    MessageSent,
    /// When a session starts
    #[serde(alias = "SessionStart")]
    SessionStart,
    /// When a session ends
    #[serde(alias = "SessionEnd")]
    SessionEnd,
    /// Before session compaction
    #[serde(alias = "PreCompact", alias = "BeforeCompaction")]
    BeforeCompaction,
    /// After session compaction
    #[serde(alias = "AfterCompaction")]
    AfterCompaction,
    /// Before an LLM provider API request is issued
    #[serde(alias = "PreApiRequest")]
    PreApiRequest,
    /// After an LLM provider API response is received
    #[serde(alias = "PostApiRequest")]
    PostApiRequest,
    /// When gateway starts
    #[serde(alias = "GatewayStart")]
    GatewayStart,
    /// When gateway stops
    #[serde(alias = "GatewayStop")]
    GatewayStop,
    /// When a notification is sent
    #[serde(alias = "Notification")]
    Notification,
    /// When a permission is requested
    #[serde(alias = "PermissionRequest")]
    PermissionRequest,
    /// When a user prompt is about to be sent to the LLM (after history
    /// build, before the first think call). Lets hooks inject context or
    /// abort the run before any provider call.
    #[serde(alias = "UserPromptSubmit")]
    UserPromptSubmit,
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
    /// Parse from string with fallback to Observer
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(HookKind::Observer)
    }
}

impl std::str::FromStr for HookKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "observer" => Ok(HookKind::Observer),
            "interceptor" => Ok(HookKind::Interceptor),
            "resolver" => Ok(HookKind::Resolver),
            _ => Err(format!("Unknown hook kind: {}", s)),
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
    /// Parse from string with fallback to Normal
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(HookPriority::Normal)
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

impl std::str::FromStr for HookPriority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "system" => Ok(HookPriority::System),
            "high" => Ok(HookPriority::High),
            "normal" => Ok(HookPriority::Normal),
            "low" => Ok(HookPriority::Low),
            _ => Err(format!("Unknown hook priority: {}", s)),
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
    /// Parse from string with fallback to System
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(PromptScope::System)
    }
}

impl std::str::FromStr for PromptScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "system" => Ok(PromptScope::System),
            "tool" => Ok(PromptScope::Tool),
            "standalone" => Ok(PromptScope::Standalone),
            "disabled" => Ok(PromptScope::Disabled),
            _ => Err(format!("Unknown prompt scope: {}", s)),
        }
    }
}

/// Hook action types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookAction {
    /// Execute a shell command. Event JSON is piped to stdin in addition to
    /// being exposed via env vars; stdout is parsed using the line-prefix
    /// protocol (see [`crate::extension::hooks::parse_command_output`]).
    Command { command: String },
    /// Provide a prompt template. The resolved prompt is injected as
    /// `additional_context` for the next LLM turn (no separate LLM call).
    Prompt { prompt: String },
    /// Invoke a named subagent. (Lookup at the call site; no inline run.)
    Agent { agent: String },
    /// POST the event JSON to a URL. The response body is parsed using the
    /// same line-prefix protocol as Command, so HTTP hooks can return
    /// `block:` / `deny:` / `context:` directives.
    Http {
        url: String,
        /// Optional HTTP headers. `${VAR}` placeholders are substituted from
        /// the hook context env (no system-env interpolation — to prevent
        /// secret exfiltration via misconfigured templates).
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        headers: HashMap<String, String>,
    },
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

    /// Per-hook timeout in seconds. Overrides the executor's default
    /// timeout when set. Applies to Command and Http actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
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
