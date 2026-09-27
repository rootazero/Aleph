//! `HookExecutor` implementation — action dispatch and execution logic

use super::session_facts::SessionFacts;
use super::{
    substitute_path_variables, substitute_variables, ActionResult, HookContext, ShellHookConsent,
    DEFAULT_COMMAND_TIMEOUT_SECS, MAX_HOOK_TIMEOUT_SECS, PLUGIN_DATA_VARIABLES,
    PLUGIN_ROOT_VARIABLES,
};
use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind};
use crate::extension::visibility::ScopeKey;
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
/// output to disk; a hard cap is the minimal Aleph equivalent. A plugin
/// command's inline shell output (`slash_command_body`) is capped by the
/// same number: it is prompt text too.
pub(crate) const MAX_HOOK_OUTPUT_BYTES: u64 = 64 * 1024;

/// Claude Code's PostToolUse result key, carrying the same output as the
/// Aleph-native `tool_output` — as the tool's structure when it has one (an
/// object in the capture), as text otherwise ([`tool_response_value`]).
/// Settled by the P0 live capture (2026-09-20): the official hooks reference
/// and dsh's `hooks-claude-code` port both spell it `tool_response`; the
/// plugin-dev skill's prose (`tool_result`) is stale. This constant is the
/// one production spelling. This module's payload tests read the key through
/// it; the real-tool-call test in `tools::scoped` spells it out literally, so
/// changing the constant fails that test instead of silently following.
pub(crate) const CC_POST_TOOL_RESULT_KEY: &str = "tool_response";

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

/// Max bytes for a single payload-mirroring env var (a hook's `ARGUMENTS` /
/// `TOOL_INPUT`, an inline command's `ARGUMENTS`). Comfortably under every
/// platform's `ARG_MAX` (Linux 128KB per string, macOS 256KB total) so
/// setting it can never push the spawn over the limit. The value stays
/// readable elsewhere — the marker says where.
const MAX_ENV_VALUE_BYTES: usize = 32 * 1024;

/// `value` for the env var `key`, or a placeholder for any value past
/// [`MAX_ENV_VALUE_BYTES`] — an oversized env var can make `spawn` fail with
/// E2BIG, which on an interceptor seam fails closed and blocks the tool, and
/// for an inline command fails every command in the body. `read_instead`
/// names where the child can still read it (a hook: `stdin JSON`; an inline
/// command: its positional parameters), and is all the marker says.
pub(crate) fn bounded_env_value(key: &str, value: &str, read_instead: &str) -> String {
    if value.len() <= MAX_ENV_VALUE_BYTES {
        return value.to_string();
    }
    debug!(
        key,
        len = value.len(),
        read_instead,
        "env value exceeds cap; replaced by a marker"
    );
    format!("[{} bytes — read from {read_instead}]", value.len())
}

/// A command hook's child process, derived once from the action: the shell
/// line, the directory, the environment and the stdin JSON.
///
/// Production ([`HookExecutor`]'s command action) and `aleph-server hooks test` both
/// spawn from this value, so a script an operator reviews there sees the same
/// path substitution, the same data variables and the same payload it will
/// get in production. Production adds one thing: the plugin's runtime
/// settings (`ALEPH_PLUGIN_OPTION_*`), which need the running extension
/// manager.
#[derive(Debug, Clone)]
pub struct CommandHookInvocation {
    /// The shell (`sh` / `cmd`).
    pub program: &'static str,
    /// Its "run this line" flag (`-c` / `/C`).
    pub flag: &'static str,
    /// The command as the shell receives it: verbatim on unix, where every
    /// variable reaches the child through [`env`](Self::env); with the path
    /// variables substituted — and nothing else — on Windows, where `cmd`
    /// cannot expand `${…}`.
    pub line: String,
    /// Where the child runs: the context's `working_dir`, else the hook's
    /// plugin root. `None` only when neither is known.
    pub current_dir: Option<std::path::PathBuf>,
    /// Environment changes, in the order they are applied. `Some` sets the
    /// variable; `None` removes it, so a fixed field or path variable this
    /// hook does not have is never inherited from the daemon's own
    /// environment. An event `env` key appears only when the event carries
    /// it; a name the event lacks is not listed, and so is inherited.
    pub env: Vec<(String, Option<std::ffi::OsString>)>,
    /// The event as JSON, written to the child's stdin.
    pub stdin: String,
}

/// Derive a command hook's [`CommandHookInvocation`].
///
/// `event_name` is the spelling the hook receives on `hook_event_name`
/// ([`HookConfig::event_name`]). `plugin_root` is `None` only for a consent
/// entry recorded before the root was kept: the path variables are then
/// unset rather than given an invented directory.
#[must_use]
pub fn command_hook_invocation(
    command: &str,
    event_name: &str,
    context: &HookContext,
    plugin_root: Option<&Path>,
    plugin_name: &str,
) -> CommandHookInvocation {
    use std::ffi::OsString;
    // Derived once, so the stdin payload and the environment read the same
    // answer.
    let facts = SessionFacts::derive(context);
    // The data directory: plugin-owned hooks only — a settings source label
    // such as `user:project` is not a plugin id and has none.
    let plugin_vars = plugin_root
        .filter(|_| crate::extension::manifest::validate_plugin_id(plugin_name).is_ok())
        .map(|root| crate::extension::plugin_vars::PluginVars::new(plugin_name, root));

    // What the shell parses. On unix: the command as written. Every
    // variable — the path variables included — reaches the child through
    // the environment below, and `sh` expands it as one word of data. The
    // data variables (`$ARGUMENTS`, `$DENY_REASON`, …) are model-controlled
    // or quote identifiers in backticks, and a path variable's value is a
    // directory name — `fmt$(…)`, `Jane Doe` — so either one spliced into
    // the source would be parsed, and consent approves the template, not the
    // resolved string. On Windows `cmd` cannot expand `${…}`, so the path
    // variables (never the data) are substituted into the line: the one
    // platform difference. `cmd` would expand a `%VAR%` before parsing, so
    // data is read from the stdin JSON there.
    let line = if cfg!(windows) {
        plugin_root.map_or_else(
            || command.to_string(),
            |root| substitute_path_variables(command, root, plugin_name),
        )
    } else {
        // `substitute_path_variables` creates the data directory when the
        // template names it; with no splice that is done here, from the same
        // template text.
        if let Some(vars) = &plugin_vars {
            vars.ensure_data_dir_if_referenced(command);
        }
        command.to_string()
    };
    let (program, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };

    let mut env: Vec<(String, Option<OsString>)> = Vec::new();
    let mut set = |key: &str, value: Option<OsString>| env.push((key.to_string(), value));

    // Set environment variables. These are the only route by which the data
    // variables (`TOOL_NAME`, `ARGUMENTS`, `TOOL_INPUT`, `SESSION_ID`, every
    // `context.env` key) reach a command — none is substituted into
    // its source (above). `ARGUMENTS` / `TOOL_INPUT` mirror the tool payload,
    // but a large payload (a Write tool's whole file body) can exceed the OS
    // `ARG_MAX` limit and make `spawn` fail with E2BIG — which, on an
    // interceptor seam, fails CLOSED and spuriously blocks the tool. The
    // canonical full-fidelity path is the stdin JSON (`jq -r '.tool_input…'`,
    // Claude-Code convention), so oversized values are replaced in the env by
    // a marker rather than risking the spawn — and the marker is all a
    // `"$ARGUMENTS"` script then sees.
    //
    // The path variables, every spelling the substitution knows — on unix
    // the only route by which they reach a command. Each is set when known
    // and removed when not, never inherited: a daemon launched from inside a
    // Claude Code plugin exports its own `CLAUDE_PLUGIN_ROOT` /
    // `CLAUDE_PLUGIN_DATA`, which would otherwise read as this hook's.
    let root = plugin_root.map(OsString::from);
    for name in PLUGIN_ROOT_VARIABLES {
        set(name, root.clone());
    }
    // Claude Code's project-directory variable (`$CLAUDE_PROJECT_DIR`), the
    // one every CC hook script reaches for first: `facts.cwd`, the value the
    // payload's `cwd` is written from. Removed rather than inherited when
    // unknown, so a daemon launched from inside a Claude Code session cannot
    // hand its own value to every hook.
    set("CLAUDE_PROJECT_DIR", facts.cwd.clone().map(Into::into));
    // The durable half. `CLAUDE_PLUGIN_ROOT` is destroyed by `plugin update`
    // (stage → backup → swap), so a hook that wants state that outlives an
    // upgrade had no addressable path until this line existed. Removed for a
    // hook with no data directory (a settings hook).
    let data = plugin_vars.as_ref().map(|v| OsString::from(v.data_dir()));
    for name in PLUGIN_DATA_VARIABLES {
        set(name, data.clone());
    }
    // A fixed data field this event does not carry is removed, not left to
    // the daemon's environment: a daemon started with `TOOL_NAME` or `ARGUMENTS`
    // exported would otherwise hand that value to every hook as if it were
    // the event's (the `CLAUDE_PROJECT_DIR` rule above, for the data half).
    // The `context.env` keys below have no such list: each is set when the
    // event carries it, and a name it lacks is inherited.
    set("TOOL_NAME", context.tool_name.clone().map(Into::into));
    set(
        "ARGUMENTS",
        context
            .arguments
            .as_deref()
            .map(|v| bounded_env_value("ARGUMENTS", v, "stdin JSON").into()),
    );
    set(
        "TOOL_INPUT",
        context
            .tool_input
            .as_deref()
            .map(|v| bounded_env_value("TOOL_INPUT", v, "stdin JSON").into()),
    );
    set("SESSION_ID", Some(context.session_id.clone().into()));
    // The event's own variables, last: a key here (e.g. `TOOL_NAME` from
    // `fire_observer`) overrides a removal above.
    for (key, value) in &context.env {
        set(key, Some(value.into()));
    }

    CommandHookInvocation {
        program,
        flag,
        line,
        current_dir: context
            .working_dir
            .clone()
            .or_else(|| plugin_root.map(Path::to_path_buf)),
        env,
        stdin: build_event_payload(event_name, context, &facts),
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
/// `{ hook_event_name, session_id, tool_name?, tool_input?, tool_output?, <CC_POST_TOOL_RESULT_KEY>?, tool_error?, cwd?, transcript_path?, permission_mode?, env? }`
///
/// Shared by the stdin/HTTP string form ([`build_event_payload`]) and the
/// plugin-hook path, which passes the value straight to `execute_plugin_hook`
/// without a string round-trip.
///
/// Takes the event's NAME rather than the [`HookEvent`] enum: the spelling a
/// hook sees on `hook_event_name` is whatever it was registered under —
/// the dispatch loops pass [`HookConfig::event_name`] (the declared
/// spelling, else `HookEvent::canonical_name`).
///
/// `facts` are the session facts the executor derived for this action
/// ([`SessionFacts::derive`]) from what the run publishes; the only input a
/// fire site could add is an explicit `working_dir`, and no production fire
/// site sets one.
fn build_event_payload_value(
    event_name: &str,
    context: &HookContext,
    facts: &SessionFacts,
) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    let mut payload: Map<String, Value> = Map::new();
    payload.insert(
        "hook_event_name".into(),
        Value::String(event_name.to_string()),
    );
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
        payload.insert(CC_POST_TOOL_RESULT_KEY.into(), tool_response_value(o));
    }
    if let Some(e) = context.tool_error {
        payload.insert("tool_error".into(), Value::Bool(e));
    }
    // Omitted when unknown (outside a run) — see `SessionFacts::cwd`.
    if let Some(c) = &facts.cwd {
        payload.insert("cwd".into(), Value::String(c.to_string_lossy().to_string()));
    }
    if let Some(t) = &facts.transcript_path {
        payload.insert(
            "transcript_path".into(),
            Value::String(t.to_string_lossy().to_string()),
        );
    }
    if let Some(m) = context.permission_mode {
        payload.insert("permission_mode".into(), Value::String(m.to_string()));
    }
    if !context.env.is_empty() {
        payload.insert("env".into(), json!(context.env));
    }
    Value::Object(payload)
}

/// Claude Code's `tool_response` for one tool output: the tool's structured
/// answer where there is one (an object in the P0 capture, so
/// `jq '.tool_response.file'` works), else the text.
///
/// The tool-dispatch seam hands over `Value::to_string()` of the result the
/// model saw, and after the result budget that value is a JSON string holding
/// the model-facing text — so the text is decoded first, and a text that is
/// itself a JSON object or array is the structure. A scalar-looking text
/// (`42`, `true`) stays text: nothing says the tool meant a number. Output
/// that is not JSON at all (a failure's error text, an agent's final reply)
/// is the text as given.
fn tool_response_value(output: &str) -> serde_json::Value {
    use serde_json::Value;
    match serde_json::from_str::<Value>(output) {
        Ok(Value::String(text)) => match serde_json::from_str::<Value>(&text) {
            Ok(structured @ (Value::Object(_) | Value::Array(_))) => structured,
            _ => Value::String(text),
        },
        Ok(structured @ (Value::Object(_) | Value::Array(_))) => structured,
        _ => Value::String(output.to_string()),
    }
}

/// Build a Claude Code-style event payload JSON string for stdin / HTTP body.
/// `aleph-server hooks test` reaches it through [`command_hook_invocation`], so a
/// hook exercised there reads the exact payload production sends.
fn build_event_payload(event_name: &str, context: &HookContext, facts: &SessionFacts) -> String {
    serde_json::to_string(&build_event_payload_value(event_name, context, facts))
        .unwrap_or_else(|_| "{}".to_string())
}

/// Short, human-readable label for one hook action, used by the runtime
/// inventory. Long commands / URLs are elided — the inventory is a "what is
/// wired up" listing, not a config dump; `hooks.list` still returns the
/// verbatim file for editing.
fn describe_action(action: &HookAction) -> String {
    /// Keep labels to one terminal line.
    const LABEL_CAP: usize = 80;
    let (kind, detail) = match action {
        HookAction::Command { command } => ("command", command.as_str()),
        HookAction::Prompt { prompt } => ("prompt", prompt.as_str()),
        HookAction::Agent { agent } => ("agent", agent.as_str()),
        HookAction::Http { url, .. } => ("http", url.as_str()),
        HookAction::Plugin { plugin_id, handler } => {
            return format!("plugin: {plugin_id}::{handler}")
        }
    };
    format!(
        "{kind}: {}",
        crate::utils::text_format::truncate_text(detail, LABEL_CAP)
    )
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
    /// `aleph-server hooks` CLI.
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

    /// The registered hooks, for tests outside this module that assert on a
    /// field `inventory()` deliberately does not show (`declared_event`).
    #[cfg(test)]
    pub(crate) fn hook_configs_for_test(&self) -> &[HookConfig] {
        &self.hooks
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

        // Test the regex against the Aleph name AND every Claude Code spelling
        // of it (`Edit` for `file_edit`, `mcp__srv__tool` for `srv__tool`), so
        // a matcher copied from a CC `settings.json` selects the tool it names.
        let candidates: Vec<String> = std::iter::once(tool_name.clone())
            .chain(super::cc_spellings(tool_name))
            .collect();
        let hit = |re: &regex::Regex| candidates.iter().any(|c| re.is_match(c));

        // Look up compiled regex from cache
        match self.regex_cache.get(matcher.as_str()) {
            Some(Some(re)) => hit(re),
            Some(None) => false, // Invalid regex, logged at cache time
            None => {
                // Fallback: compile on the fly (should not happen if add_hook was used)
                match crate::security::safe_regex::bounded_builder(matcher).build() {
                    Ok(re) => hit(&re),
                    Err(e) => {
                        warn!("Invalid hook matcher regex '{}': {}", matcher, e);
                        false
                    }
                }
            }
        }
    }

    /// Gate a hook to the sessions that may see it.
    ///
    /// Every hook carries a [`ScopeKey`](crate::extension::visibility::ScopeKey)
    /// stamped by its producer; this is the hook face of the one visibility
    /// predicate ([`visible_to`](crate::extension::visibility::visible_to)) the
    /// five capability faces share: tool index, skills, sub-agents, slash list
    /// and MCP (the request-time MCP join, the capability builtins it binds,
    /// and the MCP catalog / slash rows). The daemon serves every registered
    /// project from one process, so all project hooks live in one executor —
    /// without this gate a hook checked into project A would fire while the
    /// agent works inside project B (an isolation / arbitrary-command-execution
    /// leak).
    ///
    /// `ctx` is computed once per fire-site call, not per hook, so a batch of
    /// interceptors is judged against one answer.
    fn project_scope_allows(
        &self,
        hook: &HookConfig,
        ctx: &crate::extension::visibility::VisibilityCtx,
    ) -> bool {
        crate::extension::visibility::visible_to(&hook.scope_key, ctx)
    }

    /// Execute a single action.
    ///
    /// `timeout_override` lets the per-hook `timeout_secs` setting take
    /// precedence over the executor's default. Applies to Command/Http.
    async fn execute_action(
        &self,
        action: &HookAction,
        context: &HookContext,
        plugin_root: &Path,
        plugin_name: &str,
        scope_key: &ScopeKey,
        event: HookEvent,
        event_name: &str,
        timeout_override: Option<Duration>,
    ) -> Result<ActionResult, ExtensionError> {
        match action {
            HookAction::Command { command } => {
                self.execute_command(
                    command,
                    context,
                    plugin_root,
                    plugin_name,
                    scope_key,
                    event,
                    event_name,
                    timeout_override,
                )
                .await
            }
            HookAction::Prompt { prompt } => {
                self.execute_prompt(prompt, context, plugin_root, plugin_name)
                    .await
            }
            HookAction::Agent { agent } => self.execute_agent(agent).await,
            HookAction::Http { url, headers } => {
                self.execute_http(
                    url,
                    headers,
                    context,
                    plugin_root,
                    plugin_name,
                    scope_key,
                    event,
                    event_name,
                    timeout_override,
                )
                .await
            }
            HookAction::Plugin { plugin_id, handler } => {
                self.execute_plugin(plugin_id, handler, context, event_name)
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
        event_name: &str,
    ) -> Result<ActionResult, ExtensionError> {
        let Some(manager) = crate::extension::try_extension_manager() else {
            return Ok(ActionResult {
                success: false,
                output: None,
                error: Some("extension manager not initialized; plugin hook skipped".to_string()),
                exit_code: None,
            });
        };
        let payload =
            build_event_payload_value(event_name, context, &SessionFacts::derive(context));
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
    ///
    /// The override is clamped to [`MAX_HOOK_TIMEOUT_SECS`]. This is the ONLY
    /// place a `timeout_secs` declaration becomes a real deadline, so clamping
    /// here covers every source (`~/.aleph/hooks.json`, project hooks, plugin
    /// `hooks.json`, `aleph.plugin.toml`) without a per-loader guard. A zero
    /// override is treated as "unset" — `Duration::ZERO` would make every hook
    /// time out instantly.
    fn effective_timeout(&self, override_secs: Option<u64>) -> Duration {
        match override_secs.filter(|s| *s > 0) {
            Some(secs) => {
                if secs > MAX_HOOK_TIMEOUT_SECS {
                    warn!(
                        requested = secs,
                        clamped_to = MAX_HOOK_TIMEOUT_SECS,
                        "hook timeout_secs exceeds the ceiling; clamping"
                    );
                }
                Duration::from_secs(secs.min(MAX_HOOK_TIMEOUT_SECS))
            }
            None => self.command_timeout,
        }
    }

    /// Execute a shell command
    async fn execute_command(
        &self,
        command: &str,
        context: &HookContext,
        plugin_root: &Path,
        plugin_name: &str,
        scope_key: &ScopeKey,
        event: HookEvent,
        event_name: &str,
        timeout_override: Option<Duration>,
    ) -> Result<ActionResult, ExtensionError> {
        // Shell-hook consent gate: an un-approved command must not run. It is
        // recorded as `pending` (so `aleph-server hooks list` surfaces it) and the
        // action returns a non-success result with no output — interceptors
        // treat empty output as "no effect", so a skipped hook never blocks
        // the tool call. Approving arbitrary code execution is the operator's
        // explicit decision, not a default.
        if let Some(consent) = &self.consent {
            // Keyed by the hook's `scope_key` — the field
            // `project_scope_allows` reads — so a project hook's approval is
            // its project's alone (`consent.rs`, module doc).
            if !consent.is_approved(plugin_name, scope_key, plugin_root, command) {
                // What `aleph-server hooks test` rebuilds the run from: the spelling
                // this hook is dispatched with and the root its path
                // variables resolve to.
                consent.record_pending(plugin_name, scope_key, command, event_name, plugin_root);
                warn!(
                    plugin = plugin_name,
                    event = ?event,
                    scope = ?scope_key,
                    "Shell hook command not approved — skipped. Review with `aleph-server hooks list`."
                );
                return Ok(ActionResult {
                    success: false,
                    output: None,
                    error: Some(format!(
                        "shell hook from plugin '{plugin_name}' is not approved; \
                         run `aleph-server hooks test` to review and approve it"
                    )),
                    exit_code: None,
                });
            }
        }

        // Shell line, directory, environment and stdin payload: one derivation,
        // shared with `aleph-server hooks test` so a reviewed script runs as it does
        // here.
        let invocation =
            command_hook_invocation(command, event_name, context, Some(plugin_root), plugin_name);
        debug!(plugin = plugin_name, event = ?event, "Executing hook command");

        let mut cmd = Command::new(invocation.program);
        cmd.args([invocation.flag, invocation.line.as_str()]);
        if let Some(dir) = &invocation.current_dir {
            cmd.current_dir(dir);
        }

        // Kill the child when the timeout drops the wait future, so a hung
        // hook command does not keep running as an orphan past its deadline.
        cmd.kill_on_drop(true);

        for (key, value) in &invocation.env {
            match value {
                Some(value) => {
                    cmd.env(key, value);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
        // The operator's configuration for this plugin, in the same env
        // spelling the plugin's MCP servers get. A hook and an MCP server from
        // one plugin reading the same setting under two different names would
        // be two conventions for one fact. Plugin-owned hooks only (the gate
        // is inside `plugin_settings_env`, shared with inline commands).
        if let Some(manager) = crate::extension::try_extension_manager() {
            // Runtime form: these values become the hook subprocess's
            // environment, so a `{{secret:NAME}}` reference resolves here.
            // The display faces deliberately keep the placeholder, and an
            // inline command gets none — see `crate::extension::plugin_secrets`.
            let form = crate::extension::plugin_secrets::SettingsForm::Runtime;
            for (key, value) in manager.plugin_settings_env(plugin_name, form).await {
                cmd.env(key, value);
            }
        }

        // Configure stdio. The event JSON payload is piped to stdin so
        // hook scripts can `jq -r '.tool_input.file_path'` (Claude Code
        // convention). The env vars above carry the same data for
        // `"$VAR"`-style scripts on unix.
        let payload = invocation.stdin;
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

        // No generic non-success log here: `derive_decision` is the ONE
        // owner of exit-code interpretation, including its log line — a
        // non-zero exit is either a hook DECISION (exit 2, no log needed on
        // the interceptor path; logged on the observer path since that seam
        // cannot act on it) or a non-blocking error (`derive_decision`'s own
        // `Some(code) => warn!`). Logging here too produced a duplicate line
        // for every non-zero exit.
        //
        // `exit_code` is consumed by `derive_decision` at both call sites
        // (`execute_interceptors` / `execute_observers`) — exit 2 blocks an
        // interceptor even when stdout is empty (the CC `>&2; exit 2` idiom).
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
        owner: &str,
    ) -> Result<ActionResult, ExtensionError> {
        let resolved = substitute_variables(prompt, context, plugin_root, owner);

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
        scope_key: &ScopeKey,
        event: HookEvent,
        event_name: &str,
        timeout_override: Option<Duration>,
    ) -> Result<ActionResult, ExtensionError> {
        if let Some(consent) = &self.consent {
            let consent_key = format!("http:{url}");
            if !consent.is_approved(plugin_name, scope_key, plugin_root, &consent_key) {
                consent.record_pending(
                    plugin_name,
                    scope_key,
                    &consent_key,
                    event_name,
                    plugin_root,
                );
                warn!(
                    plugin = plugin_name,
                    event = ?event,
                    scope = ?scope_key,
                    "HTTP hook URL not approved — skipped. Review with `aleph-server hooks list`."
                );
                return Ok(ActionResult {
                    success: false,
                    output: None,
                    error: Some(format!(
                        "http hook from plugin '{plugin_name}' is not approved; \
                         run `aleph-server hooks test` to review and approve it"
                    )),
                    exit_code: None,
                });
            }
        }

        let resolved_url = substitute_variables(url, context, plugin_root, plugin_name);
        let payload = build_event_payload(event_name, context, &SessionFacts::derive(context));
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
            let resolved_v = substitute_variables(v, context, plugin_root, plugin_name);
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
        let visibility = crate::extension::visibility::VisibilityCtx::for_session();

        for hook in interceptors {
            // Check matcher pattern
            if !self.matches_pattern(hook, &current_context) {
                continue;
            }

            // Project-scoped hooks fire only in their own workspace.
            if !self.project_scope_allows(hook, &visibility) {
                continue;
            }

            debug!(
                "Executing interceptor hook from plugin '{}' for event {:?}",
                hook.plugin_name, event
            );
            accumulated.hooks_executed += 1;

            // The name this hook sees on `hook_event_name`: the spelling it
            // was registered under, so it is per hook, not per call.
            let event_name = hook.event_name();

            // Execute all actions for this hook
            for action in &hook.actions {
                let action_result = self
                    .execute_action(
                        action,
                        &current_context,
                        &hook.plugin_root,
                        &hook.plugin_name,
                        &hook.scope_key,
                        event,
                        &event_name,
                        hook.timeout_secs.map(Duration::from_secs),
                    )
                    .await;

                match action_result {
                    Ok(ar) => {
                        match action {
                            HookAction::Command { .. } => {
                                // Exit code, stdout and stderr go through the
                                // ONE derivation; exit 2 blocks here even when
                                // stdout is empty (the CC `>&2; exit 2` idiom).
                                super::derive_decision(
                                    ar.exit_code,
                                    ar.output.as_deref().unwrap_or(""),
                                    ar.error.as_deref().unwrap_or(""),
                                    HookKind::Interceptor,
                                    &mut accumulated,
                                );
                                if accumulated.blocked || accumulated.denied {
                                    return Ok((current_context, accumulated));
                                }
                            }
                            HookAction::Http { .. } | HookAction::Plugin { .. } => {
                                // `ActionResult::exit_code` is the HTTP status
                                // here (`Some(200)`), not a process code — pass
                                // `None` so the transport's own success verdict
                                // stands and the body is read as the decision,
                                // exactly as before.
                                super::derive_decision(
                                    None,
                                    ar.output.as_deref().unwrap_or(""),
                                    "",
                                    HookKind::Interceptor,
                                    &mut accumulated,
                                );
                                if accumulated.blocked || accumulated.denied {
                                    return Ok((current_context, accumulated));
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

    /// Every registered hook, as the running server sees it.
    ///
    /// Answers "what is actually wired up, and will it fire?" — the question
    /// the `hooks.list` file view structurally cannot, because it only ever
    /// reads `~/.aleph/hooks.json` and therefore misses project and
    /// plugin-shipped hooks, the resolved `kind`, and both reachability
    /// foot-guns. Sorted by (event, priority) so the output reads in roughly
    /// the order hooks would run.
    #[must_use]
    pub fn inventory(&self) -> Vec<super::HookInventoryEntry> {
        let mut out: Vec<_> = self.hooks.iter().map(|h| self.describe(h)).collect();
        out.sort_by(|a, b| a.event.cmp(&b.event).then(a.priority.cmp(&b.priority)));
        out
    }

    /// Build the inventory row for one hook.
    fn describe(&self, hook: &HookConfig) -> super::HookInventoryEntry {
        let kind = match hook.kind {
            HookKind::Interceptor => "interceptor",
            HookKind::Observer => "observer",
        };

        // Reachability mirrors the two load-time foot-gun warnings, reading
        // the SAME predicates on `HookEvent` so the two can never disagree.
        let (reachable, issue) = if hook.matcher.is_some() && !hook.event.supports_matcher() {
            (
                false,
                Some(
                    "`matcher` is set on an event that carries no tool name; matchers test \
                     tool_name only, so this hook never fires. Drop the matcher."
                        .to_string(),
                ),
            )
        } else if hook.kind == HookKind::Interceptor && !hook.event.supports_interceptor() {
            (
                false,
                Some(
                    "kind is `interceptor` but this event's fire-site dispatches observers \
                     only, so this hook never executes. Use `\"kind\": \"observer\"`."
                        .to_string(),
                ),
            )
        } else {
            (true, None)
        };

        super::HookInventoryEntry {
            source: hook.plugin_name.clone(),
            // Canonical, not the declared spelling: a diagnostic view, not
            // the wire a script reads.
            event: hook.event.canonical_name(),
            kind: kind.to_string(),
            priority: format!("{:?}", hook.priority).to_lowercase(),
            matcher: hook.matcher.clone(),
            actions: hook.actions.iter().map(describe_action).collect(),
            timeout_secs: hook.timeout_secs,
            // Same fact the fire-time gate reads (`project_scope_allows`), so
            // the inventory can never show a hook as unbound while the gate
            // suppresses it. Canonical spelling, as the key stores it.
            project_root: match &hook.scope_key {
                crate::extension::visibility::ScopeKey::Global => None,
                crate::extension::visibility::ScopeKey::Project(p) => Some(p.display().to_string()),
            },
            reachable,
            issue,
            consent: self.consent_state(hook),
        }
    }

    /// Consent state for a hook's gated actions (`command` / `http`).
    ///
    /// `pending` wins over `approved`: a hook is only fully live when EVERY
    /// gated action it owns is approved, and reporting the weakest link is
    /// what makes "my hook doesn't run" diagnosable. `None` means nothing to
    /// approve (or no gate attached — the unit-test default).
    fn consent_state(&self, hook: &HookConfig) -> Option<String> {
        let consent = self.consent.as_ref()?;
        let mut saw_gated = false;
        let mut all_approved = true;
        for action in &hook.actions {
            let key = match action {
                HookAction::Command { command } => command.clone(),
                HookAction::Http { url, .. } => format!("http:{url}"),
                _ => continue,
            };
            saw_gated = true;
            if !consent.is_approved(&hook.plugin_name, &hook.scope_key, &hook.plugin_root, &key) {
                all_approved = false;
            }
        }
        if !saw_gated {
            return None;
        }
        Some(if all_approved { "approved" } else { "pending" }.to_string())
    }

    /// Execute observer hooks for an event
    ///
    /// Different observers run in parallel, but actions within each observer
    /// run sequentially. Observers cannot block or modify the context.
    /// Errors are logged but do not propagate.
    pub async fn execute_observers(&self, event: HookEvent, context: &HookContext) {
        let visibility = crate::extension::visibility::VisibilityCtx::for_session();
        // Filter hooks by event and kind == Observer
        let observers: Vec<_> = self
            .hooks
            .iter()
            .filter(|h| h.event == event && h.kind == HookKind::Observer)
            .filter(|h| self.matches_pattern(h, context))
            .filter(|h| self.project_scope_allows(h, &visibility))
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
                // Per hook, as in `execute_interceptors`'s twin: the spelling
                // this hook was registered under.
                let event_name = hook.event_name();
                for action in &hook.actions {
                    match self
                        .execute_action(
                            action,
                            context,
                            &hook.plugin_root,
                            &hook.plugin_name,
                            &hook.scope_key,
                            event,
                            &event_name,
                            timeout_override,
                        )
                        .await
                    {
                        Ok(ar) => {
                            // Same derivation as the interceptor seam, with the
                            // observer kind: exit 2 is logged, never applied.
                            // `scratch` is dropped — observers cannot modify.
                            if let HookAction::Command { .. } = action {
                                let mut scratch = super::HookResult::default();
                                super::derive_decision(
                                    ar.exit_code,
                                    ar.output.as_deref().unwrap_or(""),
                                    ar.error.as_deref().unwrap_or(""),
                                    HookKind::Observer,
                                    &mut scratch,
                                );
                            }
                        }
                        Err(e) => warn!(
                            "Observer hook action from plugin '{}' failed: {}",
                            hook.plugin_name, e
                        ),
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
    use crate::extension::visibility::{canonical_root, ScopeKey, VisibilityCtx};
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
            declared_event: None,
            scope_key: ScopeKey::Global,
        }
    }

    /// The stdin JSON a command hook registered under `event`'s canonical
    /// name is handed — the same derivation production and `aleph-server hooks
    /// test` spawn from.
    fn stdin_json(event: HookEvent, ctx: &HookContext) -> String {
        command_hook_invocation("true", &event.canonical_name(), ctx, None, "test").stdin
    }

    #[test]
    fn inventory_flags_a_matcher_on_a_tool_less_event() {
        // Foot-gun #1: matchers test `tool_name`, which SessionStart has none
        // of — the hook loads, never fires, and used to say so only in a boot
        // log line nobody reads hours later.
        let mut hook = dummy_hook("user:global");
        hook.event = HookEvent::SessionStart;
        hook.kind = HookKind::Observer;
        hook.matcher = Some("Write".into());

        let entry = &HookExecutor::new(vec![hook]).inventory()[0];
        assert!(!entry.reachable);
        assert!(
            entry
                .issue
                .as_deref()
                .unwrap_or_default()
                .contains("matcher"),
            "issue must name the cause: {:?}",
            entry.issue
        );
    }

    #[test]
    fn inventory_flags_an_interceptor_on_an_observer_only_event() {
        // Foot-gun #2: the global fire-and-forget seams dispatch observers
        // only, so an interceptor registered there never executes.
        let mut hook = dummy_hook("plugin:audit");
        hook.event = HookEvent::MessageSent;
        hook.kind = HookKind::Interceptor;

        let entry = &HookExecutor::new(vec![hook]).inventory()[0];
        assert!(!entry.reachable);
        assert!(entry
            .issue
            .as_deref()
            .unwrap_or_default()
            .contains("observers"));
    }

    #[test]
    fn inventory_reports_a_well_formed_hook_as_reachable() {
        let mut hook = dummy_hook("user:global");
        hook.event = HookEvent::BeforeToolCall;
        hook.kind = HookKind::Interceptor;
        hook.matcher = Some("Write|Edit".into());
        hook.timeout_secs = Some(30);

        let entry = &HookExecutor::new(vec![hook]).inventory()[0];
        assert!(entry.reachable);
        assert!(entry.issue.is_none());
        assert_eq!(entry.kind, "interceptor");
        assert_eq!(entry.event, "before_tool_call", "canonical serde name");
        assert_eq!(entry.matcher.as_deref(), Some("Write|Edit"));
        assert_eq!(entry.timeout_secs, Some(30));
        assert_eq!(entry.actions, vec!["command: true"]);
        assert_eq!(entry.source, "user:global");
        // No consent gate attached → nothing to report.
        assert!(entry.consent.is_none());
    }

    #[test]
    fn a_claude_code_matcher_selects_the_aleph_tool() {
        let mut hook = dummy_hook("user:global");
        hook.event = HookEvent::BeforeToolCall;
        hook.matcher = Some("Edit|Write".into());
        let exec = HookExecutor::new(vec![hook.clone()]);
        assert!(exec.matches_pattern(&hook, &HookContext::new("s").with_tool_name("file_write")));
        assert!(exec.matches_pattern(&hook, &HookContext::new("s").with_tool_name("file_edit")));
        assert!(!exec.matches_pattern(&hook, &HookContext::new("s").with_tool_name("file_read")));
        // An MCP tool under its CC spelling.
        let mut mcp = dummy_hook("user:global");
        mcp.event = HookEvent::BeforeToolCall;
        mcp.matcher = Some("mcp__.*__delete.*".into());
        let exec = HookExecutor::new(vec![mcp.clone()]);
        assert!(exec.matches_pattern(
            &mcp,
            &HookContext::new("s").with_tool_name("github__delete_repo")
        ));
        assert!(!exec.matches_pattern(
            &mcp,
            &HookContext::new("s").with_tool_name("github__list_repos")
        ));
    }

    /// Same claim as `a_claude_code_matcher_selects_the_aleph_tool`, but
    /// driven through the real `execute_interceptors` fire site rather than
    /// calling `matches_pattern` directly — so bypassing `cc_spellings` at
    /// the fire site (not just breaking `CC_TOOL_ALIASES`) goes red here
    /// (判据 §4, mirrors the project-scope fire-site tests below).
    #[tokio::test]
    async fn execute_interceptors_fires_on_a_claude_code_spelled_matcher() {
        let mut hook = dummy_hook("plugin:foo");
        hook.event = HookEvent::BeforeToolCall;
        hook.kind = HookKind::Interceptor;
        hook.matcher = Some("Edit|Write".into());
        hook.actions = vec![HookAction::Prompt {
            prompt: "gated".into(),
        }];
        let exec = HookExecutor::new(vec![hook]);

        let (_, fired) = exec
            .execute_interceptors(
                HookEvent::BeforeToolCall,
                HookContext::new("s").with_tool_name("file_write"),
            )
            .await
            .unwrap();
        assert_eq!(
            fired.hooks_executed, 1,
            "CC `Write` matcher must select Aleph's file_write via execute_interceptors"
        );

        let (_, skipped) = exec
            .execute_interceptors(
                HookEvent::BeforeToolCall,
                HookContext::new("s").with_tool_name("file_read"),
            )
            .await
            .unwrap();
        assert_eq!(
            skipped.hooks_executed, 0,
            "CC `Edit|Write` matcher must not select file_read"
        );
    }

    #[test]
    fn inventory_reports_pending_consent_for_unapproved_commands() {
        // The third silent-death cause, and the one with no load-time warning
        // at all: the hook is perfectly well-formed and simply never runs
        // because nobody approved it.
        use crate::sync_primitives::Arc;
        let dir = tempfile::tempdir().unwrap();
        let consent = Arc::new(crate::extension::hooks::ShellHookConsent::with_path(
            dir.path().join("allowlist.json"),
        ));
        let mut hook = dummy_hook("plugin:linter");
        hook.event = HookEvent::BeforeToolCall;
        hook.kind = HookKind::Interceptor;
        // The root the approval below is recorded with — an approval binds it.
        hook.plugin_root = PathBuf::from("/p");

        let exec = HookExecutor::new(vec![hook]).with_consent(consent.clone());
        assert_eq!(exec.inventory()[0].consent.as_deref(), Some("pending"));

        // Approving flips it — and the hook stays reachable throughout, since
        // consent is a separate axis from configuration validity.
        consent.record_pending(
            "plugin:linter",
            &ScopeKey::Global,
            "true",
            "before_tool_call",
            std::path::Path::new("/p"),
        );
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(std::path::Path::new("/p")))
            .expect("approve");
        let entry = &exec.inventory()[0];
        assert_eq!(entry.consent.as_deref(), Some("approved"));
        assert!(entry.reachable);
    }

    #[test]
    fn inventory_binds_project_hooks_to_their_project_root() {
        // A project hook only fires inside its own workspace; the inventory
        // must say which, otherwise "registered but silent" looks like a bug.
        let root = tempfile::tempdir().unwrap();
        let hook = project_hook("user:project", root.path());
        let entry = &HookExecutor::new(vec![hook]).inventory()[0];
        // Canonical spelling — the same string the gate compares against.
        assert_eq!(
            entry.project_root,
            Some(canonical_root(root.path()).display().to_string())
        );

        // Global and plugin hooks are not workspace-bound.
        let plain = &HookExecutor::new(vec![dummy_hook("plugin:foo")]).inventory()[0];
        assert!(plain.project_root.is_none());
    }

    /// The inventory reads the same fact the gate reads: a plugin-shipped hook
    /// whose key is `Project(root)` is bound to that root, label or no label.
    /// Deriving it from the `user:project` prefix instead would show
    /// `project_root: None` for a hook the gate suppresses everywhere else —
    /// "registered but silent" with no visible reason.
    #[test]
    fn inventory_binds_a_plugin_hook_with_a_project_key_to_that_root() {
        let root = tempfile::tempdir().unwrap();
        let mut hook = dummy_hook("plugin:foo");
        hook.scope_key = ScopeKey::project(root.path());
        let entry = &HookExecutor::new(vec![hook]).inventory()[0];
        assert_eq!(
            entry.project_root,
            Some(canonical_root(root.path()).display().to_string())
        );
    }

    #[test]
    fn hook_timeout_override_is_clamped_to_the_ceiling() {
        // An interceptor seam AWAITS its hooks: an unclamped
        // `timeout_secs: 86400` would wedge the tool gate for a day. The
        // clamp lives at this single chokepoint so every config source
        // (user / project / plugin hooks.json / aleph.plugin.toml) is covered.
        let exec = HookExecutor::empty();
        assert_eq!(
            exec.effective_timeout(Some(86_400)),
            Duration::from_secs(MAX_HOOK_TIMEOUT_SECS)
        );
        // Under the ceiling passes through untouched.
        assert_eq!(exec.effective_timeout(Some(30)), Duration::from_secs(30));
        // Exactly at the ceiling is allowed.
        assert_eq!(
            exec.effective_timeout(Some(MAX_HOOK_TIMEOUT_SECS)),
            Duration::from_secs(MAX_HOOK_TIMEOUT_SECS)
        );
    }

    #[test]
    fn zero_and_absent_timeout_fall_back_to_the_executor_default() {
        // `Duration::ZERO` would time every hook out instantly, so a zero
        // override is read as "unset" rather than honoured literally.
        let exec = HookExecutor::empty().with_timeout(Duration::from_secs(11));
        assert_eq!(exec.effective_timeout(None), Duration::from_secs(11));
        assert_eq!(exec.effective_timeout(Some(0)), Duration::from_secs(11));
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

    /// A project hook carries its project's key; the hook loader stamps it
    /// from the file's directory (`<root>/.aleph/hooks*.json` → `Project(root)`).
    fn project_hook(plugin_name: &str, project_root: &Path) -> HookConfig {
        let mut h = dummy_hook(plugin_name);
        h.plugin_root = project_root.join(".aleph");
        h.scope_key = ScopeKey::project(project_root);
        h
    }

    #[test]
    fn global_hooks_are_never_project_gated() {
        let exec = HookExecutor::empty();
        // No `with_project_root` scope active here, yet these must still fire.
        assert!(
            exec.project_scope_allows(&dummy_hook("user:global"), &VisibilityCtx::for_session())
        );
        assert!(exec.project_scope_allows(&dummy_hook("plugin:foo"), &VisibilityCtx::for_session()));
    }

    /// The gate no longer sniffs the `user:project` label: a plugin-shipped
    /// hook whose plugin was found under a project is gated exactly like a
    /// project hook file. Same predicate, sixth face.
    #[tokio::test]
    async fn a_plugin_hook_with_a_project_key_is_gated_like_a_project_hook() {
        let proj = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let exec = HookExecutor::empty();
        let mut hook = dummy_hook("some-plugin");
        hook.scope_key = ScopeKey::project(proj.path());

        let inside = crate::projects::with_project_root(Some(proj.path().to_path_buf()), async {
            exec.project_scope_allows(&hook, &VisibilityCtx::for_session())
        })
        .await;
        let elsewhere =
            crate::projects::with_project_root(Some(other.path().to_path_buf()), async {
                exec.project_scope_allows(&hook, &VisibilityCtx::for_session())
            })
            .await;
        assert!(inside, "plugin hook must fire inside its own project");
        assert!(
            !elsewhere,
            "plugin hook must NOT fire inside another project"
        );
    }

    #[tokio::test]
    async fn project_hook_fires_only_in_its_own_workspace() {
        let proj_a = tempfile::tempdir().unwrap();
        let proj_b = tempfile::tempdir().unwrap();
        let exec = HookExecutor::empty();
        let hook_a = project_hook("user:project", proj_a.path());

        // Active project == the hook's project → fires.
        let in_a = crate::projects::with_project_root(Some(proj_a.path().to_path_buf()), async {
            exec.project_scope_allows(&hook_a, &VisibilityCtx::for_session())
        })
        .await;
        assert!(in_a, "project hook must fire inside its own project");

        // A different project is active → suppressed (no cross-project leak).
        let in_b = crate::projects::with_project_root(Some(proj_b.path().to_path_buf()), async {
            exec.project_scope_allows(&hook_a, &VisibilityCtx::for_session())
        })
        .await;
        assert!(!in_b, "project hook must NOT fire inside another project");
    }

    /// Behaviour change recorded in the spec: with NO resolvable project
    /// (task-local `None` and CWD unreadable is not reproducible here, so the
    /// ctx is built directly) a project hook is suppressed, where it used to
    /// fail open.
    #[test]
    fn a_project_hook_with_no_project_context_is_suppressed_not_fired() {
        let proj = tempfile::tempdir().unwrap();
        let exec = HookExecutor::empty();
        let hook = project_hook("user:project", proj.path());
        let nowhere = VisibilityCtx { project_root: None };
        assert!(!exec.project_scope_allows(&hook, &nowhere));
    }

    // ── Fire-site wiring (判据 §4) ────────────────────────────────────────
    //
    // The tests above call `project_scope_allows` directly, so deleting the
    // `continue` in `execute_interceptors` or the `.filter` in
    // `execute_observers` would leave them all green. These two go through
    // the fire sites themselves: effect reached, not just predicate correct.

    /// An interceptor keyed to project `a` is skipped by `execute_interceptors`
    /// while project `b` is active, and runs while `a` is. A `Prompt` action
    /// keeps this spawn-free; `hooks_executed` is bumped right after the gate.
    #[tokio::test]
    async fn execute_interceptors_skips_a_hook_keyed_to_another_project() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut hook = dummy_hook("plugin:foo");
        hook.event = HookEvent::BeforeToolCall;
        hook.kind = HookKind::Interceptor;
        hook.actions = vec![HookAction::Prompt {
            prompt: "gated".into(),
        }];
        hook.scope_key = ScopeKey::project(a.path());
        let exec = HookExecutor::new(vec![hook]);

        let in_b = crate::projects::with_project_root(Some(b.path().to_path_buf()), async {
            exec.execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
                .await
                .unwrap()
                .1
        })
        .await;
        assert_eq!(
            in_b.hooks_executed, 0,
            "interceptor keyed to a must not run inside b"
        );

        let in_a = crate::projects::with_project_root(Some(a.path().to_path_buf()), async {
            exec.execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
                .await
                .unwrap()
                .1
        })
        .await;
        assert_eq!(
            in_a.hooks_executed, 1,
            "interceptor keyed to a must run inside a"
        );
    }

    /// An observer keyed to project `a` does not touch its sentinel while
    /// project `b` is active, and does while `a` is.
    #[cfg(unix)] // POSIX-only: shell observer uses the `touch` fixture
    #[tokio::test]
    async fn execute_observers_skips_a_hook_keyed_to_another_project() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let sentinel = a.path().join("observed.flag");
        let mut hook = dummy_hook("plugin:foo");
        hook.actions = vec![HookAction::Command {
            command: format!("touch '{}'", sentinel.display()),
        }];
        // Command actions spawn with `plugin_root` as cwd; `dummy_hook`'s
        // empty path would fail the spawn before the gate is ever exercised.
        hook.plugin_root = std::env::temp_dir();
        hook.scope_key = ScopeKey::project(a.path());
        let exec = HookExecutor::new(vec![hook]);
        let ctx = HookContext::new("s");

        crate::projects::with_project_root(Some(b.path().to_path_buf()), async {
            exec.execute_observers(HookEvent::MessageReceived, &ctx)
                .await;
        })
        .await;
        assert!(
            !sentinel.exists(),
            "observer keyed to a must not run inside b"
        );

        crate::projects::with_project_root(Some(a.path().to_path_buf()), async {
            exec.execute_observers(HookEvent::MessageReceived, &ctx)
                .await;
        })
        .await;
        assert!(sentinel.exists(), "observer keyed to a must run inside a");
    }

    /// Captures every event's formatted `message` field, scoped to one
    /// async block via `tracing_subscriber`'s `set_default()` guard — the
    /// same in-crate idiom as `approval::config::tests::CaptureLayer` /
    /// `spend::tests::CapturedErrorEvents` (no new dependency). The observer
    /// arm's `derive_decision` call (see `execute_observers` above) writes
    /// only into a `scratch: HookResult` that is immediately discarded — a
    /// `tracing::warn!` line is the ONLY externally observable effect a
    /// fire-site test can assert on (P4.1 review, Q1).
    #[derive(Clone, Default)]
    struct CapturedMessages(Arc<std::sync::Mutex<Vec<String>>>);

    struct MessageVisitor(String);

    impl tracing::field::Visit for MessageVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedMessages {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(visitor.0);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exit_2_observer_hook_warns_but_does_not_block() {
        // Fire-site proof per 判据 §4: drives the REAL `execute_observers` →
        // `execute_action` → `derive_decision` path (Observer kind), not the
        // pure-function matrix test (`exit_2_on_an_observer_only_logs` in
        // `hooks/mod.rs`). Deleting the `if let HookAction::Command { .. }`
        // block in `execute_observers` (or its `derive_decision` call) must
        // turn this red — see the mutation record in the report.
        use crate::extension::hooks::HookContext;
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::util::SubscriberInitExt as _;

        let mut hook = dummy_hook("plugin:exit2-observer");
        hook.actions = vec![HookAction::Command {
            command: "echo 'outside the repo' >&2; exit 2".into(),
        }];
        // `dummy_hook`'s empty `plugin_root` would fail the spawn itself
        // (see `execute_observers_skips_a_hook_keyed_to_another_project`).
        hook.plugin_root = std::env::temp_dir();
        let executor = HookExecutor::new(vec![hook]);

        let captured = CapturedMessages::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        let _guard = subscriber.set_default();

        executor
            .execute_observers(HookEvent::MessageReceived, &HookContext::new("s"))
            .await;

        drop(_guard);
        let messages = captured.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("observer hook exited 2")),
            "observer exit-2 warn must fire; got: {messages:?}"
        );
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
            declared_event: None,
            scope_key: ScopeKey::Global,
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
    async fn exit_2_command_hook_blocks_the_interceptor_seam_with_its_stderr() {
        // The most common Claude Code hook idiom. Before this task the exit
        // code was recorded on `ActionResult` and never consulted, so this
        // hook let the tool through.
        use crate::extension::hooks::HookContext;
        let hook = interceptor_command_hook("echo 'outside the repo' >&2; exit 2");
        let executor = HookExecutor::new(vec![hook]);
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("interceptor pass returns Ok on a hook decision");
        assert!(result.blocked, "exit 2 must block");
        assert_eq!(result.block_reason.as_deref(), Some("outside the repo"));
        assert!(
            !result.action_failed,
            "exit 2 is a hook DECISION, not an infrastructure failure"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exit_1_command_hook_is_non_blocking() {
        use crate::extension::hooks::HookContext;
        let hook = interceptor_command_hook("echo 'deny: nope'; exit 1");
        let executor = HookExecutor::new(vec![hook]);
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("Ok");
        assert!(!result.blocked && !result.denied, "exit 1 is non-blocking");
        assert_eq!(result.action_results.len(), 1);
        assert_eq!(result.action_results[0].exit_code, Some(1));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_event_name_echoes_the_spelling_the_hook_was_registered_under() {
        // Two hooks on the same seam, one written the Claude Code way, one the
        // Aleph way; each must read back its OWN spelling from stdin. Both
        // dispatch loops are driven (interceptor and observer) — they are the
        // two places the payload is built.
        use crate::extension::hooks::HookContext;
        let dir = tempfile::tempdir().unwrap();
        let out = |name: &str| dir.path().join(name);
        let capture = |name: &str| format!("cat > {}", out(name).display());
        let mut cc = interceptor_command_hook(&capture("cc.json"));
        cc.declared_event = Some("PreToolUse".into());
        let mut aleph = interceptor_command_hook(&capture("aleph.json"));
        aleph.declared_event = Some("before_tool_call".into());
        let mut observer = interceptor_command_hook(&capture("observer.json"));
        observer.event = HookEvent::AfterToolCall;
        observer.kind = HookKind::Observer;
        observer.declared_event = Some("PostToolUse".into());
        let mut bare = interceptor_command_hook("true");
        bare.declared_event = None;
        assert_eq!(
            bare.event_name(),
            "before_tool_call",
            "no spelling → the canonical serde name"
        );

        let executor = HookExecutor::new(vec![cc, aleph, observer]);
        let ctx = HookContext::new("s").with_tool_name("bash");
        executor
            .execute_interceptors(HookEvent::BeforeToolCall, ctx.clone())
            .await
            .expect("both hooks run");
        executor
            .execute_observers(HookEvent::AfterToolCall, &ctx)
            .await;

        let seen = |name: &str| -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(out(name)).unwrap()).unwrap()
        };
        assert_eq!(seen("cc.json")["hook_event_name"], "PreToolUse");
        assert_eq!(seen("aleph.json")["hook_event_name"], "before_tool_call");
        assert_eq!(seen("observer.json")["hook_event_name"], "PostToolUse");
    }

    /// `aleph-server hooks test` rebuilds a hook's run from its consent entry, so the
    /// entry must hold what production derived: the spelling the hook is
    /// dispatched with (not the enum's Rust name) and the root its path
    /// variables resolve to — recorded at the real consent gate.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_consent_entry_records_what_aleph_hooks_test_rebuilds_the_run_from() {
        use crate::extension::hooks::{HookContext, ShellHookConsent};
        use crate::sync_primitives::Arc;
        let dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let consent = Arc::new(ShellHookConsent::with_path(
            dir.path().join("allowlist.json"),
        ));
        let mut hook = interceptor_command_hook("true");
        hook.declared_event = Some("PreToolUse".into());
        hook.plugin_root = root.path().to_path_buf();
        HookExecutor::new(vec![hook.clone()])
            .with_consent(consent.clone())
            .execute_interceptors(
                HookEvent::BeforeToolCall,
                HookContext::new("s").with_tool_name("bash"),
            )
            .await
            .expect("an unapproved hook is skipped, not an error");

        let entry = consent
            .entries()
            .into_iter()
            .next()
            .expect("recorded pending");
        assert_eq!(entry.event, "PreToolUse");
        assert_eq!(entry.plugin_root.as_deref(), Some(root.path()));
        // The CLI's rebuilt payload names the event as production does.
        let rebuilt: serde_json::Value = serde_json::from_str(
            &command_hook_invocation(
                &entry.command,
                &entry.event,
                &HookContext::new("s"),
                entry.plugin_root.as_deref(),
                &entry.plugin_name,
            )
            .stdin,
        )
        .unwrap();
        assert_eq!(rebuilt["hook_event_name"], hook.event_name());
    }

    /// A fixed data field this event does not carry is removed from the
    /// command's environment — not inherited from the daemon's, where a variable of the
    /// same name would read as the event's value. `CLAUDE_PROJECT_DIR` is the
    /// same rule for the run directory (unknown outside a run).
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial] // writes process env a spawned child reads
    async fn a_field_the_event_lacks_is_not_inherited_from_the_daemon() {
        use crate::extension::hooks::HookContext;
        const INHERITED: [&str; 4] = ["TOOL_NAME", "ARGUMENTS", "TOOL_INPUT", "CLAUDE_PROJECT_DIR"];
        /// Sets the daemon-side values for the test and removes them after.
        struct DaemonEnv;
        impl Drop for DaemonEnv {
            fn drop(&mut self) {
                for key in INHERITED {
                    std::env::remove_var(key);
                }
            }
        }
        let _daemon = DaemonEnv;
        for key in INHERITED {
            std::env::set_var(key, "from-the-daemon");
        }

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("env.txt");
        let shown: Vec<String> = INHERITED
            .iter()
            .map(|key| format!("\"${{{key}-unset}}\""))
            .collect();
        let mut hook = interceptor_command_hook(&format!(
            "printf '%s|' {} > '{}'",
            shown.join(" "),
            out.display()
        ));
        hook.event = HookEvent::SessionStart;
        hook.kind = HookKind::Observer;
        // Outside a run: no tool, no input, no run directory.
        HookExecutor::new(vec![hook])
            .execute_observers(HookEvent::SessionStart, &HookContext::new("s"))
            .await;

        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "unset|unset|unset|unset|"
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
        let rewriter = interceptor_command_hook(r#"echo 'update_input: {"path":"/rewritten"}'"#);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn interceptor_chain_propagates_a_json_hook_specific_rewrite_too() {
        // Same contract as `interceptor_chain_propagates_rewrite_to_stdin_payload`,
        // but for the Claude-Code spelling (`hookSpecificOutput.updatedInput`)
        // instead of the Aleph-native `update_input:` line — both must reach
        // `accumulated.updated_input` through `HookResult::set_updated_input`
        // and thread forward to the next interceptor's stdin the same way.
        use crate::extension::hooks::HookContext;
        let rewriter = interceptor_command_hook(
            r#"echo '{"hookSpecificOutput":{"updatedInput":{"path":"/rewritten"}}}'"#,
        );
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
            "second interceptor must see the JSON rewrite on stdin: {:?}",
            result.additional_contexts
        );
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
                &ScopeKey::Global,
                HookEvent::MessageReceived,
                "message_received",
                None,
            )
            .await
            .expect("plugin action must skip (not error) when manager is absent");
        assert!(!result.success);
        assert!(result.output.is_none());
    }

    /// Names `path` as the transcript of `session` and of nothing else, so a
    /// test also proves the lookup is keyed by the context's session id.
    struct OneTranscript {
        session: &'static str,
        path: &'static str,
    }

    impl crate::extension::hooks::TranscriptSource for OneTranscript {
        fn transcript_path(&self, session_id: &str) -> Option<PathBuf> {
            (session_id == self.session).then(|| PathBuf::from(self.path))
        }
    }

    fn one_transcript(
        session: &'static str,
        path: &'static str,
    ) -> Option<Arc<dyn crate::extension::hooks::TranscriptSource>> {
        Some(Arc::new(OneTranscript { session, path }))
    }

    #[tokio::test]
    async fn post_tool_payload_carries_the_claude_code_envelope_keys() {
        use crate::extension::hooks::{with_transcript_source, HookContext};
        let ctx = HookContext::new("agent:main:ws:1")
            .with_tool_name("file_read")
            .with_tool_input(r#"{"path":"/tmp/x"}"#)
            .with_tool_output("contents")
            .with_tool_error(false)
            .with_working_dir("/work")
            .with_permission_mode("default")
            .with_env("RUN_ID", "r1");
        let transcripts = one_transcript("agent:main:ws:1", "/data/sessions/k/transcript.jsonl");
        let json: serde_json::Value = serde_json::from_str(
            &with_transcript_source(transcripts, async {
                stdin_json(HookEvent::AfterToolCall, &ctx)
            })
            .await,
        )
        .unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected = vec![
            "hook_event_name",
            "session_id",
            "tool_name",
            "tool_input",
            "tool_output",
            CC_POST_TOOL_RESULT_KEY,
            "tool_error",
            "cwd",
            "transcript_path",
            "permission_mode",
            "env",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected, "payload key set");
        assert_eq!(json["permission_mode"], "default");
        assert_eq!(json["transcript_path"], "/data/sessions/k/transcript.jsonl");
        assert_eq!(json["cwd"], "/work");
        // Output that is not JSON reads the same under either key.
        assert_eq!(json[CC_POST_TOOL_RESULT_KEY], json["tool_output"]);
    }

    /// `tool_response` is the tool's structure when there is one — an object
    /// in the P0 capture (`{"type":"text","file":{..}}`) — and text otherwise.
    /// The inputs are the shapes producers really hand over: the dispatch seam
    /// sends `Value::to_string()` of the budgeted result, i.e. a JSON STRING
    /// holding the model-facing text. `tool_output` stays the verbatim text.
    #[test]
    fn tool_response_is_the_tools_structure_and_text_stays_text() {
        let payload = |output: &str| -> serde_json::Value {
            let ctx = HookContext::new("s").with_tool_output(output);
            serde_json::from_str(&stdin_json(HookEvent::AfterToolCall, &ctx)).unwrap()
        };
        let dispatched = |text: &str| serde_json::Value::String(text.to_string()).to_string();

        let object = payload(&dispatched(r#"{"type":"text","file":{"numLines":1}}"#));
        assert_eq!(
            object[CC_POST_TOOL_RESULT_KEY],
            serde_json::json!({"type": "text", "file": {"numLines": 1}})
        );
        assert_eq!(object[CC_POST_TOOL_RESULT_KEY]["file"]["numLines"], 1);
        assert!(
            object["tool_output"].is_string(),
            "the native key stays text"
        );

        assert_eq!(
            payload(&dispatched("contents"))[CC_POST_TOOL_RESULT_KEY],
            "contents"
        );
        assert_eq!(
            payload(&dispatched("42"))[CC_POST_TOOL_RESULT_KEY],
            "42",
            "a scalar-looking text is still text"
        );
        assert_eq!(
            payload("permission denied: rm")[CC_POST_TOOL_RESULT_KEY],
            "permission denied: rm"
        );
    }

    #[test]
    fn unknown_session_facts_and_mode_are_omitted_not_blanked() {
        // A hook must not be handed `""` for a path that does not exist or a
        // mode nobody resolved (`BeforeAgentStart` fires before the tier is
        // known), nor the daemon's own cwd for a directory nobody published:
        // absent means "unknown", a value reads as a fact.
        use crate::extension::hooks::HookContext;
        let ctx = HookContext::new("s").with_tool_name("bash");
        let json: serde_json::Value =
            serde_json::from_str(&stdin_json(HookEvent::BeforeToolCall, &ctx)).unwrap();
        assert!(json.get("cwd").is_none());
        assert!(json.get("transcript_path").is_none());
        assert!(json.get("permission_mode").is_none());
        assert!(
            json.get(CC_POST_TOOL_RESULT_KEY).is_none(),
            "no output → no result key"
        );
    }

    /// A `SessionStart` observer whose command writes its stdin to
    /// `<dir>/stdin.json` and its environment to `<dir>/env`. Its
    /// `plugin_root` is `dir` — deliberately NOT any project the tests
    /// publish, so a `CLAUDE_PROJECT_DIR` that fell back to it is visible.
    #[cfg(unix)]
    fn dumping_executor(dir: &std::path::Path) -> HookExecutor {
        let mut hook = dummy_hook("user:global");
        hook.event = HookEvent::SessionStart;
        hook.plugin_root = dir.to_path_buf();
        hook.actions = vec![HookAction::Command {
            command: format!("cat > '{0}/stdin.json'; env > '{0}/env'", dir.display()),
        }];
        HookExecutor::new(vec![hook])
    }

    /// What the hook saw: `(stdin cwd, $CLAUDE_PROJECT_DIR)`.
    #[cfg(unix)]
    fn seen_by_hook(dir: &std::path::Path) -> (Option<String>, Option<String>) {
        let stdin: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("stdin.json")).expect("the hook ran"),
        )
        .unwrap();
        let env = std::fs::read_to_string(dir.join("env")).unwrap();
        (
            stdin["cwd"].as_str().map(str::to_string),
            env.lines()
                .find_map(|l| l.strip_prefix("CLAUDE_PROJECT_DIR="))
                .map(str::to_string),
        )
    }

    /// Inside a project run both the payload's `cwd` and the command's
    /// `$CLAUDE_PROJECT_DIR` name the project — not `plugin_root` (one level
    /// too deep for a project settings hook), not the daemon's cwd.
    #[cfg(unix)]
    #[tokio::test]
    async fn inside_a_project_run_cwd_and_claude_project_dir_are_the_project() {
        let hook_dir = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let exec = dumping_executor(hook_dir.path());
        let ctx = HookContext::new("s");
        crate::projects::with_project_root(
            Some(project.path().to_path_buf()),
            exec.execute_observers(HookEvent::SessionStart, &ctx),
        )
        .await;
        let want = project.path().to_string_lossy().to_string();
        assert_eq!(
            seen_by_hook(hook_dir.path()),
            (Some(want.clone()), Some(want))
        );
    }

    /// A run with no project still has an authorised workspace; that is the
    /// session's directory.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_run_without_a_project_reports_its_exec_workspace() {
        let hook_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let exec = dumping_executor(hook_dir.path());
        let ctx = HookContext::new("s");
        crate::projects::with_project_root(
            None,
            crate::sandbox::context::with_exec_workspace(
                Some(workspace.path().to_path_buf()),
                exec.execute_observers(HookEvent::SessionStart, &ctx),
            ),
        )
        .await;
        let want = workspace.path().to_string_lossy().to_string();
        assert_eq!(
            seen_by_hook(hook_dir.path()),
            (Some(want.clone()), Some(want))
        );
    }

    /// Outside any run nobody can name the session's directory: both are
    /// absent — not the daemon's cwd, not `plugin_root`.
    #[cfg(unix)]
    #[tokio::test]
    async fn outside_any_run_neither_cwd_nor_claude_project_dir_is_sent() {
        let hook_dir = tempfile::tempdir().unwrap();
        let exec = dumping_executor(hook_dir.path());
        exec.execute_observers(HookEvent::SessionStart, &HookContext::new("s"))
            .await;
        assert_eq!(seen_by_hook(hook_dir.path()), (None, None));
    }

    /// A published source that has no file for THIS session answers nothing,
    /// even though it names a file for another one.
    #[tokio::test]
    async fn a_source_without_this_sessions_file_leaves_the_key_out() {
        use crate::extension::hooks::{with_transcript_source, HookContext};
        let ctx = HookContext::new("agent:main:other");
        let json: serde_json::Value = serde_json::from_str(
            &with_transcript_source(one_transcript("agent:main:ws:1", "/t.jsonl"), async {
                stdin_json(HookEvent::Stop, &ctx)
            })
            .await,
        )
        .unwrap();
        assert!(json.get("transcript_path").is_none());
    }

    // -----------------------------------------------------------------------
    // A command hook's variables reach it through its environment. On unix
    // nothing is spliced into the shell source — not the data, and not the
    // path variables either (Windows keeps splicing those).
    // -----------------------------------------------------------------------

    /// A `$(…)` in a tool argument is data to the hook, not code: the shell
    /// expands `"$ARGUMENTS"` from the environment and does not parse it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_substitution_in_the_arguments_does_not_run_in_the_hook() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("M");
        let out = dir.path().join("out");
        let args = serde_json::json!({ "x": format!("$(touch {})", marker.display()) }).to_string();
        let hook = interceptor_command_hook(&format!(
            r#"printf '%s' "$ARGUMENTS" > '{}'"#,
            out.display()
        ));
        let ctx = HookContext::new("s")
            .with_tool_name("bash")
            .with_arguments(args.clone())
            .with_tool_input(args.clone());
        HookExecutor::new(vec![hook])
            .execute_interceptors(HookEvent::BeforeToolCall, ctx)
            .await
            .expect("the hook runs");
        assert!(!marker.exists(), "the argument's `$(…)` ran as shell");
        assert_eq!(std::fs::read_to_string(&out).expect("the hook ran"), args);
    }

    /// A refusal's reason quotes identifiers in backticks and says `Aleph's`;
    /// a hook that reads it as `"$DENY_REASON"` prints it and runs nothing.
    #[cfg(unix)]
    #[tokio::test]
    async fn backticks_in_a_deny_reason_do_not_run_in_the_hook() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("B");
        let out = dir.path().join("out");
        let reason = format!(
            "This conversation is PLANNING, so `touch {}` does not run yet; Aleph's plan first.",
            marker.display()
        );
        // `printf`, not `echo`: dash's and macOS `sh`'s `echo` interpret
        // backslashes, so a reason holding one would read back altered.
        let mut hook = interceptor_command_hook(&format!(
            r#"printf '%s\n' "$DENY_REASON" > '{}'"#,
            out.display()
        ));
        hook.event = HookEvent::PermissionDenied;
        hook.kind = HookKind::Observer;
        let ctx = HookContext::new("s")
            .with_tool_name("bash")
            .with_env("DENY_REASON", reason.clone());
        HookExecutor::new(vec![hook])
            .execute_observers(HookEvent::PermissionDenied, &ctx)
            .await;
        assert!(!marker.exists(), "the reason's backticks ran as shell");
        assert_eq!(
            std::fs::read_to_string(&out).expect("the hook ran"),
            format!("{reason}\n")
        );
    }

    /// Every spelling of the plugin root reaches the command through its
    /// environment, and nothing replaces one before the shell parses the
    /// line: the single-quoted copy stays literal text, which it could not be
    /// if it had been spliced in. That copy is the one use whose meaning
    /// changed when the splice was removed.
    #[cfg(unix)]
    #[tokio::test]
    async fn path_variables_reach_the_command_through_its_environment() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("x"), "from-x").unwrap();
        let out = root.path().join("out");
        let reads: Vec<String> = PLUGIN_ROOT_VARIABLES
            .iter()
            .map(|name| format!(r#"cat "${{{name}}}/x""#))
            .collect();
        let mut hook = interceptor_command_hook(&format!(
            "({reads}) > '{out}'; printf '|%s' '${{CLAUDE_PLUGIN_ROOT}}' >> '{out}'",
            reads = reads.join("; "),
            out = out.display()
        ));
        hook.plugin_root = root.path().to_path_buf();
        HookExecutor::new(vec![hook])
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("the hook runs");
        assert_eq!(
            std::fs::read_to_string(&out).expect("the hook ran"),
            format!(
                "{}|${{CLAUDE_PLUGIN_ROOT}}",
                "from-x".repeat(PLUGIN_ROOT_VARIABLES.len())
            )
        );
    }

    /// A plugin root is a directory name, and a directory name can hold a
    /// `$(…)` and a space. Expanded by the shell it is one word of data: the
    /// quoted path reads the file, and nothing runs.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_plugin_root_named_with_a_command_substitution_is_one_word_of_data() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("fmt $(touch M)");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("x"), "from-x").unwrap();
        let out = base.path().join("out");
        let mut hook = interceptor_command_hook(&format!(
            r#"cat "${{CLAUDE_PLUGIN_ROOT}}/x" > '{}'"#,
            out.display()
        ));
        hook.plugin_root = root.clone();
        HookExecutor::new(vec![hook])
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("the hook runs");
        // The hook runs in its root, so a `touch M` that ran lands there.
        assert!(!root.join("M").exists(), "the root's `$(…)` ran as shell");
        assert_eq!(
            std::fs::read_to_string(&out).expect("the hook ran"),
            "from-x"
        );
    }

    /// A plugin hook's data directory reaches it through the environment
    /// under both spellings, and is created because the template names it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_plugin_hooks_data_directory_reaches_it_through_the_environment() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let mut hook = interceptor_command_hook(&format!(
            r#"printf '%s|%s' "${{CLAUDE_PLUGIN_DATA}}" "${{ALEPH_PLUGIN_DATA}}" > '{}'"#,
            out.display()
        ));
        hook.plugin_name = "fmt".into();
        hook.plugin_root = dir.path().to_path_buf();
        let data = crate::extension::plugin_data_dir("fmt");
        HookExecutor::new(vec![hook])
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("the hook runs");
        let data_str = data.to_string_lossy();
        assert_eq!(
            std::fs::read_to_string(&out).expect("the hook ran"),
            format!("{data_str}|{data_str}")
        );
        assert!(
            data.is_dir(),
            "a template that names the directory creates it"
        );
    }

    /// A settings hook has a root but no data directory, and a hook with no
    /// recorded root (an old consent entry, rebuilt by `aleph-server hooks test`)
    /// has neither: whatever is unknown is removed, not inherited from the
    /// daemon — which may itself run inside a Claude Code plugin.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial] // writes process env a spawned child reads
    async fn a_path_variable_the_hook_lacks_is_not_inherited_from_the_daemon() {
        /// Sets the daemon-side values for the test and removes them after.
        struct DaemonEnv;
        impl Drop for DaemonEnv {
            fn drop(&mut self) {
                for key in PLUGIN_DATA_VARIABLES {
                    std::env::remove_var(key);
                }
            }
        }
        let _daemon = DaemonEnv;
        for key in PLUGIN_DATA_VARIABLES {
            std::env::set_var(key, "from-the-daemon");
        }

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("env.txt");
        let shown: Vec<String> = PLUGIN_DATA_VARIABLES
            .iter()
            .map(|key| format!("\"${{{key}-unset}}\""))
            .collect();
        let mut hook = interceptor_command_hook(&format!(
            "printf '%s|' {} > '{}'",
            shown.join(" "),
            out.display()
        ));
        hook.plugin_name = "user:global".into();
        HookExecutor::new(vec![hook])
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("the hook runs");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "unset|unset|");

        let rebuilt = command_hook_invocation("true", "e", &HookContext::new("s"), None, "fmt");
        for name in PLUGIN_ROOT_VARIABLES.iter().chain(&PLUGIN_DATA_VARIABLES) {
            assert!(
                rebuilt.env.contains(&((*name).to_string(), None)),
                "{name} must be removed when the root is unknown"
            );
        }
    }
}
