//! Hook execution system
//!
//! Handles event-driven hooks for tool lifecycle, session events, etc.
//!
//! # Hook Events
//!
//! - `BeforeToolCall` / `AfterToolCall` - Tool execution lifecycle
//! - `BeforeAgentStart` / `AgentEnd` - Agent lifecycle
//! - `SessionStart` / `SessionEnd` - Session lifecycle
//! - `MessageReceived` / `MessageSending` / `MessageSent` - Message flow
//! - `BeforeCompaction` / `AfterCompaction` - Context compaction
//! - `GatewayStart` / `GatewayStop` - Gateway lifecycle
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::extension::hooks::{HookExecutor, HookContext};
//!
//! let executor = HookExecutor::new(hooks);
//!
//! // Execute pre-tool hooks
//! let result = executor.execute(HookEvent::BeforeToolCall, &context).await?;
//! if result.blocked {
//!     return Err("Tool blocked by hook");
//! }
//! ```

mod executor;

pub use executor::HookExecutor;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default timeout for command execution (300 seconds)
const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 300;

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
    pub fn with_tool_error(mut self, is_error: bool) -> Self {
        self.tool_error = Some(is_error);
        self
    }
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

/// Hook execution result (aggregated from all matching hooks)
#[derive(Debug, Default)]
pub struct HookResult {
    /// Whether the action was blocked (for BeforeToolCall)
    pub blocked: bool,
    /// Block reason (if blocked)
    pub block_reason: Option<String>,
    /// Modified arguments (if any hook modified them)
    pub modified_arguments: Option<String>,
    /// Messages to inject into the conversation
    pub messages: Vec<String>,
    /// Agents to invoke
    pub agents_to_invoke: Vec<String>,
    /// Individual action results
    pub action_results: Vec<ActionResult>,
    /// Number of hooks executed
    pub hooks_executed: usize,
    /// Hook-modified tool input (JSON). Last writer wins across interceptor chain.
    pub updated_input: Option<serde_json::Value>,
    /// Additional context strings to inject into next LLM turn (as system-reminders).
    pub additional_contexts: Vec<String>,
    /// If true, agent loop should stop even if the tool succeeded.
    pub prevent_continuation: bool,
}

impl HookResult {
    /// Check if all actions succeeded
    pub fn all_succeeded(&self) -> bool {
        self.action_results.iter().all(|r| r.success)
    }

    /// Get all outputs from successful actions
    pub fn outputs(&self) -> Vec<&str> {
        self.action_results
            .iter()
            .filter(|r| r.success)
            .filter_map(|r| r.output.as_deref())
            .collect()
    }

    /// Get all errors from failed actions
    pub fn errors(&self) -> Vec<&str> {
        self.action_results
            .iter()
            .filter(|r| !r.success)
            .filter_map(|r| r.error.as_deref())
            .collect()
    }
}

/// Result from an interceptor hook
#[derive(Debug, Clone, Default)]
pub struct InterceptorResult {
    /// Whether the request should pass through
    pub pass: bool,
    /// Modified context (if the interceptor modified the request)
    pub modified_context: Option<HookContext>,
    /// Reason for blocking (if pass is false)
    pub block_reason: Option<String>,
    /// Whether to suppress the block message from the user
    pub silent: bool,
}

impl InterceptorResult {
    /// Create a passing result
    pub fn pass() -> Self {
        Self {
            pass: true,
            ..Default::default()
        }
    }

    /// Create a blocking result with a reason
    pub fn block(reason: impl Into<String>) -> Self {
        Self {
            pass: false,
            block_reason: Some(reason.into()),
            ..Default::default()
        }
    }

    /// Create a silent blocking result (no message shown to user)
    pub fn block_silent(reason: impl Into<String>) -> Self {
        Self {
            pass: false,
            block_reason: Some(reason.into()),
            silent: true,
            ..Default::default()
        }
    }

    /// Create a passing result with modified context
    pub fn modified(ctx: HookContext) -> Self {
        Self {
            pass: true,
            modified_context: Some(ctx),
            ..Default::default()
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
        result = result.replace(&format!("${}", key), value);
        result = result.replace(&format!("${{{}}}", key), value);
    }

    result
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
        assert_eq!(ctx.tool_output, Some("File written successfully".to_string()));
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

        let result = executor
            .execute(HookEvent::BeforeToolCall, &context)
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 0);
        assert!(!result.blocked);
    }

    #[tokio::test]
    async fn test_hook_executor_with_prompt() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::default(),
            priority: HookPriority::default(),
            matcher: Some("Write".to_string()),
            actions: vec![HookAction::Prompt {
                prompt: "Checking ${TOOL_NAME} operation".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/plugin"),
            handler: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session").with_tool_name("Write");

        let result = executor
            .execute(HookEvent::BeforeToolCall, &context)
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 1);
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0], "Checking Write operation");
    }

    #[tokio::test]
    async fn test_hook_executor_pattern_mismatch() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::default(),
            priority: HookPriority::default(),
            matcher: Some("Write".to_string()),
            actions: vec![HookAction::Prompt {
                prompt: "test".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/plugin"),
            handler: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session").with_tool_name("Read");

        let result = executor
            .execute(HookEvent::BeforeToolCall, &context)
            .await
            .unwrap();

        // Pattern doesn't match, so no hooks executed
        assert_eq!(result.hooks_executed, 0);
    }

    #[tokio::test]
    async fn test_hook_executor_regex_pattern() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::default(),
            priority: HookPriority::default(),
            matcher: Some("Write|Edit".to_string()),
            actions: vec![HookAction::Prompt {
                prompt: "Modifying file".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/plugin"),
            handler: None,
        }];

        let executor = HookExecutor::new(hooks);

        // Test with Write
        let context = HookContext::new("session").with_tool_name("Write");
        let result = executor
            .execute(HookEvent::BeforeToolCall, &context)
            .await
            .unwrap();
        assert_eq!(result.hooks_executed, 1);

        // Test with Edit
        let context = HookContext::new("session").with_tool_name("Edit");
        let result = executor
            .execute(HookEvent::BeforeToolCall, &context)
            .await
            .unwrap();
        assert_eq!(result.hooks_executed, 1);

        // Test with Read (no match)
        let context = HookContext::new("session").with_tool_name("Read");
        let result = executor
            .execute(HookEvent::BeforeToolCall, &context)
            .await
            .unwrap();
        assert_eq!(result.hooks_executed, 0);
    }

    #[tokio::test]
    async fn test_hook_executor_with_agent() {
        let hooks = vec![HookConfig {
            event: HookEvent::AfterToolCall,
            kind: HookKind::default(),
            priority: HookPriority::default(),
            matcher: None, // Matches all
            actions: vec![HookAction::Agent {
                agent: "review-agent".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/plugin"),
            handler: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session").with_tool_name("Write");

        let result = executor
            .execute(HookEvent::AfterToolCall, &context)
            .await
            .unwrap();

        assert_eq!(result.hooks_executed, 1);
        assert_eq!(result.agents_to_invoke, vec!["review-agent"]);
    }

    #[tokio::test]
    async fn test_hook_executor_command() {
        let hooks = vec![HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::default(),
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "echo 'test output'".to_string(),
            }],
            plugin_name: "test-plugin".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
        }];

        let executor = HookExecutor::new(hooks);
        let context = HookContext::new("session");

        let result = executor
            .execute(HookEvent::BeforeToolCall, &context)
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
}
