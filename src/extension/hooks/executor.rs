//! HookExecutor implementation — action dispatch and execution logic

use super::{
    substitute_variables, ActionResult, HookContext, HookResult, DEFAULT_COMMAND_TIMEOUT_SECS,
};
use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
use crate::extension::ExtensionError;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, trace, warn};

/// Hook executor - runs hook actions based on events
#[derive(Clone)]
pub struct HookExecutor {
    pub(super) hooks: Vec<HookConfig>,
    /// Command timeout in seconds
    pub(super) command_timeout: Duration,
    /// Compiled regex cache: matcher string -> compiled Regex (None if invalid)
    regex_cache: HashMap<String, Option<regex::Regex>>,
}

impl HookExecutor {
    /// Create a new hook executor
    pub fn new(hooks: Vec<HookConfig>) -> Self {
        let regex_cache = Self::build_regex_cache(&hooks);
        Self {
            hooks,
            command_timeout: Duration::from_secs(DEFAULT_COMMAND_TIMEOUT_SECS),
            regex_cache,
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
        if let Some(ref matcher) = hook.matcher {
            self.cache_regex(matcher);
        }
        self.hooks.push(hook);
    }

    /// Build regex cache from all hooks
    fn build_regex_cache(hooks: &[HookConfig]) -> HashMap<String, Option<regex::Regex>> {
        let mut cache = HashMap::new();
        for hook in hooks {
            if let Some(ref matcher) = hook.matcher {
                if !cache.contains_key(matcher) {
                    match regex::Regex::new(matcher) {
                        Ok(re) => {
                            cache.insert(matcher.clone(), Some(re));
                        }
                        Err(e) => {
                            warn!("Invalid hook matcher regex '{}': {}", matcher, e);
                            cache.insert(matcher.clone(), None);
                        }
                    }
                }
            }
        }
        cache
    }

    /// Cache a single regex pattern
    fn cache_regex(&mut self, pattern: &str) {
        if !self.regex_cache.contains_key(pattern) {
            match regex::Regex::new(pattern) {
                Ok(re) => {
                    self.regex_cache.insert(pattern.to_string(), Some(re));
                }
                Err(e) => {
                    warn!("Invalid hook matcher regex '{}': {}", pattern, e);
                    self.regex_cache.insert(pattern.to_string(), None);
                }
            }
        }
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
                let action_result = self
                    .execute_action(action, context, &hook.plugin_root)
                    .await;

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
                                if let Some(ref output) = ar.output {
                                    super::parse_command_output(output, &mut result);
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

        // Look up compiled regex from cache
        match self.regex_cache.get(matcher.as_str()) {
            Some(Some(re)) => re.is_match(tool_name),
            Some(None) => false, // Invalid regex, logged at cache time
            None => {
                // Fallback: compile on the fly (should not happen if add_hook was used)
                match regex::Regex::new(matcher) {
                    Ok(re) => re.is_match(tool_name),
                    Err(e) => {
                        warn!("Invalid hook matcher regex '{}': {}", matcher, e);
                        false
                    }
                }
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
        let working_dir = context.working_dir.as_ref().unwrap_or(plugin_root);

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

    /// Execute interceptor hooks for an event.
    ///
    /// Interceptors run sequentially in priority order and can:
    /// - Block execution (short-circuit)
    /// - Modify tool input via `update_input:`
    /// - Inject additional contexts and messages
    ///
    /// Returns the (possibly modified) context and a `HookResult` that
    /// accumulates outputs from all non-blocking interceptors. If any
    /// interceptor blocks, the result's `blocked` field is `true` and
    /// execution short-circuits.
    pub async fn execute_interceptors(
        &self,
        event: HookEvent,
        context: HookContext,
    ) -> Result<(HookContext, super::HookResult), ExtensionError> {
        let mut accumulated = super::HookResult::default();

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
            accumulated.hooks_executed += 1;

            // Execute all actions for this hook
            for action in &hook.actions {
                let action_result = self
                    .execute_action(action, &current_context, &hook.plugin_root)
                    .await;

                match action_result {
                    Ok(ar) => {
                        if let HookAction::Command { .. } = action {
                            if let Some(ref output) = ar.output {
                                super::parse_command_output(output, &mut accumulated);
                                if accumulated.blocked {
                                    return Ok((current_context, accumulated));
                                }
                            }
                        }
                        accumulated.action_results.push(ar);
                    }
                    Err(e) => {
                        warn!("Interceptor hook action failed: {}", e);
                        // Interceptor failures block by default for safety
                        accumulated.blocked = true;
                        accumulated.block_reason = Some(format!("Interceptor hook failed: {}", e));
                        return Ok((current_context, accumulated));
                    }
                }
            }
        }

        Ok((current_context, accumulated))
    }

    /// Execute observer hooks for an event
    ///
    /// Different observers run in parallel, but actions within each observer
    /// run sequentially. Observers cannot block or modify the context.
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
