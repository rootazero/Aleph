//! HookExecutor implementation — action dispatch and execution logic

use super::{
    substitute_variables, ActionResult, HookContext, HookResult, DEFAULT_COMMAND_TIMEOUT_SECS,
};
use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
use crate::extension::ExtensionError;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, trace, warn};

/// Hook executor - runs hook actions based on events
pub struct HookExecutor {
    pub(super) hooks: Vec<HookConfig>,
    /// Command timeout in seconds
    pub(super) command_timeout: Duration,
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
        plugin_root: &std::path::PathBuf,
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
        plugin_root: &std::path::PathBuf,
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
