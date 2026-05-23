//! HookExecutor implementation — action dispatch and execution logic

use super::{
    substitute_variables, ActionResult, HookContext, HookResult, ShellHookConsent,
    DEFAULT_COMMAND_TIMEOUT_SECS,
};
use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
use crate::extension::ExtensionError;
use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, trace, warn};

/// Build a Claude Code-style event payload JSON for stdin / HTTP body.
///
/// Schema (keyed `snake_case` to match the rest of the hook surface):
/// `{ hook_event_name, session_id, tool_name?, tool_input?, tool_output?, tool_error?, cwd?, env }`
fn build_event_payload(event: HookEvent, context: &HookContext) -> String {
    use serde_json::{json, Map, Value};
    let event_str = match serde_json::to_value(event) {
        Ok(Value::String(s)) => s,
        _ => format!("{event:?}").to_lowercase(),
    };
    let mut payload: Map<String, Value> = Map::new();
    payload.insert("hook_event_name".into(), Value::String(event_str));
    payload.insert(
        "session_id".into(),
        Value::String(context.session_id.clone()),
    );
    if let Some(t) = &context.tool_name {
        payload.insert("tool_name".into(), Value::String(t.clone()));
    }
    if let Some(t) = &context.tool_input {
        // Prefer parsed JSON; fall back to string when the tool_input is plain text.
        let parsed: Value = serde_json::from_str(t).unwrap_or_else(|_| Value::String(t.clone()));
        payload.insert("tool_input".into(), parsed);
    }
    if let Some(o) = &context.tool_output {
        payload.insert("tool_output".into(), Value::String(o.clone()));
    }
    if let Some(e) = context.tool_error {
        payload.insert("tool_error".into(), Value::Bool(e));
    }
    if let Some(c) = &context.working_dir {
        payload.insert("cwd".into(), Value::String(c.to_string_lossy().to_string()));
    }
    if !context.env.is_empty() {
        payload.insert("env".into(), json!(context.env));
    }
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

/// Hook executor - runs hook actions based on events
#[derive(Clone)]
pub struct HookExecutor {
    pub(super) hooks: Vec<HookConfig>,
    /// Command timeout in seconds
    pub(super) command_timeout: Duration,
    /// Compiled regex cache: matcher string -> compiled Regex (None if invalid)
    regex_cache: HashMap<String, Option<regex::Regex>>,
    /// Optional shell-hook consent allowlist. When set, `HookAction::Command`
    /// hooks only run if their command is operator-approved; un-approved
    /// commands are skipped (fail-safe) and recorded as `pending`. `None`
    /// disables the gate entirely (the default, so tests run commands freely).
    consent: Option<Arc<ShellHookConsent>>,
}

impl HookExecutor {
    /// Create a new hook executor
    pub fn new(hooks: Vec<HookConfig>) -> Self {
        let regex_cache = Self::build_regex_cache(&hooks);
        Self {
            hooks,
            command_timeout: Duration::from_secs(DEFAULT_COMMAND_TIMEOUT_SECS),
            regex_cache,
            consent: None,
        }
    }

    /// Set the command timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    /// Attach a shell-hook consent allowlist. With it set, `HookAction::Command`
    /// hooks run only when their command is operator-approved; un-approved
    /// commands are skipped and recorded as `pending` for review via the
    /// `aleph hooks` CLI.
    pub fn with_consent(mut self, consent: Arc<ShellHookConsent>) -> Self {
        self.consent = Some(consent);
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
                    .execute_action(
                        action,
                        context,
                        &hook.plugin_root,
                        &hook.plugin_name,
                        event,
                        hook.timeout_secs.map(Duration::from_secs),
                    )
                    .await;

                match action_result {
                    Ok(ar) => {
                        // Handle special action results
                        match action {
                            HookAction::Prompt { .. } => {
                                // The resolved prompt template is injected
                                // as additional context for the next LLM
                                // turn. (Out-of-band LLM judgment that
                                // returns {ok,reason} is a future enhancement
                                // — when missing, the prompt itself goes to
                                // the calling LLM, which is the next-best
                                // semantic.)
                                if let Some(ref output) = ar.output {
                                    result.additional_contexts.push(output.clone());
                                }
                            }
                            HookAction::Agent { agent } => {
                                result.agents_to_invoke.push(agent.clone());
                            }
                            HookAction::Command { .. } | HookAction::Http { .. } => {
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

    /// Execute a single action.
    ///
    /// `timeout_override` lets the per-hook `timeout_secs` setting take
    /// precedence over the executor's default. Applies to Command/Http.
    async fn execute_action(
        &self,
        action: &HookAction,
        context: &HookContext,
        plugin_root: &std::path::PathBuf,
        plugin_name: &str,
        event: HookEvent,
        timeout_override: Option<Duration>,
    ) -> Result<ActionResult, ExtensionError> {
        match action {
            HookAction::Command { command } => {
                self.execute_command(
                    command,
                    context,
                    plugin_root,
                    plugin_name,
                    event,
                    timeout_override,
                )
                .await
            }
            HookAction::Prompt { prompt } => {
                self.execute_prompt(prompt, context, plugin_root).await
            }
            HookAction::Agent { agent } => self.execute_agent(agent).await,
            HookAction::Http { url, headers } => {
                self.execute_http(url, headers, context, plugin_root, event, timeout_override)
                    .await
            }
        }
    }

    /// Effective timeout for a single hook execution (per-hook override or
    /// the executor default).
    fn effective_timeout(&self, override_secs: Option<u64>) -> Duration {
        override_secs
            .map(Duration::from_secs)
            .unwrap_or(self.command_timeout)
    }

    /// Execute a shell command
    async fn execute_command(
        &self,
        command: &str,
        context: &HookContext,
        plugin_root: &std::path::PathBuf,
        plugin_name: &str,
        event: HookEvent,
        timeout_override: Option<Duration>,
    ) -> Result<ActionResult, ExtensionError> {
        // Shell-hook consent gate: an un-approved command must not run. It is
        // recorded as `pending` (so `aleph hooks list` surfaces it) and the
        // action returns a non-success result with no output — interceptors
        // treat empty output as "no effect", so a skipped hook never blocks
        // the tool call. Approving arbitrary code execution is the operator's
        // explicit decision, not a default.
        if let Some(consent) = &self.consent {
            if !consent.is_approved(plugin_name, command) {
                consent.record_pending(plugin_name, command, &format!("{event:?}"));
                warn!(
                    plugin = plugin_name,
                    event = ?event,
                    "Shell hook command not approved — skipped. Review with `aleph hooks list`."
                );
                return Ok(ActionResult {
                    success: false,
                    output: None,
                    error: Some(format!(
                        "shell hook from plugin '{plugin_name}' is not approved; \
                         run `aleph hooks test` to review and approve it"
                    )),
                    exit_code: None,
                });
            }
        }

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

        // Configure stdio. The event JSON payload is piped to stdin so
        // hook scripts can `jq -r '.tool_input.file_path'` (Claude Code
        // convention). Env vars stay set for back-compat.
        let payload = build_event_payload(event, context);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Execute with timeout (per-hook override > executor default)
        let effective = self.effective_timeout(timeout_override.map(|d| d.as_secs()));
        let output = match timeout(effective, async {
            let mut child = cmd.spawn().map_err(|e| {
                ExtensionError::HookExecution(format!("Failed to spawn command: {}", e))
            })?;
            if let Some(mut stdin) = child.stdin.take() {
                // Best-effort: if stdin write fails (hook ignored stdin), keep going.
                let _ = stdin.write_all(payload.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }
            child.wait_with_output().await.map_err(|e| {
                ExtensionError::HookExecution(format!("Failed to await command: {}", e))
            })
        })
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                return Err(ExtensionError::HookExecution(format!(
                    "Command timed out after {:?}",
                    effective
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

    /// Execute an HTTP hook — POST the event JSON payload to `url` and
    /// parse the response body using the same line-prefix protocol as
    /// command hooks. Useful for team audit logs, webhooks, and
    /// LLM-judge gateways without spawning a shell.
    async fn execute_http(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        context: &HookContext,
        plugin_root: &Path,
        event: HookEvent,
        timeout_override: Option<Duration>,
    ) -> Result<ActionResult, ExtensionError> {
        let resolved_url = substitute_variables(url, context, plugin_root);
        let payload = build_event_payload(event, context);
        let effective = self.effective_timeout(timeout_override.map(|d| d.as_secs()));

        let client = reqwest::Client::builder()
            .timeout(effective)
            .build()
            .map_err(|e| {
                ExtensionError::HookExecution(format!("Failed to build HTTP client: {}", e))
            })?;

        let mut req = client
            .post(&resolved_url)
            .header("content-type", "application/json")
            .body(payload);
        for (k, v) in headers {
            // Only context-env substitution — no process env — so a misconfigured
            // template can't leak `$AWS_SECRET_ACCESS_KEY` etc.
            let resolved_v = substitute_variables(v, context, plugin_root);
            req = req.header(k.as_str(), resolved_v);
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    warn!("Hook HTTP {} -> {}: {}", resolved_url, status, body);
                }
                Ok(ActionResult {
                    success: status.is_success(),
                    output: if body.is_empty() { None } else { Some(body) },
                    error: if status.is_success() {
                        None
                    } else {
                        Some(format!("HTTP {}", status.as_u16()))
                    },
                    exit_code: Some(status.as_u16() as i32),
                })
            }
            Err(e) => Ok(ActionResult {
                success: false,
                output: None,
                error: Some(format!("HTTP request failed: {}", e)),
                exit_code: None,
            }),
        }
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
                    .execute_action(
                        action,
                        &current_context,
                        &hook.plugin_root,
                        &hook.plugin_name,
                        event,
                        hook.timeout_secs.map(Duration::from_secs),
                    )
                    .await;

                match action_result {
                    Ok(ar) => {
                        match action {
                            HookAction::Command { .. } | HookAction::Http { .. } => {
                                if let Some(ref output) = ar.output {
                                    super::parse_command_output(output, &mut accumulated);
                                    if accumulated.blocked || accumulated.denied {
                                        return Ok((current_context, accumulated));
                                    }
                                }
                            }
                            HookAction::Prompt { .. } => {
                                if let Some(ref output) = ar.output {
                                    accumulated.additional_contexts.push(output.clone());
                                }
                            }
                            HookAction::Agent { agent } => {
                                accumulated.agents_to_invoke.push(agent.clone());
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
                let timeout_override = hook.timeout_secs.map(Duration::from_secs);
                for action in &hook.actions {
                    if let Err(e) = self
                        .execute_action(
                            action,
                            context,
                            &hook.plugin_root,
                            &hook.plugin_name,
                            event,
                            timeout_override,
                        )
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
                    .execute_action(
                        action,
                        context,
                        &hook.plugin_root,
                        &hook.plugin_name,
                        event,
                        hook.timeout_secs.map(Duration::from_secs),
                    )
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
