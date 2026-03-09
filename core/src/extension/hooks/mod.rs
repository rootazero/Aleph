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

use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
use crate::extension::ExtensionError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, trace, warn};

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

/// Hook executor - runs hook actions based on events
pub struct HookExecutor {
    hooks: Vec<HookConfig>,
    /// Command timeout in seconds
    command_timeout: Duration,
}

impl HookExecutor {
    /// Create a new hook executor
    pub fn new(hooks: Vec<HookConfig>) -> Self {
        Self {
            hooks,
            command_timeout: Duration::from_secs(DEFAULT_COMMAND_TIMEOUT_SECS),
        }
    }

    /// Set the command timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    /// Create a new empty hook executor
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Add a hook to the executor
    pub fn add_hook(&mut self, hook: HookConfig) {
        self.hooks.push(hook);
    }

    /// Get the number of hooks
    pub fn hook_count(&self) -> usize {
        self.hooks.len()
    }

    /// Execute hooks for an event
    pub async fn execute(
        &self,
        event: HookEvent,
        context: &HookContext,
    ) -> Result<HookResult, ExtensionError> {
        let mut result = HookResult::default();

        for hook in &self.hooks {
            if hook.event != event {
                continue;
            }

            // Check matcher pattern
            if !self.matches_pattern(hook, context) {
                continue;
            }

            debug!(
                "Executing hook from plugin '{}' for event {:?}",
                hook.plugin_name, event
            );
            result.hooks_executed += 1;

            // Execute all actions for this hook
            for action in &hook.actions {
                let action_result = self.execute_action(action, context, &hook.plugin_root).await;

                match action_result {
                    Ok(ar) => {
                        // Handle special action results
                        match action {
                            HookAction::Prompt { .. } => {
                                if let Some(ref output) = ar.output {
                                    result.messages.push(output.clone());
                                }
                            }
                            HookAction::Agent { agent } => {
                                result.agents_to_invoke.push(agent.clone());
                            }
                            HookAction::Command { .. } => {
                                // Check for block signal in command output
                                if let Some(ref output) = ar.output {
                                    if output.trim().to_lowercase().starts_with("block:") {
                                        result.blocked = true;
                                        result.block_reason =
                                            Some(output.trim()[6..].trim().to_string());
                                    }
                                }
                            }
                        }
                        result.action_results.push(ar);
                    }
                    Err(e) => {
                        warn!("Hook action failed: {}", e);
                        result.action_results.push(ActionResult {
                            success: false,
                            output: None,
                            error: Some(e.to_string()),
                            exit_code: None,
                        });
                    }
                }
            }
        }

        trace!(
            "Hook execution complete: {} hooks, {} actions",
            result.hooks_executed,
            result.action_results.len()
        );

        Ok(result)
    }

    /// Check if a hook's pattern matches the context
    fn matches_pattern(&self, hook: &HookConfig, context: &HookContext) -> bool {
        // If no matcher, hook applies to all
        let matcher = match &hook.matcher {
            Some(m) => m,
            None => return true,
        };

        // Get the tool name to match against
        let tool_name = match &context.tool_name {
            Some(n) => n,
            None => return false, // No tool name, can't match
        };

        // Try regex match
        match regex::Regex::new(matcher) {
            Ok(re) => re.is_match(tool_name),
            Err(e) => {
                warn!("Invalid hook matcher regex '{}': {}", matcher, e);
                false
            }
        }
    }

    /// Execute a single action
    async fn execute_action(
        &self,
        action: &HookAction,
        context: &HookContext,
        plugin_root: &PathBuf,
    ) -> Result<ActionResult, ExtensionError> {
        match action {
            HookAction::Command { command } => {
                self.execute_command(command, context, plugin_root).await
            }
            HookAction::Prompt { prompt } => {
                self.execute_prompt(prompt, context, plugin_root).await
            }
            HookAction::Agent { agent } => self.execute_agent(agent).await,
        }
    }

    /// Execute a shell command
    async fn execute_command(
        &self,
        command: &str,
        context: &HookContext,
        plugin_root: &PathBuf,
    ) -> Result<ActionResult, ExtensionError> {
        // Substitute variables
        let resolved = substitute_variables(command, context, plugin_root);
        debug!("Executing hook command: {}", resolved);

        // Determine working directory
        let working_dir = context
            .working_dir
            .as_ref()
            .unwrap_or(plugin_root);

        // Build command
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", &resolved]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", &resolved]);
            c
        };

        // Set working directory
        cmd.current_dir(working_dir);

        // Set environment variables
        cmd.env("PLUGIN_ROOT", plugin_root);
        cmd.env("CLAUDE_PLUGIN_ROOT", plugin_root);
        if let Some(ref tool_name) = context.tool_name {
            cmd.env("TOOL_NAME", tool_name);
        }
        if let Some(ref args) = context.arguments {
            cmd.env("ARGUMENTS", args);
        }
        if let Some(ref input) = context.tool_input {
            cmd.env("TOOL_INPUT", input);
        }
        if let Some(ref file) = context.file_path {
            cmd.env("FILE", file);
        }
        cmd.env("SESSION_ID", &context.session_id);

        // Add custom environment variables
        for (key, value) in &context.env {
            cmd.env(key, value);
        }

        // Configure stdio
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Execute with timeout
        let output = match timeout(self.command_timeout, cmd.output()).await {
            Ok(result) => result.map_err(|e| {
                ExtensionError::HookExecution(format!("Failed to execute command: {}", e))
            })?,
            Err(_) => {
                return Err(ExtensionError::HookExecution(format!(
                    "Command timed out after {:?}",
                    self.command_timeout
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            warn!(
                "Hook command exited with status {:?}: {}",
                output.status.code(),
                stderr
            );
        }

        Ok(ActionResult {
            success: output.status.success(),
            output: if stdout.is_empty() {
                None
            } else {
                Some(stdout)
            },
            error: if stderr.is_empty() {
                None
            } else {
                Some(stderr)
            },
            exit_code: output.status.code(),
        })
    }

    /// Execute a prompt hook (returns prompt for LLM evaluation)
    async fn execute_prompt(
        &self,
        prompt: &str,
        context: &HookContext,
        plugin_root: &Path,
    ) -> Result<ActionResult, ExtensionError> {
        let resolved = substitute_variables(prompt, context, plugin_root);

        Ok(ActionResult {
            success: true,
            output: Some(resolved),
            error: None,
            exit_code: None,
        })
    }

    /// Execute an agent hook (returns agent name for the caller to invoke)
    async fn execute_agent(&self, agent: &str) -> Result<ActionResult, ExtensionError> {
        Ok(ActionResult {
            success: true,
            output: Some(agent.to_string()),
            error: None,
            exit_code: None,
        })
    }

    /// Execute interceptor hooks for an event
    ///
    /// Interceptors run sequentially in priority order and can:
    /// - Block execution (short-circuit)
    /// - Modify the context for downstream processing
    ///
    /// Returns the (possibly modified) context and an optional block reason.
    pub async fn execute_interceptors(
        &self,
        event: HookEvent,
        context: HookContext,
    ) -> Result<(HookContext, Option<String>), ExtensionError> {
        // Filter hooks by event and kind == Interceptor
        let mut interceptors: Vec<_> = self
            .hooks
            .iter()
            .filter(|h| h.event == event && h.kind == HookKind::Interceptor)
            .collect();

        // Sort by priority (lower value = earlier execution)
        interceptors.sort_by_key(|h| h.priority.as_i32());

        let current_context = context;

        for hook in interceptors {
            // Check matcher pattern
            if !self.matches_pattern(hook, &current_context) {
                continue;
            }

            debug!(
                "Executing interceptor hook from plugin '{}' for event {:?}",
                hook.plugin_name, event
            );

            // Execute all actions for this hook
            for action in &hook.actions {
                let action_result = self
                    .execute_action(action, &current_context, &hook.plugin_root)
                    .await;

                match action_result {
                    Ok(ar) => {
                        // Check for block signal in command output
                        if let HookAction::Command { .. } = action {
                            if let Some(ref output) = ar.output {
                                if output.trim().to_lowercase().starts_with("block:") {
                                    let reason = output.trim()[6..].trim().to_string();
                                    return Ok((current_context, Some(reason)));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Interceptor hook action failed: {}", e);
                        // Interceptor failures block by default for safety
                        return Ok((
                            current_context,
                            Some(format!("Interceptor hook failed: {}", e)),
                        ));
                    }
                }
            }
        }

        Ok((current_context, None))
    }

    /// Execute observer hooks for an event
    ///
    /// Observers run in parallel and cannot block or modify the context.
    /// Errors are logged but do not propagate.
    pub async fn execute_observers(&self, event: HookEvent, context: &HookContext) {
        // Filter hooks by event and kind == Observer
        let observers: Vec<_> = self
            .hooks
            .iter()
            .filter(|h| h.event == event && h.kind == HookKind::Observer)
            .filter(|h| self.matches_pattern(h, context))
            .collect();

        if observers.is_empty() {
            return;
        }

        debug!(
            "Executing {} observer hooks for event {:?}",
            observers.len(),
            event
        );

        // Execute all observers in parallel
        let futures: Vec<_> = observers
            .into_iter()
            .map(|hook| async move {
                for action in &hook.actions {
                    if let Err(e) = self
                        .execute_action(action, context, &hook.plugin_root)
                        .await
                    {
                        warn!(
                            "Observer hook action from plugin '{}' failed: {}",
                            hook.plugin_name, e
                        );
                    }
                }
            })
            .collect();

        futures::future::join_all(futures).await;
    }

    /// Execute resolver hooks for an event
    ///
    /// Resolvers run sequentially in priority order and stop when one returns a value.
    /// The `resolver_fn` is called with each hook's action results to extract the value.
    ///
    /// # Type Parameters
    /// - `T`: The type of value being resolved
    /// - `F`: A function that takes action results and returns `Option<T>`
    pub async fn execute_resolvers<T, F>(
        &self,
        event: HookEvent,
        context: &HookContext,
        resolver_fn: F,
    ) -> Option<T>
    where
        F: Fn(&[ActionResult]) -> Option<T>,
    {
        // Filter hooks by event and kind == Resolver
        let mut resolvers: Vec<_> = self
            .hooks
            .iter()
            .filter(|h| h.event == event && h.kind == HookKind::Resolver)
            .collect();

        // Sort by priority (lower value = earlier execution)
        resolvers.sort_by_key(|h| h.priority.as_i32());

        for hook in resolvers {
            // Check matcher pattern
            if !self.matches_pattern(hook, context) {
                continue;
            }

            debug!(
                "Executing resolver hook from plugin '{}' for event {:?}",
                hook.plugin_name, event
            );

            // Execute all actions for this hook and collect results
            let mut action_results = Vec::new();
            for action in &hook.actions {
                match self
                    .execute_action(action, context, &hook.plugin_root)
                    .await
                {
                    Ok(ar) => action_results.push(ar),
                    Err(e) => {
                        warn!(
                            "Resolver hook action from plugin '{}' failed: {}",
                            hook.plugin_name, e
                        );
                    }
                }
            }

            // Try to resolve using the provided function
            if let Some(value) = resolver_fn(&action_results) {
                return Some(value);
            }
        }

        None
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

        let result = substitute_variables(
            "$CUSTOM_VAR and ${ANOTHER}",
            &context,
            &plugin_root,
        );

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

        let result = executor.execute(HookEvent::BeforeToolCall, &context).await.unwrap();

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

        let result = executor.execute(HookEvent::BeforeToolCall, &context).await.unwrap();

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

        let result = executor.execute(HookEvent::BeforeToolCall, &context).await.unwrap();

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
        let result = executor.execute(HookEvent::BeforeToolCall, &context).await.unwrap();
        assert_eq!(result.hooks_executed, 1);

        // Test with Edit
        let context = HookContext::new("session").with_tool_name("Edit");
        let result = executor.execute(HookEvent::BeforeToolCall, &context).await.unwrap();
        assert_eq!(result.hooks_executed, 1);

        // Test with Read (no match)
        let context = HookContext::new("session").with_tool_name("Read");
        let result = executor.execute(HookEvent::BeforeToolCall, &context).await.unwrap();
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

        let result = executor.execute(HookEvent::AfterToolCall, &context).await.unwrap();

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

        let result = executor.execute(HookEvent::BeforeToolCall, &context).await.unwrap();

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
