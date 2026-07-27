//! Hook execution system
//!
//! Handles event-driven hooks for tool lifecycle, session events, etc.
//!
//! # Hook Events
//!
//! The canonical, exhaustive list lives on the `HookEvent` enum
//! (`crate::extension::types::hooks`). Major groups:
//!
//! - Tool: `BeforeToolCall` / `AfterToolCall` / `AfterToolCallFailure` / `ToolResultPersist`
//! - Agent: `BeforeAgentStart` / `AgentEnd` / `UserPromptSubmit` / `Stop`
//! - Session: `SessionStart` / `SessionEnd`
//! - Subagent: `SubagentStart` / `SubagentStop`
//! - Message: `MessageReceived` / `MessageSending` / `MessageSent`
//! - Compaction: `BeforeCompaction` / `AfterCompaction`
//! - Provider: `PreApiRequest` / `PostApiRequest`
//! - Approval: `PermissionRequest` / `Notification`
//! - Gateway: `GatewayStart` / `GatewayStop`
//!
//! # Command-hook output contract
//!
//! A `command`-type hook signals decisions via stdout in one of two ways:
//!
//! 1. **Line-prefix protocol** (Aleph-native) — see [`parse_command_output`].
//! 2. **JSON decision object** (Claude-Code / hermes interop) — when the entire
//!    stdout is a JSON object it is decoded by the `json_output` module and
//!    mapped onto the same [`HookResult`] fields. Non-JSON output falls back to
//!    (1), so the two contracts coexist without ambiguity.
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::extension::hooks::{HookExecutor, HookContext};
//!
//! let executor = HookExecutor::new(hooks);
//!
//! // Interceptor-kind hooks (sequential, can block / rewrite):
//! let (ctx, result) = executor
//!     .execute_interceptors(HookEvent::BeforeToolCall, context)
//!     .await?;
//! if result.blocked {
//!     return Err("Tool blocked by hook");
//! }
//!
//! // Observer-kind hooks (parallel, fire-and-forget):
//! executor.execute_observers(HookEvent::AfterToolCall, &ctx).await;
//! ```

mod consent;
mod executor;
mod json_output;
mod output_budget;
mod user_settings;

pub use consent::{ConsentEntry, ConsentStatus, ShellHookConsent};
pub(crate) use executor::read_capped;
pub use executor::{event_payload_json, HookExecutor};
pub use output_budget::{budget_hook_contexts, join_messages};
pub(crate) use user_settings::default_kind_for_event;
pub use user_settings::load_user_hooks;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default timeout for command execution (300 seconds)
const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 300;

/// Hard ceiling on any per-hook `timeout_secs` override.
///
/// `timeout_secs` arrives from three untrusted-ish sources (`hooks.json`,
/// a plugin's `hooks.json`, `aleph.plugin.toml`) and used to be honoured
/// verbatim. An interceptor seam AWAITS its hooks, so a hook declaring
/// `timeout_secs: 86400` wedges the tool-dispatch (or stop) gate for a day —
/// well past the 180s per-tool budget the rest of the system assumes. The
/// clamp is applied at the single chokepoint that turns the override into a
/// `Duration` ([`HookExecutor::effective_timeout`]), so every source is
/// covered without touching any loader. hermes clamps identically
/// (`MAX_TIMEOUT_SECONDS = 300`).
pub(crate) const MAX_HOOK_TIMEOUT_SECS: u64 = 300;

/// Hook execution context
#[derive(Debug, Clone, Default)]
pub struct HookContext {
    /// Session ID
    pub session_id: String,
    /// Tool name (for tool events)
    pub tool_name: Option<String>,
    /// Tool arguments (JSON string)
    pub arguments: Option<String>,
    /// Tool input content
    pub tool_input: Option<String>,
    /// File path (if applicable)
    pub file_path: Option<PathBuf>,
    /// Working directory for commands
    pub working_dir: Option<PathBuf>,
    /// Additional environment variables
    pub env: HashMap<String, String>,
    /// Tool execution output (only set for AfterToolCall/AfterToolCallFailure hooks)
    pub tool_output: Option<String>,
    /// Whether the tool execution resulted in an error
    pub tool_error: Option<bool>,
}

impl HookContext {
    /// Create a new hook context
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            ..Default::default()
        }
    }

    /// Set the tool name
    pub fn with_tool_name(mut self, name: impl Into<String>) -> Self {
        self.tool_name = Some(name.into());
        self
    }

    /// Set the arguments
    pub fn with_arguments(mut self, args: impl Into<String>) -> Self {
        self.arguments = Some(args.into());
        self
    }

    /// Set the tool input
    pub fn with_tool_input(mut self, input: impl Into<String>) -> Self {
        self.tool_input = Some(input.into());
        self
    }

    /// Set the file path
    pub fn with_file_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    /// Set the working directory
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Add an environment variable
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set the tool output
    pub fn with_tool_output(mut self, output: impl Into<String>) -> Self {
        self.tool_output = Some(output.into());
        self
    }

    /// Set whether the tool errored
    #[must_use]
    pub const fn with_tool_error(mut self, is_error: bool) -> Self {
        self.tool_error = Some(is_error);
        self
    }
}

/// One registered hook as the running server actually sees it.
///
/// This is the **runtime** view, deliberately distinct from the `hooks.list`
/// RPC's **file** view of `~/.aleph/hooks.json`. The file view cannot answer
/// the questions that actually go wrong in practice:
///
/// - project-scoped and plugin-shipped hooks never appear in it at all;
/// - `kind` is usually omitted in config and resolved per-event at load time,
///   so the file does not say what a hook will actually do;
/// - the two silent-death foot-guns (a `matcher` on an event with no tool
///   name; `kind: interceptor` on an observer-only seam) were previously
///   reported ONLY as a `warn!` line at boot — invisible to anyone debugging
///   "why doesn't my hook fire?" hours later.
///
/// [`reachable`](Self::reachable) answers exactly that question, and
/// [`issue`](Self::issue) says why not.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HookInventoryEntry {
    /// Registration source: `user:global`, `user:project`, `user:project-local`,
    /// or a plugin name.
    pub source: String,
    /// Canonical (snake_case) event name.
    pub event: String,
    /// Resolved kind — `interceptor` or `observer` — after per-event defaults.
    pub kind: String,
    /// Priority bucket; interceptors run in ascending order.
    pub priority: String,
    /// Tool-name regex, when set.
    pub matcher: Option<String>,
    /// One label per action, e.g. `command: ./lint.sh` / `http: https://…`.
    pub actions: Vec<String>,
    /// Effective per-hook timeout override, if declared.
    pub timeout_secs: Option<u64>,
    /// For project-scoped hooks, the project this hook is bound to. Such a
    /// hook only fires while the agent works inside that folder.
    pub project_root: Option<String>,
    /// Whether this hook can fire at all with its current configuration.
    pub reachable: bool,
    /// Why it cannot fire (or a caveat worth surfacing), when applicable.
    pub issue: Option<String>,
    /// Consent state for hooks whose actions need operator approval
    /// (`command` / `http`): `approved`, `pending`, or `None` when the hook
    /// has no gated action or no consent gate is attached.
    pub consent: Option<String>,
}

/// Result of a single hook action
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// Whether the action succeeded
    pub success: bool,
    /// Output from the action
    pub output: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
    /// Exit code (for command actions)
    pub exit_code: Option<i32>,
}

impl Default for ActionResult {
    fn default() -> Self {
        Self {
            success: true,
            output: None,
            error: None,
            exit_code: None,
        }
    }
}

/// Hook-emitted permission decision for tool execution.
///
/// Follows the principle that hook `Allow` does NOT bypass settings-level
/// deny rules — it only waives the hook-level Ask escalation; the permission policy still applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Hook vouches for safety — proceed without hook-forced confirmation,
    /// but NOT settings-level deny rules.
    Allow,
    /// Force user confirmation before execution.
    Ask { reason: String },
    /// Temporary interception — retryable (maps to legacy `blocked`).
    Block { reason: String },
    /// Hard policy deny — not retryable (maps to legacy `denied`).
    Deny { reason: String },
}

/// Hook execution result (aggregated from all matching hooks)
#[derive(Debug, Default)]
pub struct HookResult {
    /// Whether the action was blocked (for `BeforeToolCall`)
    pub blocked: bool,
    /// Block reason (if blocked)
    pub block_reason: Option<String>,
    /// Plain stdout lines (no prefix). On the `UserPromptSubmit` /
    /// `SessionStart` seams these ARE injected as context (Claude-Code
    /// convention: plain stdout counts there); on `prevent_continuation`
    /// paths the first line doubles as the stop message. Elsewhere they stay
    /// diagnostic-only — `additional_contexts` is the general-purpose channel
    /// that reaches the LLM next turn.
    pub messages: Vec<String>,
    /// Names of agents requested by `agent`-type hooks. The executor never
    /// spawns agents inline (R10) — each entry is also rendered as an
    /// `additional_contexts` directive asking the calling LLM to delegate
    /// via the `subagent` tool. This list stays as the structured record
    /// for diagnostics and tests.
    pub agents_to_invoke: Vec<String>,
    /// Individual action results
    pub action_results: Vec<ActionResult>,
    /// Number of hooks executed
    pub hooks_executed: usize,
    /// Hook-modified tool input (JSON). Last writer wins across interceptor chain.
    pub updated_input: Option<serde_json::Value>,
    /// Additional context strings to inject into next LLM turn (as system-reminders).
    pub additional_contexts: Vec<String>,
    /// If true, the agent run stops gracefully (Claude-Code `continue: false`).
    /// Honored by the gateway run loop on the `BeforeAgentStart` and
    /// `UserPromptSubmit` lifecycle interceptor seams (`run_loop.rs`): the run
    /// returns the hook's message as its final output instead of proceeding.
    pub prevent_continuation: bool,
    /// If true, tool call is denied by policy (not retryable). Takes precedence over blocked.
    pub denied: bool,
    /// Reason for denial (if denied).
    pub deny_reason: Option<String>,
    /// Replacement for tool output text (last-writer-wins). Effective on the
    /// `AfterToolCall` / `AfterToolCallFailure` interceptor path and on the
    /// `AgentEnd` seam, where it rewrites the final assistant response
    /// (hermes `transform_llm_output` parity).
    pub updated_output: Option<String>,
    /// Hook-emitted permission decision. Last writer wins across interceptor chain.
    /// Supersedes legacy `blocked`/`denied` fields (preserved for backward compat).
    pub permission_decision: Option<PermissionDecision>,
    /// True when an interceptor ACTION failed at the infrastructure level
    /// (spawn error / timeout / truncated output) rather than by a deliberate
    /// hook decision. `blocked` is still set (the tool gate fails closed on
    /// broken hooks by policy), but seams with the OPPOSITE failure policy —
    /// the extension stop gate is fail-open, a broken script must not wedge
    /// the loop — read this flag to tell the two apart.
    pub action_failed: bool,
}

impl HookResult {
    /// Check if all actions succeeded
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.action_results.iter().all(|r| r.success)
    }

    /// Get all outputs from successful actions
    #[must_use]
    pub fn outputs(&self) -> Vec<&str> {
        self.action_results
            .iter()
            .filter(|r| r.success)
            .filter_map(|r| r.output.as_deref())
            .collect()
    }

    /// Get all errors from failed actions
    #[must_use]
    pub fn errors(&self) -> Vec<&str> {
        self.action_results
            .iter()
            .filter(|r| !r.success)
            .filter_map(|r| r.error.as_deref())
            .collect()
    }

    /// The human-facing message for a graceful stop (`prevent_continuation` /
    /// `continue: false`). Plain stdout (`messages`) wins over the JSON
    /// `stopReason` that rides in `additional_contexts`; `default` is used
    /// when a hook halted without saying why. Single source for the three
    /// lifecycle seams that honour `prevent_continuation` (BeforeAgentStart,
    /// UserPromptSubmit, the extension stop gate).
    #[must_use]
    pub fn stop_message(&self, default: &str) -> String {
        self.messages
            .first()
            .or_else(|| self.additional_contexts.first())
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }
}

/// Parse structured output from a command hook.
///
/// Two contracts are supported, tried in order:
///
/// 1. **JSON decision object** — if the whole (trimmed) output is a JSON
///    object it is decoded as a Claude-Code / hermes decision and mapped onto
///    [`HookResult`] (see `json_output`). This makes hooks written for the
///    wider Claude-Code ecosystem work unchanged.
/// 2. **Line-prefix protocol** (Aleph-native fallback) — each line parsed
///    independently:
///    - `block: <reason>` — block the tool call (retryable)
///    - `deny: <reason>` — deny the tool call (not retryable)
///    - `allow` — proceed without hook-forced confirmation
///    - `ask: <reason>` — force user confirmation before execution
///    - `update_input: <json>` — replace tool input arguments
///    - `update_output: <text>` — replace tool output text
///    - `context: <text>` — inject additional context for LLM
///    - `prevent_continuation` — stop the agent loop
///    - (no prefix) — treat as a message
pub fn parse_command_output(output: &str, result: &mut HookResult) {
    // JSON decision object takes precedence; non-object output falls through.
    if json_output::apply_json_decision(output, result) {
        return;
    }

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(reason) = trimmed.strip_prefix("block:") {
            let reason = reason.trim().to_string();
            result.blocked = true;
            result.block_reason = Some(reason.clone());
            result.permission_decision = Some(PermissionDecision::Block { reason });
        } else if let Some(reason) = trimmed.strip_prefix("deny:") {
            let reason = reason.trim().to_string();
            result.denied = true;
            result.deny_reason = Some(reason.clone());
            result.permission_decision = Some(PermissionDecision::Deny { reason });
        } else if trimmed == "allow" {
            // Clear blocked/denied flags since the final decision is Allow
            result.blocked = false;
            result.block_reason = None;
            result.denied = false;
            result.deny_reason = None;
            result.permission_decision = Some(PermissionDecision::Allow);
        } else if let Some(reason) = trimmed.strip_prefix("ask:") {
            result.permission_decision = Some(PermissionDecision::Ask {
                reason: reason.trim().to_string(),
            });
        } else if let Some(json_str) = trimmed.strip_prefix("update_input:") {
            match serde_json::from_str(json_str.trim()) {
                Ok(val) => result.updated_input = Some(val),
                Err(e) => {
                    tracing::warn!("Hook update_input invalid JSON: {}", e);
                }
            }
        } else if let Some(output_text) = trimmed.strip_prefix("update_output:") {
            result.updated_output = Some(output_text.trim().to_string());
        } else if let Some(ctx) = trimmed.strip_prefix("context:") {
            result.additional_contexts.push(ctx.trim().to_string());
        } else if trimmed == "prevent_continuation" {
            result.prevent_continuation = true;
        } else {
            result.messages.push(trimmed.to_string());
        }
    }
}

/// Substitute variables in a string
///
/// Supported variables:
/// - `${PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_ROOT}` - Plugin root directory
/// - `$ARGUMENTS` / `${ARGUMENTS}` - Tool arguments (JSON)
/// - `$TOOL_INPUT` / `${TOOL_INPUT}` - Tool input content
/// - `$FILE` / `${FILE}` - File path
/// - `$TOOL_NAME` / `${TOOL_NAME}` - Tool name
/// - `$SESSION_ID` / `${SESSION_ID}` - Session ID
#[must_use]
pub fn substitute_variables(template: &str, context: &HookContext, plugin_root: &Path) -> String {
    let mut result = template.to_string();
    let plugin_root_str = plugin_root.to_string_lossy();

    // Plugin root (both formats)
    result = result.replace("${PLUGIN_ROOT}", &plugin_root_str);
    result = result.replace("${CLAUDE_PLUGIN_ROOT}", &plugin_root_str);

    // Tool name
    if let Some(ref name) = context.tool_name {
        result = result.replace("$TOOL_NAME", name);
        result = result.replace("${TOOL_NAME}", name);
    }

    // Arguments
    if let Some(ref args) = context.arguments {
        result = result.replace("$ARGUMENTS", args);
        result = result.replace("${ARGUMENTS}", args);
    }

    // Tool input
    if let Some(ref input) = context.tool_input {
        result = result.replace("$TOOL_INPUT", input);
        result = result.replace("${TOOL_INPUT}", input);
    }

    // File path
    if let Some(ref file) = context.file_path {
        let file_str = file.to_string_lossy();
        result = result.replace("$FILE", &file_str);
        result = result.replace("${FILE}", &file_str);
    }

    // Session ID
    result = result.replace("$SESSION_ID", &context.session_id);
    result = result.replace("${SESSION_ID}", &context.session_id);

    // Custom environment variables
    for (key, value) in &context.env {
        result = result.replace(&format!("${key}"), value);
        result = result.replace(&format!("${{{key}}}"), value);
    }

    result
}

/// Fire an observer hook against an explicit executor.
///
/// Builds a [`HookContext`] from `session_id` + `env` and dispatches the event
/// to every matching observer. A no-op when the executor has no hooks.
async fn fire_observer(
    executor: &HookExecutor,
    event: crate::extension::HookEvent,
    session_id: &str,
    env: Vec<(&'static str, String)>,
) {
    if executor.hook_count() == 0 {
        return;
    }
    let mut ctx = HookContext::new(session_id);
    for (key, value) in env {
        // A TOOL_NAME env entry also populates the structured field the
        // matcher regex tests against — without this hop, a `matcher` on
        // PermissionRequest / Notification hooks could never fire (the
        // fire-sites pass the tool name as env only).
        if key == "TOOL_NAME" {
            ctx = ctx.with_tool_name(value.clone());
        }
        ctx = ctx.with_env(key, value);
    }
    executor.execute_observers(event, &ctx).await;
}

/// Fire an observer hook against the process-global extension manager.
///
/// For fire-sites that do not already hold a per-run [`HookExecutor`] — gateway
/// lifecycle, channel I/O, provider calls. Best-effort and fire-and-forget: a
/// silent no-op when the manager is unregistered or carries no hooks, so hot
/// paths can call it unconditionally.
pub async fn fire_global_observer(
    event: crate::extension::HookEvent,
    session_id: &str,
    env: Vec<(&'static str, String)>,
) {
    let Some(manager) = crate::extension::try_extension_manager() else {
        return;
    };
    let executor = manager.hook_executor_snapshot().await;
    fire_observer(&executor, event, session_id, env).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
    use crate::extension::HookPriority;

    #[test]
    fn test_substitute_variables() {
        let context = HookContext {
            session_id: "test-session".to_string(),
            tool_name: Some("Write".to_string()),
            arguments: Some(r#"{"path": "/test.txt"}"#.to_string()),
            tool_input: Some("file content".to_string()),
            file_path: Some(PathBuf::from("/path/to/file.txt")),
            working_dir: None,
            env: HashMap::new(),
            tool_output: None,
            tool_error: None,
        };

        let plugin_root = PathBuf::from("/plugins/my-plugin");

        let result = substitute_variables(
            "Run ${PLUGIN_ROOT}/script.sh with $ARGUMENTS on $FILE for $TOOL_NAME",
            &context,
            &plugin_root,
        );

        assert!(result.contains("/plugins/my-plugin/script.sh"));
        assert!(result.contains(r#"{"path": "/test.txt"}"#));
        assert!(result.contains("/path/to/file.txt"));
        assert!(result.contains("Write"));
    }

    #[test]
    fn test_substitute_variables_braced() {
        let context = HookContext::new("session-1")
            .with_tool_name("Read")
            .with_arguments("test args");

        let plugin_root = PathBuf::from("/plugin");

        let result = substitute_variables(
            "${TOOL_NAME}: ${ARGUMENTS} (${SESSION_ID})",
            &context,
            &plugin_root,
        );

        assert_eq!(result, "Read: test args (session-1)");
    }

    #[test]
    fn test_substitute_variables_custom_env() {
        let context = HookContext::new("session")
            .with_env("CUSTOM_VAR", "custom_value")
            .with_env("ANOTHER", "another_value");

        let plugin_root = PathBuf::from("/plugin");

        let result = substitute_variables("$CUSTOM_VAR and ${ANOTHER}", &context, &plugin_root);

        assert_eq!(result, "custom_value and another_value");
    }

    #[test]
    fn test_hook_context_builder() {
        let context = HookContext::new("session-123")
            .with_tool_name("Bash")
            .with_arguments(r#"{"command": "ls"}"#)
            .with_file_path("/some/path")
            .with_working_dir("/work")
            .with_env("MY_VAR", "my_value");

        assert_eq!(context.session_id, "session-123");
        assert_eq!(context.tool_name, Some("Bash".to_string()));
        assert_eq!(context.arguments, Some(r#"{"command": "ls"}"#.to_string()));
        assert_eq!(context.file_path, Some(PathBuf::from("/some/path")));
        assert_eq!(context.working_dir, Some(PathBuf::from("/work")));
        assert_eq!(context.env.get("MY_VAR"), Some(&"my_value".to_string()));
    }

    #[test]
    fn test_hook_result_new_fields_default() {
        let result = HookResult::default();
        assert!(result.updated_input.is_none());
        assert!(result.additional_contexts.is_empty());
        assert!(!result.prevent_continuation);
    }

    #[test]
    fn test_hook_context_with_tool_output() {
        let ctx = HookContext::new("session-1")
            .with_tool_name("Write")
            .with_tool_output("File written successfully")
            .with_tool_error(false);
        assert_eq!(
            ctx.tool_output,
            Some("File written successfully".to_string())
        );
        assert_eq!(ctx.tool_error, Some(false));
    }

    #[test]
    fn test_hook_result_helpers() {
        let mut result = HookResult::default();

        result.action_results.push(ActionResult {
            success: true,
            output: Some("output1".to_string()),
            error: None,
            exit_code: Some(0),
        });

        result.action_results.push(ActionResult {
            success: false,
            output: None,
            error: Some("error1".to_string()),
            exit_code: Some(1),
        });

        result.action_results.push(ActionResult {
            success: true,
            output: Some("output2".to_string()),
            error: None,
            exit_code: Some(0),
        });

        assert!(!result.all_succeeded());
        assert_eq!(result.outputs(), vec!["output1", "output2"]);
        assert_eq!(result.errors(), vec!["error1"]);
    }

    #[tokio::test]
    async fn test_hook_executor_empty() {
        let executor = HookExecutor::new(vec![]);
        let context = HookContext::new("test");

        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, context)
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 0);
        assert!(!result.blocked);
    }

    #[tokio::test]
    async fn test_hook_executor_with_prompt() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: Some("Write".to_string()),
            actions: vec![HookAction::Prompt {
                prompt: "Checking ${TOOL_NAME} operation".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/plugin"),
            handler: None,
            timeout_secs: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session").with_tool_name("Write");

        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, context)
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 1);
        // Prompt hook output now lands in additional_contexts so it actually
        // reaches the LLM as a system reminder for the next turn.
        assert_eq!(result.additional_contexts.len(), 1);
        assert_eq!(result.additional_contexts[0], "Checking Write operation");
    }

    #[tokio::test]
    async fn test_hook_executor_pattern_mismatch() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: Some("Write".to_string()),
            actions: vec![HookAction::Prompt {
                prompt: "test".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/plugin"),
            handler: None,
            timeout_secs: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session").with_tool_name("Read");

        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, context)
            .await
            .unwrap();

        // Pattern doesn't match, so no hooks executed
        assert_eq!(result.hooks_executed, 0);
    }

    #[tokio::test]
    async fn test_hook_executor_regex_pattern() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: Some("Write|Edit".to_string()),
            actions: vec![HookAction::Prompt {
                prompt: "Modifying file".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/plugin"),
            handler: None,
            timeout_secs: None,
        }];

        let executor = HookExecutor::new(hooks);

        // Test with Write
        let context = HookContext::new("session").with_tool_name("Write");
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, context)
            .await
            .unwrap();
        assert_eq!(result.hooks_executed, 1);

        // Test with Edit
        let context = HookContext::new("session").with_tool_name("Edit");
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, context)
            .await
            .unwrap();
        assert_eq!(result.hooks_executed, 1);

        // Test with Read (no match)
        let context = HookContext::new("session").with_tool_name("Read");
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, context)
            .await
            .unwrap();
        assert_eq!(result.hooks_executed, 0);
    }

    #[tokio::test]
    async fn test_hook_executor_with_agent() {
        let hooks = vec![HookConfig {
            event: HookEvent::AfterToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None, // Matches all
            actions: vec![HookAction::Agent {
                agent: "review-agent".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/plugin"),
            handler: None,
            timeout_secs: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session").with_tool_name("Write");

        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::AfterToolCall, context)
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 1);
        assert_eq!(result.agents_to_invoke, vec!["review-agent"]);
        // The delegation request must also reach the LLM via the existing
        // additional-context plumbing (the list alone has no consumer).
        assert_eq!(result.additional_contexts.len(), 1);
        assert!(result.additional_contexts[0].contains("review-agent"));
        assert!(result.additional_contexts[0].contains("subagent"));
    }

    #[test]
    fn test_parse_command_output_block() {
        let mut result = HookResult::default();
        parse_command_output("block: unauthorized access", &mut result);
        assert!(result.blocked);
        assert_eq!(result.block_reason, Some("unauthorized access".to_string()));
    }

    #[test]
    fn test_parse_command_output_update_input() {
        let mut result = HookResult::default();
        parse_command_output(r#"update_input: {"path": "/safe"}"#, &mut result);
        assert_eq!(
            result.updated_input,
            Some(serde_json::json!({"path": "/safe"}))
        );
    }

    #[test]
    fn test_parse_command_output_invalid_json_ignored() {
        let mut result = HookResult::default();
        parse_command_output("update_input: not json", &mut result);
        assert!(result.updated_input.is_none());
    }

    #[test]
    fn test_parse_command_output_context() {
        let mut result = HookResult::default();
        parse_command_output(
            "context: File auto-formatted\ncontext: Lint passed",
            &mut result,
        );
        assert_eq!(
            result.additional_contexts,
            vec!["File auto-formatted", "Lint passed"]
        );
    }

    #[test]
    fn test_parse_command_output_prevent_continuation() {
        let mut result = HookResult::default();
        parse_command_output("prevent_continuation", &mut result);
        assert!(result.prevent_continuation);
    }

    #[test]
    fn test_parse_command_output_plain_message() {
        let mut result = HookResult::default();
        parse_command_output("Hello from hook", &mut result);
        assert_eq!(result.messages, vec!["Hello from hook"]);
    }

    #[test]
    fn test_parse_command_output_mixed() {
        let mut result = HookResult::default();
        parse_command_output(
            "context: formatted\nHello\nblock: danger\n\nprevent_continuation",
            &mut result,
        );
        assert_eq!(result.additional_contexts, vec!["formatted"]);
        assert_eq!(result.messages, vec!["Hello"]);
        assert!(result.blocked);
        assert_eq!(result.block_reason, Some("danger".to_string()));
        assert!(result.prevent_continuation);
    }

    #[test]
    fn test_parse_command_output_deny() {
        let mut result = HookResult::default();
        parse_command_output("deny: policy violation", &mut result);
        assert!(result.denied);
        assert_eq!(result.deny_reason, Some("policy violation".to_string()));
        assert!(!result.blocked);
    }

    #[test]
    fn test_parse_command_output_deny_and_block_coexist() {
        let mut result = HookResult::default();
        parse_command_output("block: temp issue\ndeny: permanent ban", &mut result);
        assert!(result.denied);
        assert_eq!(result.deny_reason, Some("permanent ban".to_string()));
        assert!(result.blocked);
    }

    #[test]
    fn test_parse_command_output_update_output() {
        let mut result = HookResult::default();
        parse_command_output("update_output: [REDACTED]", &mut result);
        assert_eq!(result.updated_output, Some("[REDACTED]".to_string()));
    }

    #[test]
    fn test_parse_command_output_update_output_last_writer_wins() {
        let mut result = HookResult::default();
        parse_command_output("update_output: first\nupdate_output: second", &mut result);
        assert_eq!(result.updated_output, Some("second".to_string()));
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / '/tmp' fixtures)
    async fn test_hook_executor_command_with_context() {
        let hooks = vec![HookConfig {
            event: HookEvent::AfterToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'context: File formatted'".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session").with_tool_name("Write");

        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::AfterToolCall, context)
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 1);
        assert_eq!(result.additional_contexts, vec!["File formatted"]);
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / '/tmp' fixtures)
    async fn test_hook_executor_command() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'test output'".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session");

        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, context)
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 1);
        assert_eq!(result.action_results.len(), 1);
        assert!(result.action_results[0].success);
        assert!(result.action_results[0]
            .output
            .as_ref()
            .unwrap()
            .contains("test output"));
    }

    #[test]
    fn test_parse_command_output_allow() {
        let mut result = HookResult::default();
        parse_command_output("allow", &mut result);
        assert_eq!(result.permission_decision, Some(PermissionDecision::Allow));
    }

    #[test]
    fn test_parse_command_output_ask() {
        let mut result = HookResult::default();
        parse_command_output("ask: user must confirm destructive operation", &mut result);
        assert_eq!(
            result.permission_decision,
            Some(PermissionDecision::Ask {
                reason: "user must confirm destructive operation".to_string()
            })
        );
    }

    #[test]
    fn test_parse_command_output_deny_sets_permission_decision() {
        let mut result = HookResult::default();
        parse_command_output("deny: policy violation", &mut result);
        assert!(result.denied);
        assert_eq!(result.deny_reason, Some("policy violation".to_string()));
        assert_eq!(
            result.permission_decision,
            Some(PermissionDecision::Deny {
                reason: "policy violation".to_string()
            })
        );
    }

    #[test]
    fn test_parse_command_output_block_sets_permission_decision() {
        let mut result = HookResult::default();
        parse_command_output("block: temporary issue", &mut result);
        assert!(result.blocked);
        assert_eq!(result.block_reason, Some("temporary issue".to_string()));
        assert_eq!(
            result.permission_decision,
            Some(PermissionDecision::Block {
                reason: "temporary issue".to_string()
            })
        );
    }

    #[test]
    fn test_permission_decision_last_writer_wins() {
        let mut result = HookResult::default();
        parse_command_output("deny: first\nallow", &mut result);
        assert_eq!(result.permission_decision, Some(PermissionDecision::Allow));
    }

    fn command_hook(command: &str) -> HookConfig {
        HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: command.to_string(),
            }],
            plugin_name: "consent-test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn unapproved_shell_hook_is_skipped_and_recorded_pending() {
        use crate::sync_primitives::Arc;
        let dir = tempfile::tempdir().expect("tempdir");
        let consent = Arc::new(ShellHookConsent::with_path(
            dir.path().join("allowlist.json"),
        ));
        let executor = HookExecutor::new(vec![command_hook("echo SHOULD_NOT_RUN")])
            .with_consent(consent.clone());

        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .unwrap();

        // The hook matched but its command was gated off.
        assert_eq!(result.hooks_executed, 1);
        assert!(!result.action_results[0].success);
        assert!(result.action_results[0].output.is_none());
        // ...and it was surfaced for operator review.
        let pending = consent.entries();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, ConsentStatus::Pending);
        assert_eq!(pending[0].plugin_name, "consent-test");
    }

    #[tokio::test]
    async fn unapproved_http_hook_is_skipped_and_recorded_pending() {
        // The consent gate covers HTTP hooks too: shipping the event payload
        // (tool inputs/outputs) to a remote URL is an exfiltration vector and
        // must not happen without operator approval. The consent key carries
        // an `http:` prefix so it can't collide with a same-text shell command.
        use crate::sync_primitives::Arc;
        let dir = tempfile::tempdir().expect("tempdir");
        let consent = Arc::new(ShellHookConsent::with_path(
            dir.path().join("allowlist.json"),
        ));
        let hook = HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Http {
                url: "http://127.0.0.1:9/never-called".to_string(),
                headers: HashMap::new(),
            }],
            plugin_name: "consent-test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
        };
        let executor = HookExecutor::new(vec![hook]).with_consent(consent.clone());

        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 1);
        assert!(!result.action_results[0].success);
        assert!(result.action_results[0].output.is_none());
        let pending = consent.entries();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, ConsentStatus::Pending);
        assert!(
            pending[0].command.starts_with("http:"),
            "http consent key must be namespaced: {}",
            pending[0].command
        );
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / '/tmp' fixtures)
    async fn approved_shell_hook_runs_normally() {
        use crate::sync_primitives::Arc;
        let dir = tempfile::tempdir().expect("tempdir");
        let consent = Arc::new(ShellHookConsent::with_path(
            dir.path().join("allowlist.json"),
        ));
        let cmd = "echo approved_output";
        consent.record_pending("consent-test", cmd, "before_tool_call");
        let fp = consent.entries()[0].fingerprint.clone();
        consent.approve(&fp).expect("approve");

        let executor = HookExecutor::new(vec![command_hook(cmd)]).with_consent(consent.clone());
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 1);
        assert!(result.action_results[0].success);
        assert!(result.action_results[0]
            .output
            .as_deref()
            .unwrap_or_default()
            .contains("approved_output"));
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / '/tmp' fixtures)
    async fn shell_hook_runs_freely_when_no_consent_gate_attached() {
        // Back-compat: a `HookExecutor` with no consent gate (the default)
        // executes command hooks exactly as before.
        let executor = HookExecutor::new(vec![command_hook("echo ungated")]);
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .unwrap();
        assert!(result.action_results[0].success);
        assert!(result.action_results[0]
            .output
            .as_deref()
            .unwrap_or_default()
            .contains("ungated"));
    }

    #[cfg(unix)]
    fn observer_command_hook(event: HookEvent, command: &str) -> HookConfig {
        HookConfig {
            event,
            kind: HookKind::Observer,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: command.to_string(),
            }],
            plugin_name: "phase3-test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn fire_observer_empty_executor_is_noop() {
        // Must not panic when no hooks are registered.
        let executor = HookExecutor::new(vec![]);
        fire_observer(&executor, HookEvent::GatewayStart, "s", vec![]).await;
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: shell hook uses sh (touch fixture)
    async fn fire_observer_tool_name_env_feeds_the_matcher() {
        // Regression lock: fire-sites pass TOOL_NAME as env only; the hop
        // into `ctx.tool_name` is what lets a `matcher` on
        // PermissionRequest / Notification observers actually select.
        let dir = tempfile::tempdir().expect("tempdir");
        let sentinel = dir.path().join("matched.flag");
        let mut hook = observer_command_hook(
            HookEvent::Notification,
            &format!("touch '{}'", sentinel.display()),
        );
        hook.matcher = Some("bash_run".to_string());
        let executor = HookExecutor::new(vec![hook]);

        // Non-matching tool name → suppressed.
        fire_observer(
            &executor,
            HookEvent::Notification,
            "s",
            vec![("TOOL_NAME", "file_read".to_string())],
        )
        .await;
        assert!(!sentinel.exists(), "non-matching TOOL_NAME must not fire");

        // Matching tool name → observer runs.
        fire_observer(
            &executor,
            HookEvent::Notification,
            "s",
            vec![("TOOL_NAME", "bash_run".to_string())],
        )
        .await;
        assert!(sentinel.exists(), "matching TOOL_NAME must fire");
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / '/tmp' fixtures)
    async fn fire_observer_runs_only_the_matching_observer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sentinel = dir.path().join("fired.flag");
        let cmd = if cfg!(windows) {
            format!("type nul > \"{}\"", sentinel.display())
        } else {
            format!("touch '{}'", sentinel.display())
        };
        let executor = HookExecutor::new(vec![observer_command_hook(HookEvent::MessageSent, &cmd)]);

        // A mismatched event must not run the MessageSent observer.
        fire_observer(&executor, HookEvent::MessageReceived, "s", vec![]).await;
        assert!(!sentinel.exists(), "wrong-event observer must not run");

        // The matching event runs the observer command.
        fire_observer(
            &executor,
            HookEvent::MessageSent,
            "s",
            vec![("CHANNEL_ID", "telegram".to_string())],
        )
        .await;
        assert!(sentinel.exists(), "MessageSent observer must run");
    }
}
