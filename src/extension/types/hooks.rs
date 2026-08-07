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
/// It uses **`snake_case`** serialization for JSON-RPC IPC with plugins,
/// with `PascalCase` aliases for backward compatibility with hooks.json files.
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
    /// When a sub-agent is spawned and begins execution. Observer-only: the
    /// child has already launched, so blocking here is meaningless. Lets hooks
    /// witness delegation fan-out (notify, audit, budget tracking).
    #[serde(alias = "SubagentStart")]
    SubagentStart,
    /// When a sub-agent completes (success, timeout, or error). Observer-only.
    /// Carries outcome, iterations, duration, tokens, and key findings so hooks
    /// can react to delegation results without re-reading the transcript store.
    #[serde(alias = "SubagentStop")]
    SubagentStop,
    /// When the agent loop is about to stop (the model produced a turn with no
    /// tool calls). Claude-Code `Stop` hook parity: an interceptor-kind hook
    /// may veto the stop (`block:` / `{"decision":"block"}` → the loop
    /// continues with the reason as feedback) or halt permanently
    /// (`prevent_continuation` / `{"continue":false}`). Evaluated through the
    /// verifier chain (`ExtensionStopHookVerifier`), NOT inside `src/harness/`
    /// (R10) — the same seam config-TOML `[[stop_hooks]]` already uses.
    #[serde(alias = "Stop")]
    Stop,
}

impl HookEvent {
    /// Every hook event, in rough lifecycle order.
    ///
    /// Single source for "what events exist" — the `hooks.events` RPC and the
    /// `hooks_manage` tool's catalogue both project this instead of keeping
    /// their own hand-written lists (which is how a new variant ends up
    /// invisible to one surface and not the other).
    pub const ALL: [Self; 23] = [
        Self::BeforeAgentStart,
        Self::AgentEnd,
        Self::BeforeToolCall,
        Self::AfterToolCall,
        Self::AfterToolCallFailure,
        Self::ToolResultPersist,
        Self::MessageReceived,
        Self::MessageSending,
        Self::MessageSent,
        Self::SessionStart,
        Self::SessionEnd,
        Self::BeforeCompaction,
        Self::AfterCompaction,
        Self::PreApiRequest,
        Self::PostApiRequest,
        Self::GatewayStart,
        Self::GatewayStop,
        Self::Notification,
        Self::PermissionRequest,
        Self::UserPromptSubmit,
        Self::SubagentStart,
        Self::SubagentStop,
        Self::Stop,
    ];

    /// Whether this event's [`HookContext`](crate::extension::hooks::HookContext)
    /// carries a `tool_name`, so a `matcher` regex can meaningfully select
    /// among invocations.
    ///
    /// The executor matches `matcher` against `tool_name` ONLY. On any other
    /// event the matcher can never match and the hook silently never fires —
    /// a foot-gun surfaced at load time (`user_settings.rs`) and reported per
    /// hook by the runtime inventory (`HookExecutor::inventory`). Keeping the
    /// predicate on the event itself is what stops those two from drifting.
    #[must_use]
    pub const fn supports_matcher(self) -> bool {
        use HookEvent::*;
        matches!(
            self,
            BeforeToolCall
                | AfterToolCall
                | AfterToolCallFailure
                | ToolResultPersist
                | PermissionRequest
                | Notification
        )
    }

    /// Whether this event's production fire-site dispatches interceptor-kind
    /// hooks.
    ///
    /// The global fire-and-forget seams (`fire_global_observer`: message /
    /// provider / gateway / subagent / permission events) run observers only,
    /// so an `"kind": "interceptor"` registered there never executes.
    #[must_use]
    pub const fn supports_interceptor(self) -> bool {
        use HookEvent::*;
        matches!(
            self,
            BeforeToolCall
                | AfterToolCall
                | AfterToolCallFailure
                | BeforeAgentStart
                | UserPromptSubmit
                | SessionStart
                | AgentEnd
                | BeforeCompaction
                | Stop
        )
    }
}

/// Hook execution kind - determines how the hook is executed
///
/// (A third `Resolver` kind — first-win competition — existed on paper but
/// never gained a production fire-site; it was removed under YAGNI. Configs
/// that still say `"kind": "resolver"` parse to the `Observer` default.)
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
}

impl HookKind {
    /// Parse from string with fallback to Observer
    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::Observer)
    }
}

impl std::str::FromStr for HookKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "observer" => Ok(Self::Observer),
            "interceptor" => Ok(Self::Interceptor),
            _ => Err(format!("Unknown hook kind: {s}")),
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
    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::Normal)
    }

    #[must_use]
    pub const fn as_i32(&self) -> i32 {
        match self {
            Self::System => -1000,
            Self::High => -100,
            Self::Normal => 0,
            Self::Low => 100,
        }
    }
}

impl std::str::FromStr for HookPriority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "system" => Ok(Self::System),
            "high" => Ok(Self::High),
            "normal" => Ok(Self::Normal),
            "low" => Ok(Self::Low),
            _ => Err(format!("Unknown hook priority: {s}")),
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
    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::System)
    }
}

impl std::str::FromStr for PromptScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "system" => Ok(Self::System),
            "tool" => Ok(Self::Tool),
            "standalone" => Ok(Self::Standalone),
            "disabled" => Ok(Self::Disabled),
            _ => Err(format!("Unknown prompt scope: {s}")),
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
    /// Request delegation to a named subagent. The executor never runs the
    /// agent inline — it emits an `additional_contexts` directive asking the
    /// calling LLM to invoke the `subagent` tool with this agent name.
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
    /// Invoke a runtime plugin's exported hook handler (WASM). Carries the
    /// plugin id + export name; the executor resolves the process-global
    /// `ExtensionManager` and calls `execute_plugin_hook`, so an event-driven
    /// plugin hook actually fires. This is what `sync_hooks_from_registry`
    /// emits for `HookRegistration` entries — previously they were synced with
    /// an empty `actions` list, so the handler name lived on the `HookConfig`
    /// but was never dispatched (a registered plugin hook silently no-op'd).
    /// Mechanical dispatch only — no reasoning (R7/R10).
    Plugin { plugin_id: String, handler: String },
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

/// MCP server configuration.
///
/// Aleph supports two transports:
/// - [`McpServerConfig::Stdio`] — a local child process speaking MCP over
///   stdio. The original and most common shape.
/// - [`McpServerConfig::Remote`] — an HTTP/SSE endpoint that speaks MCP's
///   Streamable HTTP / SSE wire format. Lets a plugin connect to a hosted MCP
///   server without spawning a child process.
///
/// The `#[serde(tag = "type")]` shape means manifests / configs declare the
/// transport explicitly: `{ "type": "stdio", "command": [...] }` or
/// `{ "type": "remote", "url": "https://mcp.example.com/api" }`. A bare
/// `{ "command": "npx", "args": [...] }` object (no `type`) deserialises as
/// the default [`McpServerConfig::Stdio`] variant for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpServerConfig {
    /// Local stdio MCP server (default — used when the `type` field is absent
    /// or set to `stdio`).
    #[serde(alias = "stdio")]
    Stdio {
        /// Command to execute
        command: String,
        /// Command arguments
        #[serde(default)]
        args: Vec<String>,
        /// Environment variables
        #[serde(default)]
        env: HashMap<String, String>,
    },
    /// Remote MCP server (HTTP/SSE transport).
    Remote {
        /// Server URL
        url: String,
        /// Custom request headers
        #[serde(default)]
        headers: HashMap<String, String>,
        /// OAuth credentials for the remote server
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<crate::extension::config::OAuthConfig>,
        /// Per-request timeout in milliseconds
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
}

impl McpServerConfig {
    /// Whether this server is a stdio transport (the original shape, which
    /// PluginLoader spawns as a child process).
    #[must_use]
    pub const fn is_stdio(&self) -> bool {
        matches!(self, Self::Stdio { .. })
    }

    /// Whether this server is a remote HTTP/SSE transport.
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// Stdio-only accessor. Returns `None` for [`Self::Remote`] so callers
    /// that assume a child process can be told "no process, this is HTTP".
    #[must_use]
    pub fn stdio_command(&self) -> Option<(&str, &[String], &HashMap<String, String>)> {
        match self {
            Self::Stdio { command, args, env } => Some((command, args, env)),
            Self::Remote { .. } => None,
        }
    }

    /// Remote-only accessor. Returns `None` for [`Self::Stdio`].
    #[must_use]
    pub fn remote_url(&self) -> Option<&str> {
        match self {
            Self::Remote { url, .. } => Some(url),
            Self::Stdio { .. } => None,
        }
    }
}
