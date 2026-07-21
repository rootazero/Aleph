//! `HookExecutor` implementation — action dispatch and execution logic

use super::{
    substitute_variables, ActionResult, HookContext, ShellHookConsent,
    DEFAULT_COMMAND_TIMEOUT_SECS,
};
use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
use crate::extension::ExtensionError;
use crate::sync_primitives::Arc;
use crate::utils::no_window::NoWindow;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

/// Cap on bytes read from a hook's stdout / stderr / HTTP response body.
/// Same value as the stop-hook executor's `MAX_OUTPUT_BYTES` — a hook that
/// dumps megabytes must not flood process memory (and, via
/// `additional_contexts`, the model's context window; the context budget
/// pipeline is the second line of defence). codex spills oversized hook
/// output to disk; a hard cap is the minimal Aleph equivalent.
const MAX_HOOK_OUTPUT_BYTES: u64 = 64 * 1024;

/// Read at most `cap` bytes from `r`, then drain (and discard) the rest so
/// the writing child process never blocks on a full pipe — a blocked child
/// would otherwise hang `wait()` until the timeout kills it, turning
/// "output too large" into a spurious "hook timed out".
///
/// Returns `(bytes, truncated)`. Callers whose protocol carries decisions in
/// the STREAM (extension hooks: `deny:` lines / JSON objects on stdout) must
/// treat `truncated == true` as a hard failure — a decision directive past
/// the cap would otherwise be silently dropped and the hook would fail OPEN.
/// Callers whose decision rides the EXIT CODE (stop hooks) can safely keep
/// the truncated text as a best-effort reason.
pub(crate) async fn read_capped<R: AsyncRead + Unpin>(mut r: R, cap: u64) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut truncated = (&mut r).take(cap).read_to_end(&mut buf).await.is_err();
    let mut sink = [0u8; 8192];
    loop {
        match r.read(&mut sink).await {
            Ok(0) => break,
            Ok(_) => truncated = true,
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }
    (buf, truncated)
}

/// Max bytes for a single payload-mirroring env var (`ARGUMENTS` /
/// `TOOL_INPUT`). Comfortably under every platform's `ARG_MAX` (Linux 128KB
/// per string, macOS 256KB total) so setting it can never push the spawn
/// over the limit. The full value is always available on stdin.
const MAX_ENV_VALUE_BYTES: usize = 32 * 1024;

/// Set `key` to `value` on `cmd`, but skip (with a placeholder) any value
/// past [`MAX_ENV_VALUE_BYTES`] — an oversized env var can make `spawn` fail
/// with E2BIG, which on an interceptor seam fails closed and blocks the tool.
fn set_env_bounded(cmd: &mut Command, key: &str, value: Option<&str>) {
    match value {
        Some(v) if v.len() <= MAX_ENV_VALUE_BYTES => {
            cmd.env(key, v);
        }
        Some(v) => {
            debug!(
                key,
                len = v.len(),
                "hook env value exceeds cap; omitting from env (full value is on stdin)"
            );
            cmd.env(key, format!("[{} bytes — read from stdin JSON]", v.len()));
        }
        None => {}
    }
}

/// Render the additional-context directive for a `HookAction::Agent`.
///
/// The hook executor never spawns agents inline (R10) — the directive asks
/// the calling LLM to delegate via the `subagent` tool, the same next-best
/// semantic the Prompt action documents. Flows to the model through the
/// existing `additional_contexts` plumbing (system-reminder blocks on the
/// tool-dispatch and run-loop consumption paths).
fn agent_invoke_directive(plugin_name: &str, event: HookEvent, agent: &str) -> String {
    format!(
        "Hook '{plugin_name}' ({event:?}) requests delegating to agent '{agent}'. \
         Invoke the `subagent` tool with agent_type=\"{agent}\", passing the \
         relevant event context as the task."
    )
}

/// Build a Claude Code-style event payload as a JSON value.
///
/// Schema (keyed `snake_case` to match the rest of the hook surface):
/// `{ hook_event_name, session_id, tool_name?, tool_input?, tool_output?, tool_error?, cwd?, env }`
///
/// Shared by the stdin/HTTP string form ([`build_event_payload`]) and the
/// plugin-hook path, which passes the value straight to `execute_plugin_hook`
/// without a string round-trip.
fn build_event_payload_value(event: HookEvent, context: &HookContext) -> serde_json::Value {
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
    Value::Object(payload)
}

/// Build a Claude Code-style event payload JSON string for stdin / HTTP body.
fn build_event_payload(event: HookEvent, context: &HookContext) -> String {
    serde_json::to_string(&build_event_payload_value(event, context))
        .unwrap_or_else(|_| "{}".to_string())
}

/// Public alias of [`build_event_payload`] for out-of-band verification
/// (`aleph hooks test`): the CLI pipes THIS to the hook's stdin, so a hook
/// exercised there sees the exact payload structure production sends —
/// hermes' "test through the same serializer" pattern. A hook that reads
/// stdin (`jq -r '.tool_name'`) behaves identically in both worlds instead
/// of hanging on an unwired stdin.
#[must_use]
pub fn event_payload_json(event: HookEvent, context: &HookContext) -> String {
    build_event_payload(event, context)
}

/// Compare two directory paths for identity, canonicalising best-effort so
/// symlinks (`/var` → `/private/var`), `.`/`..` segments and trailing slashes
/// don't cause a spurious mismatch. Falls back to the raw path when a side
/// doesn't resolve so the comparison degrades gracefully rather than failing.
fn paths_equal(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
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
    #[must_use]
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
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
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
    #[must_use]
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

    /// Remove every hook whose `plugin_name` starts with `prefix`.
    ///
    /// Used by the hot-reload path: user-level hooks loaded from
    /// `~/.aleph/hooks.json` are tagged `user:global` / `user:project` /
    /// `user:project-local`, so calling `remove_by_plugin_prefix("user:")`
    /// drops the prior layer before re-appending the freshly-parsed config —
    /// preventing duplicate registrations on every hot-reload tick.
    ///
    /// Returns the number of hooks removed.
    pub fn remove_by_plugin_prefix(&mut self, prefix: &str) -> usize {
        let before = self.hooks.len();
        self.hooks.retain(|h| !h.plugin_name.starts_with(prefix));
        // Regex cache is keyed by matcher, not plugin — entries become
        // harmlessly orphaned, not stale; cleanup on the next compile is
        // sufficient. Re-building the whole cache here would be wasted work.
        before - self.hooks.len()
    }

    /// Build regex cache from all hooks
    fn build_regex_cache(hooks: &[HookConfig]) -> HashMap<String, Option<regex::Regex>> {
        let mut cache = HashMap::new();
        for hook in hooks {
            if let Some(ref matcher) = hook.matcher {
                if !cache.contains_key(matcher) {
                    match crate::security::safe_regex::bounded_builder(matcher).build() {
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
            match crate::security::safe_regex::bounded_builder(pattern).build() {
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
    #[must_use]
    pub const fn hook_count(&self) -> usize {
        self.hooks.len()
    }

    /// Whether any registered hook targets `event`. Cheap pre-flight so
    /// fire-sites (e.g. the extension stop gate) can skip context building
    /// entirely on the common no-hooks path.
    #[must_use]
    pub fn has_hooks_for(&self, event: HookEvent) -> bool {
        self.hooks.iter().any(|h| h.event == event)
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
                match crate::security::safe_regex::bounded_builder(matcher).build() {
                    Ok(re) => re.is_match(tool_name),
                    Err(e) => {
                        warn!("Invalid hook matcher regex '{}': {}", matcher, e);
                        false
                    }
                }
            }
        }
    }

    /// Gate a project-scoped user hook to the active workspace.
    ///
    /// Hooks loaded from a project's `.aleph/hooks*.json` are tagged
    /// `user:project*` and carry that project's `.aleph` directory as their
    /// `plugin_root`. The daemon serves every registered project from one
    /// process, so all such hooks live in one shared executor — without this
    /// gate a hook checked into project A would fire while the agent works
    /// inside project B (an isolation / arbitrary-command-execution leak).
    ///
    /// Returns `true` (always fires) for non-project hooks: global user hooks
    /// (`user:global`) and plugin-shipped hooks are not bound to any one
    /// directory. For project hooks, the hook's project root is compared
    /// against the effective workspace — the Panel-picked project
    /// ([`current_project_root`](crate::projects::current_project_root)) if a
    /// run is active, else the daemon CWD (plain server mode). When neither
    /// resolves, it fails open to preserve pre-project-mode behaviour.
    fn project_scope_allows(&self, hook: &HookConfig) -> bool {
        if !hook.plugin_name.starts_with("user:project") {
            return true;
        }
        // `<project>/.aleph/hooks*.json` → plugin_root is `<project>/.aleph`,
        // so the project root is its parent.
        let hook_project = match hook.plugin_root.parent() {
            Some(p) => p,
            None => return true,
        };
        let effective =
            crate::projects::current_project_root().or_else(|| std::env::current_dir().ok());
        match effective {
            Some(root) => paths_equal(&root, hook_project),
            None => true,
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
                self.execute_http(
                    url,
                    headers,
                    context,
                    plugin_root,
                    plugin_name,
                    event,
                    timeout_override,
                )
                .await
            }
            HookAction::Plugin { plugin_id, handler } => {
                self.execute_plugin(plugin_id, handler, context, event)
                    .await
            }
        }
    }

    /// Invoke a runtime plugin's exported hook handler via the process-global
    /// [`ExtensionManager`](crate::extension::ExtensionManager).
    ///
    /// Resolving the manager from the same global accessor the gateway/channel
    /// fire-sites already use ([`try_extension_manager`](crate::extension::try_extension_manager))
    /// keeps the executor free of a loader callback — and the `Arc` ownership
    /// cycle that threading one through every `HookExecutor` clone would create.
    /// When the manager is unregistered (e.g. unit tests construct a bare
    /// executor) the invoke is skipped with a non-success, no-output result:
    /// observer fire-sites ignore it and interceptors read empty output as
    /// "no effect", so a hookless test never blocks.
    async fn execute_plugin(
        &self,
        plugin_id: &str,
        handler: &str,
        context: &HookContext,
        event: HookEvent,
    ) -> Result<ActionResult, ExtensionError> {
        let Some(manager) = crate::extension::try_extension_manager() else {
            return Ok(ActionResult {
                success: false,
                output: None,
                error: Some("extension manager not initialized; plugin hook skipped".to_string()),
                exit_code: None,
            });
        };
        let payload = build_event_payload_value(event, context);
        match manager
            .execute_plugin_hook(plugin_id, handler, payload)
            .await
        {
            Ok(value) => Ok(ActionResult {
                success: true,
                // Surface the structured return as text so interceptor/resolver
                // paths can read line-prefixed directives via the same protocol
                // as Command/Http; observers ignore output entirely.
                output: match value {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(s) if s.is_empty() => None,
                    serde_json::Value::String(s) => Some(s),
                    other => serde_json::to_string(&other).ok(),
                },
                error: None,
                exit_code: None,
            }),
            Err(e) => Err(ExtensionError::HookExecution(format!(
                "plugin hook '{plugin_id}::{handler}' failed: {e}"
            ))),
        }
    }

    /// Effective timeout for a single hook execution (per-hook override or
    /// the executor default).
    fn effective_timeout(&self, override_secs: Option<u64>) -> Duration {
        override_secs.map_or(self.command_timeout, Duration::from_secs)
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
        debug!(plugin = plugin_name, event = ?event, "Executing hook command");

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

        // Kill the child when the timeout drops the wait future, so a hung
        // hook command does not keep running as an orphan past its deadline.
        cmd.kill_on_drop(true);

        // Set environment variables. `ARGUMENTS` / `TOOL_INPUT` mirror the
        // tool payload as a convenience for `$ARGUMENTS`-style scripts, but a
        // large payload (a Write tool's whole file body) can exceed the OS
        // `ARG_MAX` limit and make `spawn` fail with E2BIG — which, on an
        // interceptor seam, fails CLOSED and spuriously blocks the tool. The
        // canonical full-fidelity path is the stdin JSON (`jq -r
        // '.tool_input…'`, Claude-Code convention), so oversized values are
        // dropped from the env with a marker rather than risking the spawn.
        cmd.env("PLUGIN_ROOT", plugin_root);
        cmd.env("CLAUDE_PLUGIN_ROOT", plugin_root);
        if let Some(ref tool_name) = context.tool_name {
            cmd.env("TOOL_NAME", tool_name);
        }
        set_env_bounded(&mut cmd, "ARGUMENTS", context.arguments.as_deref());
        set_env_bounded(&mut cmd, "TOOL_INPUT", context.tool_input.as_deref());
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

        // Execute with timeout (per-hook override > executor default).
        // stdout/stderr reads are capped at `MAX_HOOK_OUTPUT_BYTES` (with the
        // remainder drained) so a hook that dumps megabytes can neither flood
        // memory nor deadlock on a full pipe.
        let effective = self.effective_timeout(timeout_override.map(|d| d.as_secs()));
        let (status, stdout_buf, stderr_buf) = match timeout(effective, async {
            let mut child = cmd.no_window().spawn().map_err(|e| {
                ExtensionError::HookExecution(format!("Failed to spawn command: {e}"))
            })?;
            let stdin_handle = child.stdin.take();
            let stdout_handle = child.stdout.take();
            let stderr_handle = child.stderr.take();
            // Write stdin CONCURRENTLY with the output reads: a payload
            // larger than the pipe buffer (e.g. a Write tool's whole file
            // content in `tool_input`) would otherwise deadlock against a
            // hook that fills its stdout first — surfacing as a spurious
            // timeout instead of a fast result.
            let (_, (stdout_buf, stdout_truncated), (stderr_buf, stderr_truncated)) = tokio::join!(
                async {
                    if let Some(mut stdin) = stdin_handle {
                        // Best-effort: if the hook never reads stdin, the
                        // write may fail with EPIPE — keep going.
                        let _ = stdin.write_all(payload.as_bytes()).await;
                        let _ = stdin.shutdown().await;
                    }
                },
                async {
                    match stdout_handle {
                        Some(h) => read_capped(h, MAX_HOOK_OUTPUT_BYTES).await,
                        None => (Vec::new(), false),
                    }
                },
                async {
                    match stderr_handle {
                        Some(h) => read_capped(h, MAX_HOOK_OUTPUT_BYTES).await,
                        None => (Vec::new(), false),
                    }
                }
            );
            let status = child.wait().await.map_err(|e| {
                ExtensionError::HookExecution(format!("Failed to await command: {e}"))
            })?;
            // A truncated stdout may have LOST a decision directive (`deny:`
            // printed after 64KB of diagnostics) — parsing the head and
            // reporting success would fail OPEN. Surface it as a hard error
            // instead: interceptor seams fail closed on it, observers log it.
            if stdout_truncated {
                return Err(ExtensionError::HookExecution(format!(
                    "hook stdout exceeded the {MAX_HOOK_OUTPUT_BYTES}-byte cap; decision \
                     directives may have been dropped — route diagnostics to stderr and \
                     keep stdout for the decision protocol"
                )));
            }
            if stderr_truncated {
                // stderr carries no directives — truncation is harmless noise.
                warn!("hook stderr exceeded the {MAX_HOOK_OUTPUT_BYTES}-byte cap; truncated");
            }
            Ok::<_, ExtensionError>((status, stdout_buf, stderr_buf))
        })
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                return Err(ExtensionError::HookExecution(format!(
                    "Command timed out after {effective:?}"
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&stdout_buf).to_string();
        let stderr = String::from_utf8_lossy(&stderr_buf).to_string();

        if !status.success() {
            warn!("Hook command exited with status {:?}", status.code());
        }

        Ok(ActionResult {
            success: status.success(),
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
            exit_code: status.code(),
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
    ///
    /// Gated by the same consent allowlist as shell commands: an HTTP hook
    /// ships the full event payload (tool inputs/outputs) to an arbitrary
    /// remote URL — an exfiltration vector every bit as serious as arbitrary
    /// code execution, so it must not run without operator approval either.
    /// The consent key is the RAW url template (pre-substitution), prefixed
    /// `http:` so it can't collide with a shell command of the same text.
    #[allow(clippy::too_many_arguments)]
    async fn execute_http(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        context: &HookContext,
        plugin_root: &Path,
        plugin_name: &str,
        event: HookEvent,
        timeout_override: Option<Duration>,
    ) -> Result<ActionResult, ExtensionError> {
        if let Some(consent) = &self.consent {
            let consent_key = format!("http:{url}");
            if !consent.is_approved(plugin_name, &consent_key) {
                consent.record_pending(plugin_name, &consent_key, &format!("{event:?}"));
                warn!(
                    plugin = plugin_name,
                    event = ?event,
                    "HTTP hook URL not approved — skipped. Review with `aleph hooks list`."
                );
                return Ok(ActionResult {
                    success: false,
                    output: None,
                    error: Some(format!(
                        "http hook from plugin '{plugin_name}' is not approved; \
                         run `aleph hooks test` to review and approve it"
                    )),
                    exit_code: None,
                });
            }
        }

        let resolved_url = substitute_variables(url, context, plugin_root);
        let payload = build_event_payload(event, context);
        let effective = self.effective_timeout(timeout_override.map(|d| d.as_secs()));

        let client = reqwest::Client::builder()
            .timeout(effective)
            .build()
            .map_err(|e| {
                ExtensionError::HookExecution(format!("Failed to build HTTP client: {e}"))
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
            Ok(mut resp) => {
                let status = resp.status();
                // Stream the body up to the shared output cap; a hook
                // endpoint returning megabytes must not flood memory or the
                // model context. A body that HITS the cap may have lost a
                // decision directive → hard error (fail closed), mirroring
                // the command path.
                let cap = MAX_HOOK_OUTPUT_BYTES as usize;
                let mut body_bytes: Vec<u8> = Vec::new();
                let mut body_truncated = false;
                while body_bytes.len() < cap {
                    match resp.chunk().await {
                        Ok(Some(chunk)) => {
                            let remaining = cap - body_bytes.len();
                            if chunk.len() > remaining {
                                body_truncated = true;
                            }
                            body_bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                        }
                        Ok(None) => break,
                        Err(e) => {
                            return Err(ExtensionError::HookExecution(format!(
                                "Failed to read hook HTTP response: {e}"
                            )));
                        }
                    }
                }
                // A chunk boundary can land exactly on the cap — probe once
                // more so "exactly 64KB then more" is not misread as complete.
                if !body_truncated && body_bytes.len() >= cap {
                    match resp.chunk().await {
                        Ok(Some(_)) => body_truncated = true,
                        Ok(None) => {}
                        Err(e) => {
                            return Err(ExtensionError::HookExecution(format!(
                                "Failed to read hook HTTP response: {e}"
                            )));
                        }
                    }
                }
                if body_truncated {
                    return Err(ExtensionError::HookExecution(format!(
                        "hook HTTP response body exceeded the {MAX_HOOK_OUTPUT_BYTES}-byte \
                         cap; decision directives may have been dropped"
                    )));
                }
                let body = String::from_utf8_lossy(&body_bytes).to_string();
                if !status.is_success() {
                    warn!("Hook HTTP response returned status {}", status);
                }
                Ok(ActionResult {
                    success: status.is_success(),
                    output: if body.is_empty() { None } else { Some(body) },
                    error: if status.is_success() {
                        None
                    } else {
                        Some(format!("HTTP {}", status.as_u16()))
                    },
                    exit_code: Some(i32::from(status.as_u16())),
                })
            }
            Err(e) => Ok(ActionResult {
                success: false,
                output: None,
                error: Some(format!("HTTP request failed: {e}")),
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

        let mut current_context = context;

        for hook in interceptors {
            // Check matcher pattern
            if !self.matches_pattern(hook, &current_context) {
                continue;
            }

            // Project-scoped hooks fire only in their own workspace.
            if !self.project_scope_allows(hook) {
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
                            HookAction::Command { .. }
                            | HookAction::Http { .. }
                            | HookAction::Plugin { .. } => {
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
                                // Mirror the observer path: deliver the
                                // delegation request to the LLM via the
                                // existing additional-context plumbing.
                                accumulated.additional_contexts.push(agent_invoke_directive(
                                    &hook.plugin_name,
                                    event,
                                    agent,
                                ));
                            }
                        }
                        accumulated.action_results.push(ar);
                    }
                    Err(e) => {
                        warn!("Interceptor hook action failed: {}", e);
                        // Interceptor failures block by default for safety.
                        // `action_failed` marks this as an infrastructure
                        // failure (not a hook decision) so fail-open seams
                        // (extension stop gate) can tell the two apart.
                        accumulated.blocked = true;
                        accumulated.block_reason = Some(format!("Interceptor hook failed: {e}"));
                        accumulated.action_failed = true;
                        return Ok((current_context, accumulated));
                    }
                }
            }

            // Thread this interceptor's input rewrite forward so the next
            // interceptor in the chain observes the updated arguments (the
            // documented "each hook receives the previous result" contract).
            // BOTH context fields must be rewritten: `arguments` feeds the
            // `$ARGUMENTS` env var, while `tool_input` feeds the stdin JSON
            // payload's `tool_input` key — the Claude-Code-convention path
            // (`jq -r '.tool_input…'`). Updating only `arguments` (the old
            // behaviour) silently handed downstream interceptors the ORIGINAL
            // input on stdin.
            if let Some(ref updated) = accumulated.updated_input {
                let rewritten = updated.to_string();
                current_context.arguments = Some(rewritten.clone());
                current_context.tool_input = Some(rewritten);
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
            .filter(|h| self.project_scope_allows(h))
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

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::types::HookEvent;
    use std::path::PathBuf;

    fn dummy_hook(plugin_name: &str) -> HookConfig {
        HookConfig {
            event: HookEvent::MessageReceived,
            kind: HookKind::Observer,
            priority: Default::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: "true".into(),
            }],
            plugin_name: plugin_name.into(),
            plugin_root: PathBuf::new(),
            handler: None,
            timeout_secs: None,
        }
    }

    #[test]
    fn remove_by_plugin_prefix_drops_only_matching_entries() {
        let mut exec = HookExecutor::empty();
        exec.add_hook(dummy_hook("user:global"));
        exec.add_hook(dummy_hook("user:project"));
        exec.add_hook(dummy_hook("plugin:foo"));
        assert_eq!(exec.hook_count(), 3);

        let removed = exec.remove_by_plugin_prefix("user:");
        assert_eq!(removed, 2);
        assert_eq!(exec.hook_count(), 1);
        assert_eq!(exec.hooks[0].plugin_name, "plugin:foo");
    }

    #[test]
    fn remove_by_plugin_prefix_is_a_noop_when_nothing_matches() {
        let mut exec = HookExecutor::empty();
        exec.add_hook(dummy_hook("plugin:foo"));
        assert_eq!(exec.remove_by_plugin_prefix("user:"), 0);
        assert_eq!(exec.hook_count(), 1);
    }

    /// A project hook carries its project's `.aleph` dir as `plugin_root`.
    fn project_hook(plugin_name: &str, project_root: &Path) -> HookConfig {
        let mut h = dummy_hook(plugin_name);
        h.plugin_root = project_root.join(".aleph");
        h
    }

    #[test]
    fn global_and_plugin_hooks_are_never_project_gated() {
        let exec = HookExecutor::empty();
        // No `with_project_root` scope active here, yet these must still fire.
        assert!(exec.project_scope_allows(&dummy_hook("user:global")));
        assert!(exec.project_scope_allows(&dummy_hook("plugin:foo")));
    }

    #[tokio::test]
    async fn project_hook_fires_only_in_its_own_workspace() {
        let proj_a = tempfile::tempdir().unwrap();
        let proj_b = tempfile::tempdir().unwrap();
        let exec = HookExecutor::empty();
        let hook_a = project_hook("user:project", proj_a.path());

        // Active project == the hook's project → fires.
        let in_a = crate::projects::with_project_root(Some(proj_a.path().to_path_buf()), async {
            exec.project_scope_allows(&hook_a)
        })
        .await;
        assert!(in_a, "project hook must fire inside its own project");

        // A different project is active → suppressed (no cross-project leak).
        let in_b = crate::projects::with_project_root(Some(proj_b.path().to_path_buf()), async {
            exec.project_scope_allows(&hook_a)
        })
        .await;
        assert!(!in_b, "project hook must NOT fire inside another project");
    }

    #[test]
    fn plugin_action_round_trips_through_serde() {
        // Wire-format lock for the variant `sync_hooks_from_registry` emits.
        let action = HookAction::Plugin {
            plugin_id: "demo".into(),
            handler: "onEvent".into(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("\"type\":\"plugin\""), "got: {json}");
        let back: HookAction = serde_json::from_str(&json).unwrap();
        match back {
            HookAction::Plugin { plugin_id, handler } => {
                assert_eq!(plugin_id, "demo");
                assert_eq!(handler, "onEvent");
            }
            other => panic!("expected Plugin action, got {other:?}"),
        }
    }

    #[cfg(unix)]
    fn interceptor_command_hook(command: &str) -> HookConfig {
        HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: Default::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: command.into(),
            }],
            plugin_name: "chain-test".into(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn oversized_stdout_fails_closed_not_open() {
        // A hook that prints >64KB then `deny:` must NOT report success with
        // truncated head text (which would drop the deny → fail OPEN). The
        // action errors instead, and the interceptor seam converts that to a
        // fail-closed block with `action_failed` set.
        use crate::extension::hooks::HookContext;
        let big = interceptor_command_hook(
            "head -c 100000 /dev/zero | tr '\\0' 'x'; echo; echo 'deny: too late'",
        );
        let executor = HookExecutor::new(vec![big]);
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("interceptor pass returns Ok even on action error");
        assert!(result.blocked, "truncated-output hook must fail closed");
        assert!(
            result.action_failed,
            "must be flagged as an infrastructure failure, not a hook decision"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn oversized_stdin_does_not_deadlock() {
        // A stdin payload larger than the OS pipe buffer (~64KB) must be
        // written CONCURRENTLY with the output drain — a hook that ignores
        // stdin and prints a little must still complete fast, not hang until
        // the timeout. 120KB clears the pipe buffer while staying well under
        // ARG_MAX (the executor also mirrors tool_input into a TOOL_INPUT env
        // var, and an env var near ARG_MAX would fail the spawn on its own).
        use crate::extension::hooks::HookContext;
        let hook = interceptor_command_hook("echo 'context: ok'");
        let executor = HookExecutor::new(vec![hook]);
        let big_input = "x".repeat(120 * 1024);
        let ctx = HookContext::new("s")
            .with_tool_name("Write")
            .with_tool_input(&big_input);
        let start = std::time::Instant::now();
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, ctx)
            .await
            .expect("must not hang");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "oversized stdin must not deadlock (took {:?})",
            start.elapsed()
        );
        assert!(
            result.additional_contexts.iter().any(|c| c == "ok"),
            "hook must run to completion despite the large stdin payload"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn interceptor_chain_propagates_rewrite_to_stdin_payload() {
        // Regression lock for the chain contract: hook 1 rewrites the tool
        // input; hook 2 must observe the REWRITTEN value in its stdin JSON
        // payload (`tool_input` key — the Claude-Code `jq` convention), not
        // just in the `$ARGUMENTS` env var. Before the fix only `arguments`
        // was threaded forward, so stdin carried the original input.
        use crate::extension::hooks::HookContext;
        let rewriter =
            interceptor_command_hook(r#"echo 'update_input: {"path":"/rewritten"}'"#);
        let checker = interceptor_command_hook(
            r#"input=$(cat); echo "$input" | grep -q '/rewritten' && echo 'context: saw-rewrite' || echo 'context: saw-original'"#,
        );
        let executor = HookExecutor::new(vec![rewriter, checker]);
        let ctx = HookContext::new("chain")
            .with_tool_name("Write")
            .with_arguments(r#"{"path":"/original"}"#)
            .with_tool_input(r#"{"path":"/original"}"#);

        let (final_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, ctx)
            .await
            .expect("chain must run");

        assert_eq!(
            result.updated_input,
            Some(serde_json::json!({"path": "/rewritten"}))
        );
        assert!(
            result
                .additional_contexts
                .iter()
                .any(|c| c == "saw-rewrite"),
            "second interceptor must see the rewrite on stdin: {:?}",
            result.additional_contexts
        );
        // Both context fields carry the rewrite forward.
        assert_eq!(
            final_ctx.tool_input.as_deref(),
            Some(r#"{"path":"/rewritten"}"#)
        );
        assert_eq!(
            final_ctx.arguments.as_deref(),
            Some(r#"{"path":"/rewritten"}"#)
        );
    }

    #[tokio::test]
    async fn plugin_action_is_dispatched_and_skips_without_manager() {
        // Regression lock: a `HookAction::Plugin` must reach `execute_plugin`
        // (the wiring this commit adds) instead of being a silent no-op. With
        // no process-global ExtensionManager registered, the invoke skips with
        // a non-success, no-output result rather than panicking. Guarded so a
        // sibling test that boots the manager can't make this non-deterministic.
        if crate::extension::is_extension_manager_initialized() {
            return;
        }
        let exec = HookExecutor::empty();
        let ctx = HookContext::new("sess");
        let result = exec
            .execute_action(
                &HookAction::Plugin {
                    plugin_id: "demo".into(),
                    handler: "onEvent".into(),
                },
                &ctx,
                &PathBuf::new(),
                "plugin:demo",
                HookEvent::MessageReceived,
                None,
            )
            .await
            .expect("plugin action must skip (not error) when manager is absent");
        assert!(!result.success);
        assert!(result.output.is_none());
    }
}
