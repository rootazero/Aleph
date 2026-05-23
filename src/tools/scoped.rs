//! ScopedToolService — bridges LoopToolRegistry + SubagentTool to ToolService.
//!
//! Adapts the Gateway-side `LoopToolRegistry` (with optional SubagentTool,
//! ToolRefreshSource, and hook decorator) to the `ToolService` trait consumed
//! by `AgentHarness`. This is a read-only adapter; it does not modify the
//! underlying registry.

use std::collections::BTreeSet;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::Value;

use crate::agents::subagent_tool::SubagentTool;
use crate::extension::hooks::{HookContext, HookExecutor, PermissionDecision};
use crate::extension::HookEvent;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tool_metadata::ToolHealthCache;
use crate::tools::refresh::ToolRefreshSource;
use crate::tools::runtime::{LoopTool, LoopToolRegistry};
use crate::tools::service::{
    to_metadata_form, ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
};

/// Wrap a tool result `Value` with `<system-reminder>` blocks for each
/// `context:` line emitted by hooks. Strings are prefixed in-place; other
/// values are stringified so the LLM-visible payload stays uniform.
///
/// This is the seam that makes Aleph's `context:` prefix protocol actually
/// reach the model: the contexts are appended to the tool-result text the
/// LLM consumes on its next turn. Without this wiring, `additional_contexts`
/// would be a silent no-op (a historical bug).
fn wrap_value_with_hook_contexts(value: Value, contexts: &[String]) -> Value {
    if contexts.is_empty() {
        return value;
    }
    let reminders = contexts
        .iter()
        .map(|c| format!("<system-reminder>\n{}\n</system-reminder>", c.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let text = match value {
        Value::String(s) => format!("{reminders}\n\n{s}"),
        Value::Null => reminders,
        other => format!("{reminders}\n\n{other}"),
    };
    Value::String(text)
}

// =============================================================================
// ToolHookDecorator trait
// =============================================================================

/// Optional hook that wraps tool execution.
///
/// Implementations receive the tool name and input before execution and the
/// output/error after. This is intentionally minimal — the hook cannot cancel
/// or modify the call, only observe it.
pub trait ToolHookDecorator: Send + Sync {
    /// Called immediately before a tool is invoked.
    fn before_execute(&self, name: &str, input: &Value);

    /// Called after a tool invocation completes (success or error).
    fn after_execute(&self, name: &str, output: &Result<ToolOutput, ToolError>);

    /// Called after [`after_execute`] with the wall-clock duration of the
    /// dispatch (including retry). Default no-op so existing implementations
    /// keep compiling. Implement to surface duration metrics without taking
    /// on per-call latency tracking inside every adapter.
    fn after_execute_with_duration(
        &self,
        _name: &str,
        _output: &Result<ToolOutput, ToolError>,
        _duration_ms: u64,
    ) {
    }
}

// =============================================================================
// ScopedToolService
// =============================================================================

/// Adapts a `LoopToolRegistry` snapshot to the `ToolService` consumer trait.
///
/// Construction:
/// ```text
/// ScopedToolService::new(registry, allowed)
///     .with_subagent_tool(tool)
///     .with_refresh(refresh_source)
///     .with_hook_decorator(decorator)
/// ```
///
/// `allowed` is a set of permitted tool names. Empty = allow-all.
pub struct ScopedToolService {
    inner: Arc<LoopToolRegistry>,
    allowed: BTreeSet<String>,
    subagent_tool: Option<Arc<SubagentTool>>,
    refresh: Option<Arc<dyn ToolRefreshSource>>,
    hook_decorator: Option<Arc<dyn ToolHookDecorator>>,
    /// Extension-shipped hook executor. Fires `BeforeToolCall` interceptors
    /// (block/deny/ask/update_input) and `AfterToolCall`/`AfterToolCallFailure`
    /// observers around every tool execution. `None` = no extension hooks.
    hook_executor: Option<Arc<HookExecutor>>,
    /// Session identifier surfaced into `HookContext` for extension hooks
    /// (env var `SESSION_ID`, log correlation). Empty string when unset.
    hook_session_id: String,
    /// Tool names that require user confirmation before they execute.
    confirm_tools: BTreeSet<String>,
    /// Transport used to obtain that confirmation. `None` = no approval
    /// channel wired, so confirm-required tools fail closed (denied).
    approval_requester: Option<Arc<dyn ApprovalRequester>>,
    /// Routing context of the agent turn this service serves. When set,
    /// `execute()` scopes it into the `TURN_CONTEXT` task-local so HITL tools
    /// (sandbox escalation, `requires_confirmation`, `ask_user`) can route a
    /// prompt back to the originating channel.
    turn_context: Option<crate::tools::turn_context::TurnContext>,
    /// Layer 2 of the tool-result budget: persists oversized outputs to
    /// disk and replaces them with a compact marker before they enter
    /// the conversation history.
    result_store: Option<Arc<crate::tools::result_store::ToolResultStore>>,
    schema_cache: ArcSwap<Option<(u64, Arc<[crate::tool_metadata::ToolDefinition]>)>>,
    cache_generation: std::sync::atomic::AtomicU64,
    /// Optional runtime health cache. When set, `list()` and
    /// `metadata_schema()` strip tools whose probe reports Unhealthy
    /// (and the entry hasn't expired) so the LLM never sees a tool whose
    /// dependencies are dead.
    health: Option<Arc<ToolHealthCache>>,
    /// Last observed health-cache generation. Bumping this on a generation
    /// drift invalidates the `schema_cache`, so a tool flipping
    /// healthy↔unhealthy retransmits to the LLM on the next turn.
    last_health_generation: std::sync::atomic::AtomicU64,
}

impl ScopedToolService {
    /// Create a new `ScopedToolService`.
    ///
    /// `allowed` — set of tool names visible through this service. Empty = allow all.
    pub fn new(inner: Arc<LoopToolRegistry>, allowed: BTreeSet<String>) -> Self {
        Self {
            inner,
            allowed,
            subagent_tool: None,
            refresh: None,
            hook_decorator: None,
            hook_executor: None,
            hook_session_id: String::new(),
            confirm_tools: BTreeSet::new(),
            approval_requester: None,
            turn_context: None,
            result_store: None,
            schema_cache: ArcSwap::from_pointee(None),
            cache_generation: std::sync::atomic::AtomicU64::new(0),
            health: None,
            last_health_generation: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Attach the tool catalog's runtime health cache.
    ///
    /// When set, `list()` and `metadata_schema()` consult the cache and
    /// silently strip any tool whose registered probe reports a non-expired
    /// `Unhealthy`. Cache-key drift on the underlying health generation is
    /// detected on every read so a flip propagates within one turn.
    pub fn with_health(mut self, health: Arc<ToolHealthCache>) -> Self {
        self.health = Some(health);
        self
    }

    /// Attach a Layer 2 result store. When the underlying tool returns a
    /// result whose token estimate exceeds the tool's `max_result_tokens`,
    /// the full text is written to `~/.aleph/data/tool_results/<sess>/...`
    /// and the LLM sees `[Full output persisted: <path> (<n> tokens, <tool>)]`
    /// instead. Without a store wired the service falls back to head+tail
    /// truncation when the budget is exceeded.
    pub fn with_result_store(
        mut self,
        store: Arc<crate::tools::result_store::ToolResultStore>,
    ) -> Self {
        self.result_store = Some(store);
        self
    }

    /// Require user confirmation for the named tools before they execute.
    ///
    /// When a tool in `confirm_tools` is invoked, `execute()` first routes a
    /// confirmation request through `requester`; the tool runs only on an
    /// `Approved` outcome. With no requester wired, confirm-required tools
    /// fail closed.
    pub fn with_confirmation(
        mut self,
        confirm_tools: BTreeSet<String>,
        requester: Arc<dyn ApprovalRequester>,
    ) -> Self {
        self.confirm_tools = confirm_tools;
        self.approval_requester = Some(requester);
        self
    }

    /// Attach the routing context of the agent turn this service serves.
    ///
    /// `execute()` scopes it into the `TURN_CONTEXT` task-local for the
    /// duration of every tool call, letting HITL tools route a prompt back to
    /// the originating channel.
    pub fn with_turn_context(mut self, ctx: crate::tools::turn_context::TurnContext) -> Self {
        self.turn_context = Some(ctx);
        self
    }

    /// Attach a `SubagentTool` that will appear in listings and can be executed.
    pub fn with_subagent_tool(mut self, tool: Arc<SubagentTool>) -> Self {
        self.subagent_tool = Some(tool);
        self
    }

    /// Attach a refresh source. `list()` will trigger a poll on each call.
    ///
    /// Note: because `LoopToolRegistry` is shared via `Arc`, callers that need
    /// live refresh should rebuild the registry externally and swap the `Arc`.
    /// This hook is provided for compatibility with the plan interface.
    pub fn with_refresh(mut self, refresh: Arc<dyn ToolRefreshSource>) -> Self {
        self.refresh = Some(refresh);
        self
    }

    /// Attach a hook decorator for observing tool execution.
    pub fn with_hook_decorator(mut self, hook: Arc<dyn ToolHookDecorator>) -> Self {
        self.hook_decorator = Some(hook);
        self
    }

    /// Attach an extension-shipped `HookExecutor`. Wires `BeforeToolCall`
    /// interceptors (block / deny / ask / update_input) and
    /// `AfterToolCall` / `AfterToolCallFailure` observers around every tool
    /// dispatch. `session_id` flows into `HookContext` so command hooks see
    /// the `$SESSION_ID` variable.
    ///
    /// Inert when `executor.hook_count() == 0`; callers can pass a snapshot
    /// without first counting.
    pub fn with_hook_executor(
        mut self,
        executor: Arc<HookExecutor>,
        session_id: impl Into<String>,
    ) -> Self {
        self.hook_executor = Some(executor);
        self.hook_session_id = session_id.into();
        self
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn is_allowed(&self, name: &str) -> bool {
        // Attached SubagentTool always passes the allow filter. It is appended
        // to listings independently of `allowed` (which is derived from the
        // builtin tool registry — subagent isn't registered there), so without
        // this exception `list()` / `metadata_schema()` / `execute()` would
        // hide subagent from the LLM whenever a non-empty allow set was
        // configured (i.e. every real gateway path).
        if self
            .subagent_tool
            .as_ref()
            .is_some_and(|st| st.name() == name)
        {
            return true;
        }
        self.allowed.is_empty() || self.allowed.contains(name)
    }

    /// Build `ToolDefinitionMetadata` for a built-in loop tool from the
    /// static budget + idempotency tables — the same data
    /// `BuiltinHandler::definition()` surfaces through the handler path.
    /// `ScopedToolService` is the harness's production `ToolService`, so
    /// without this the per-tool wall-clock budget consulted by `act.rs`
    /// via `describe()` would always be `None` and never fire.
    fn builtin_metadata(name: &str) -> ToolDefinitionMetadata {
        ToolDefinitionMetadata {
            idempotent: crate::tools::retry::is_idempotent_builtin_name(name),
            max_duration_ms: crate::tools::budget::builtin_tool_budget_ms(name),
            ..ToolDefinitionMetadata::default()
        }
    }

    fn loop_tool_to_definition(tool: &dyn LoopTool) -> ToolDefinition {
        let name = tool.name();
        ToolDefinition {
            name: name.to_string(),
            description: tool.description().to_string(),
            input_schema: tool.schema(),
            source: ToolSource::Builtin,
            metadata: Self::builtin_metadata(name),
        }
    }

    fn subagent_definition(tool: &SubagentTool) -> ToolDefinition {
        ToolDefinition {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.schema(),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    fn tool_result_to_output(
        name: &str,
        result: crate::tools::runtime::ToolResult,
    ) -> Result<ToolOutput, ToolError> {
        use crate::session::events::ToolOutputMetadata;
        use crate::tools::runtime::ToolResult;
        match result {
            ToolResult::Success { output } | ToolResult::SuccessAndStopLoop { output } => {
                Ok(ToolOutput {
                    value: output,
                    metadata: ToolOutputMetadata::default(),
                })
            }
            // Map `retryable=true` to `ToolError::Transport` so the
            // one-shot retry helper (which keys off `ToolError::is_retryable`,
            // currently `true` for `Timeout` / `Transport` only) actually
            // fires. Semantically the LoopTool layer reports "this is a
            // transient failure that may succeed if tried again"; `Transport`
            // is the best-fitting carrier in the public `ToolError` enum
            // without expanding its variant set.
            ToolResult::Error {
                error,
                retryable: true,
            } => Err(ToolError::Transport {
                name: name.to_string(),
                cause: error,
            }),
            ToolResult::Error { error, .. } => Err(ToolError::Execution {
                name: name.to_string(),
                cause: error,
            }),
        }
    }
}

#[async_trait]
impl ToolService for ScopedToolService {
    async fn list(&self) -> Vec<ToolDefinition> {
        // Trigger refresh poll if a source is configured.
        if let Some(ref refresh) = self.refresh {
            if refresh.poll_changes() {
                // Refresh signals that tools changed externally. The registry
                // is shared via Arc so callers needing live data swap it; here
                // we just acknowledge the signal without mutating our snapshot.
                let _ = refresh.fetch_tools();
            }
        }

        // Take a single health snapshot so the filter is consistent across
        // every tool in this list call.
        let health_snap = self.health.as_ref().map(|h| h.snapshot());

        let mut defs: Vec<ToolDefinition> = self
            .inner
            .tool_definitions()
            .into_iter()
            .map(|d| {
                let metadata = Self::builtin_metadata(&d.name);
                ToolDefinition {
                    name: d.name,
                    description: d.description,
                    input_schema: d.parameters,
                    source: ToolSource::Builtin,
                    metadata,
                }
            })
            .collect();

        // Append subagent tool if configured.
        if let Some(ref st) = self.subagent_tool {
            defs.push(Self::subagent_definition(st.as_ref()));
        }

        // Apply allowed-set filter. `is_allowed` exempts the attached
        // subagent so it survives the retain even when `allowed` is non-empty
        // and doesn't list "subagent" (which it never does — see is_allowed).
        if !self.allowed.is_empty() {
            defs.retain(|d| self.is_allowed(&d.name));
        }

        // Health gate: strip any tool whose probe reports a non-expired
        // Unhealthy. Tools without a registered probe pass through (the
        // snapshot reports them healthy by default).
        if let Some(snap) = &health_snap {
            defs.retain(|d| snap.is_healthy(&d.name));
        }

        defs
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        // Enforce allowed filter first.
        if !self.is_allowed(name) {
            return None;
        }

        // Check subagent tool.
        if let Some(ref st) = self.subagent_tool {
            if st.name() == name {
                return Some(Self::subagent_definition(st.as_ref()));
            }
        }

        // Check inner registry.
        self.inner.get(name).map(Self::loop_tool_to_definition)
    }

    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        // Scope the turn's routing context so HITL tools (sandbox escalation,
        // `requires_confirmation`, `ask_user`) can reach the originating
        // channel. Scoped here — the immediate caller of every tool's
        // `execute` — so it stays visible without crossing a `tokio::spawn`.
        match self.turn_context.clone() {
            Some(turn) => {
                crate::tools::turn_context::TURN_CONTEXT
                    .scope(turn, self.execute_inner(name, input))
                    .await
            }
            None => self.execute_inner(name, input).await,
        }
    }

    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]> {
        use std::sync::atomic::Ordering;

        // Bump generation if the refresh source signals external changes.
        if let Some(ref refresh) = self.refresh {
            if refresh.poll_changes() {
                let _ = refresh.fetch_tools();
                self.cache_generation.fetch_add(1, Ordering::AcqRel);
            }
        }

        // Bump generation if the health cache rotated (a probe flipped
        // healthy↔unhealthy or `invalidate_all` fired). This keeps the
        // metadata schema cache aligned with the same gating snapshot
        // that `list()` sees.
        if let Some(health) = &self.health {
            let live_gen = health.generation();
            let prev = self.last_health_generation.swap(live_gen, Ordering::AcqRel);
            if prev != live_gen {
                self.cache_generation.fetch_add(1, Ordering::AcqRel);
            }
        }

        let gen_now = self.cache_generation.load(Ordering::Acquire);

        // Cache hit?
        if let Some(ref cached) = **self.schema_cache.load() {
            if cached.0 == gen_now {
                return Arc::clone(&cached.1);
            }
        }

        // Take a snapshot for filter application — same as list().
        let health_snap = self.health.as_ref().map(|h| h.snapshot());

        // Cache miss: rebuild loop-side defs (matching list() body), then convert.
        let mut defs: Vec<ToolDefinition> = self
            .inner
            .tool_definitions()
            .into_iter()
            .map(|d| {
                let metadata = Self::builtin_metadata(&d.name);
                ToolDefinition {
                    name: d.name,
                    description: d.description,
                    input_schema: d.parameters,
                    source: ToolSource::Builtin,
                    metadata,
                }
            })
            .collect();
        if let Some(ref st) = self.subagent_tool {
            defs.push(Self::subagent_definition(st.as_ref()));
        }
        if !self.allowed.is_empty() {
            // Mirror list() — subagent is exempt from allow-filter via is_allowed.
            defs.retain(|d| self.is_allowed(&d.name));
        }
        if let Some(snap) = &health_snap {
            defs.retain(|d| snap.is_healthy(&d.name));
        }
        let schema = to_metadata_form(&defs);
        self.schema_cache
            .store(Arc::new(Some((gen_now, Arc::clone(&schema)))));
        schema
    }
}

impl ScopedToolService {
    /// Tool dispatch proper. Wrapped by the `ToolService::execute` trait
    /// method, which scopes `TURN_CONTEXT` around it.
    async fn execute_inner(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        // Enforce allowed filter.
        if !self.is_allowed(name) {
            return Err(ToolError::NotFound {
                name: name.to_string(),
            });
        }

        // Confirmation gate: tools flagged `requires_confirmation` must be
        // approved by the user before they run. Fails closed when no approval
        // transport is wired.
        if self.confirm_tools.contains(name) {
            match &self.approval_requester {
                Some(requester) => {
                    let reason = format!("Tool `{name}` requires your confirmation to run.");
                    // Fire PermissionRequest + Notification (best-effort,
                    // observer-only) so user-facing channels can pop a
                    // toast / send an email / etc. without blocking the
                    // approval path itself.
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::PermissionRequest,
                        &self.hook_session_id,
                        vec![
                            ("TOOL_NAME", name.to_string()),
                            ("REASON", reason.clone()),
                        ],
                    )
                    .await;
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::Notification,
                        &self.hook_session_id,
                        vec![
                            ("KIND", "permission_request".to_string()),
                            ("TOOL_NAME", name.to_string()),
                            ("MESSAGE", reason.clone()),
                        ],
                    )
                    .await;
                    let outcome = requester.request_approval(name, &reason).await;
                    if outcome != ApprovalOutcome::Approved {
                        return Err(ToolError::Execution {
                            name: name.to_string(),
                            cause: format!(
                                "User did not approve running `{name}` ({outcome:?}). \
                                 Do not retry; ask the user how to proceed."
                            ),
                        });
                    }
                }
                None => {
                    return Err(ToolError::Execution {
                        name: name.to_string(),
                        cause: format!(
                            "Tool `{name}` requires confirmation but no approval \
                             channel is available. Do not retry."
                        ),
                    });
                }
            }
        }

        // Fire pre-hook (legacy observational decorator).
        if let Some(ref hook) = self.hook_decorator {
            hook.before_execute(name, &input);
        }

        // Extension `BeforeToolCall` interceptors. May block / deny / ask, or
        // rewrite the tool input via `update_input:`. Inert when no executor
        // is wired or when no hooks match the event. Runs BEFORE routing so a
        // blocked call never reaches the retry pipeline.
        let started = std::time::Instant::now();
        let (effective_input, pre_hook_contexts) =
            match self.run_before_tool_hooks(name, input.clone()).await {
                Ok(outcome) => outcome,
                Err(err) => {
                    let duration_ms: u64 =
                        started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
                    let rejection: Result<ToolOutput, ToolError> = Err(err);
                    if let Some(ref hook) = self.hook_decorator {
                        hook.after_execute(name, &rejection);
                        hook.after_execute_with_duration(name, &rejection, duration_ms);
                    }
                    return rejection;
                }
            };

        // Route to subagent tool if name matches; otherwise route into the
        // inner LoopToolRegistry. Both paths share the retry/Layer 2/sanitize
        // pipeline below.
        let routing = if self
            .subagent_tool
            .as_ref()
            .is_some_and(|st| st.name() == name)
        {
            RoutingTarget::Subagent
        } else if self.inner.get(name).is_some() || self.inner.resolve(name).is_some() {
            RoutingTarget::Inner
        } else {
            RoutingTarget::Missing
        };

        let mut result = match routing {
            RoutingTarget::Missing => Err(ToolError::NotFound {
                name: name.to_string(),
            }),
            target => {
                // One-shot retry: if the inner Loop tool returned
                // `retryable: true` (mapped to `ToolError::Transport` in
                // `tool_result_to_output`), the helper sleeps 100ms and
                // retries exactly once — but ONLY for tools declared
                // idempotent. Non-idempotent tools (default) skip the
                // retry to avoid duplicate side effects on a timeout that
                // may have already reached the server. R10-safe: no policy
                // selection beyond the static idempotency classification.
                let idempotent = crate::tools::retry::is_idempotent_builtin_name(name);
                let raw_outcome =
                    crate::tools::retry::execute_with_one_shot_backoff(idempotent, || {
                        let input = effective_input.clone();
                        let name_owned = name.to_string();
                        async move {
                            let raw = match target {
                                RoutingTarget::Subagent => {
                                    let st = self.subagent_tool.as_ref().ok_or_else(|| {
                                        ToolError::Execution {
                                            name: name_owned.clone(),
                                            cause: "SubagentTool was checked above but is now None"
                                                .into(),
                                        }
                                    })?;
                                    st.execute(input).await
                                }
                                RoutingTarget::Inner => {
                                    self.inner.execute(&name_owned, input).await
                                }
                                RoutingTarget::Missing => unreachable!(),
                            };
                            Self::tool_result_to_output(&name_owned, raw)
                        }
                    })
                    .await;
                match raw_outcome {
                    Ok(output) => Ok(self.apply_layer_two(name, output).await),
                    Err(err) => Err(Self::sanitize_tool_error(name, err)),
                }
            }
        };

        // Extension `AfterToolCall` / `AfterToolCallFailure` hooks. Observers
        // fire in parallel; Interceptors run sequentially and may rewrite the
        // visible tool output via `update_output:` on the success path.
        // `pre_hook_contexts` from BeforeToolCall are merged in here so they
        // ride along on the same tool result the LLM sees next turn.
        self.run_after_tool_hooks(name, &effective_input, &mut result, pre_hook_contexts)
            .await;

        let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

        // Fire post-hooks. Both for back-compat (v1) and with-duration (v2).
        if let Some(ref hook) = self.hook_decorator {
            hook.after_execute(name, &result);
            hook.after_execute_with_duration(name, &result, duration_ms);
        }

        result
    }

    /// Fire `BeforeToolCall` interceptors. Returns the (possibly rewritten)
    /// input + any `context:` lines the interceptors emitted (to be wrapped
    /// into the tool result), or a `ToolError` when a hook blocks / denies
    /// the call or when an `Ask` decision is not approved by the user.
    async fn run_before_tool_hooks(
        &self,
        name: &str,
        input: Value,
    ) -> Result<(Value, Vec<String>), ToolError> {
        let executor = match self.hook_executor.as_ref() {
            Some(e) if e.hook_count() > 0 => e.clone(),
            _ => return Ok((input, Vec::new())),
        };

        let ctx = self.build_hook_context(name, &input, None, None);
        let (_ctx, hook_result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, ctx)
            .await
            .map_err(|e| ToolError::Execution {
                name: name.to_string(),
                cause: format!("BeforeToolCall hook executor failed: {e}"),
            })?;

        // Hard deny — not retryable.
        if hook_result.denied {
            return Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason: hook_result
                    .deny_reason
                    .unwrap_or_else(|| "denied by hook".to_string()),
            });
        }

        // Soft block — surfaces as an execution error so the LLM can react.
        if hook_result.blocked {
            return Err(ToolError::Execution {
                name: name.to_string(),
                cause: hook_result
                    .block_reason
                    .unwrap_or_else(|| "blocked by hook".to_string()),
            });
        }

        // Ask: route through the approval requester (same seam as
        // `confirm_tools`). Fails closed when no transport is wired.
        if let Some(PermissionDecision::Ask { reason }) = hook_result.permission_decision {
            match &self.approval_requester {
                Some(requester) => {
                    // Mirror confirm_tools: fire PermissionRequest +
                    // Notification observers for user-attention plumbing.
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::PermissionRequest,
                        &self.hook_session_id,
                        vec![
                            ("TOOL_NAME", name.to_string()),
                            ("REASON", reason.clone()),
                        ],
                    )
                    .await;
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::Notification,
                        &self.hook_session_id,
                        vec![
                            ("KIND", "permission_request".to_string()),
                            ("TOOL_NAME", name.to_string()),
                            ("MESSAGE", reason.clone()),
                        ],
                    )
                    .await;
                    let outcome = requester.request_approval(name, &reason).await;
                    if outcome != ApprovalOutcome::Approved {
                        return Err(ToolError::Execution {
                            name: name.to_string(),
                            cause: format!(
                                "Hook requested user confirmation for `{name}` and the \
                                 user did not approve ({outcome:?})."
                            ),
                        });
                    }
                }
                None => {
                    return Err(ToolError::Execution {
                        name: name.to_string(),
                        cause: format!(
                            "Hook requested user confirmation for `{name}` but no \
                             approval channel is available. Do not retry."
                        ),
                    });
                }
            }
        }

        // Last-writer-wins rewrite of the tool input; surface
        // `context:` lines so they actually reach the LLM next turn.
        Ok((
            hook_result.updated_input.unwrap_or(input),
            hook_result.additional_contexts,
        ))
    }

    /// Fire `AfterToolCall` / `AfterToolCallFailure` hooks. Observers run in
    /// parallel; Interceptors run sequentially and may override the visible
    /// tool output via `update_output:` on the success path. Any
    /// `additional_contexts` from BeforeToolCall (`pre_contexts`) plus those
    /// emitted here are wrapped into the tool output as
    /// `<system-reminder>` blocks so the LLM actually sees them next turn.
    async fn run_after_tool_hooks(
        &self,
        name: &str,
        input: &Value,
        result: &mut Result<ToolOutput, ToolError>,
        pre_contexts: Vec<String>,
    ) {
        let executor = match self.hook_executor.as_ref() {
            Some(e) if e.hook_count() > 0 => e.clone(),
            _ => {
                if !pre_contexts.is_empty() {
                    if let Ok(output) = result {
                        output.value = wrap_value_with_hook_contexts(
                            std::mem::take(&mut output.value),
                            &pre_contexts,
                        );
                    }
                }
                return;
            }
        };

        match result {
            Ok(output) => {
                let output_str = output.value.to_string();
                let ctx = self.build_hook_context(name, input, Some(&output_str), Some(false));
                // Fire fire-and-forget Observer-kind hooks in parallel first.
                executor
                    .execute_observers(HookEvent::AfterToolCall, &ctx)
                    .await;
                // Then run Interceptor-kind hooks to harvest `update_output:`
                // — the only post-execution mutation we honor. block / deny
                // semantics make no sense post-hoc and are ignored.
                let mut all_contexts = pre_contexts;
                if let Ok((_ctx, hr)) = executor
                    .execute_interceptors(HookEvent::AfterToolCall, ctx)
                    .await
                {
                    if let Some(text) = hr.updated_output {
                        output.value = Value::String(text);
                    }
                    all_contexts.extend(hr.additional_contexts);
                }
                if !all_contexts.is_empty() {
                    output.value = wrap_value_with_hook_contexts(
                        std::mem::take(&mut output.value),
                        &all_contexts,
                    );
                }
            }
            Err(err) => {
                let err_str = err.to_string();
                let ctx = self.build_hook_context(name, input, Some(&err_str), Some(true));
                executor
                    .execute_observers(HookEvent::AfterToolCallFailure, &ctx)
                    .await;
                // Symmetry with the success path: let Interceptor-kind hooks
                // fire too (e.g., for structured logging), but the failure
                // path is read-only — `update_output:` is ignored because
                // there is no `ToolOutput` to mutate. Pre-hook contexts are
                // intentionally dropped on failure; they referenced an input
                // that never produced a result for the LLM to attach them to.
                let _ = executor
                    .execute_interceptors(HookEvent::AfterToolCallFailure, ctx)
                    .await;
            }
        }
    }

    fn build_hook_context(
        &self,
        name: &str,
        input: &Value,
        tool_output: Option<&str>,
        tool_error: Option<bool>,
    ) -> HookContext {
        let mut ctx = HookContext::new(self.hook_session_id.clone())
            .with_tool_name(name.to_string())
            .with_arguments(input.to_string())
            .with_tool_input(input.to_string());
        if let Some(out) = tool_output {
            ctx = ctx.with_tool_output(out.to_string());
        }
        if let Some(is_err) = tool_error {
            ctx = ctx.with_tool_error(is_err);
        }
        ctx
    }

    /// Apply Layer 2 of the budget pipeline (`compress → persist-if-large
    /// → truncate`) to a successful tool output. Reuses the existing
    /// per-tool compression hook (`compress_tool_output`) and the shared
    /// `result_store` if one is wired; falls back to head+tail truncation
    /// otherwise.
    async fn apply_layer_two(&self, name: &str, mut out: ToolOutput) -> ToolOutput {
        // Compress first: hands JSON to the per-tool summarizer that
        // already exists in `tool_output::compressor`. The text we feed
        // into Layer 2 reflects what the LLM will ultimately see.
        let raw = match &out.value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let compressed = crate::tool_output::compressor::compress_tool_output(name, &raw);

        let explicit = self.inner.max_result_tokens_for(name);
        let budget = crate::tools::result_processing::resolve_result_budget(name, explicit);

        // Generate a per-call file name suffix so concurrent calls to the
        // same tool do not collide on disk. The LLM correlates the result
        // through the surrounding conversation history, not the path.
        let call_id = uuid::Uuid::new_v4().simple().to_string();

        let processed = crate::tools::result_processing::apply_result_budget(
            &call_id,
            name,
            &compressed,
            self.result_store.as_deref(),
            budget,
        );

        // Mark `metadata.truncated` whenever Layer 2 shortened the text.
        if processed.persisted_path.is_some() || processed.text.contains("[output truncated") {
            out.metadata.truncated = true;
        }

        // Extension hooks observe large tool results offloaded to disk.
        if let Some(ref path) = processed.persisted_path {
            if let Some(executor) = self.hook_executor.as_ref() {
                let ctx = HookContext::new(self.hook_session_id.clone())
                    .with_tool_name(name)
                    .with_env("TOOL_CALL_ID", call_id.clone())
                    .with_env("PERSIST_PATH", path.display().to_string())
                    .with_env("PERSIST_REF", processed.text.clone());
                executor
                    .execute_observers(HookEvent::ToolResultPersist, &ctx)
                    .await;
            }
        }

        out.value = Value::String(processed.text);
        out
    }

    /// Wrap a `ToolError` text payload with the standard external-content
    /// fence so reflected user input / scraped remote data inside the
    /// error message cannot smuggle prompt-injection patterns back into
    /// the LLM. The fence labels the source as `tool_error:<tool>` so the
    /// model can pattern-match consistently with `tool_error` outputs
    /// from other channels.
    fn sanitize_tool_error(name: &str, err: ToolError) -> ToolError {
        use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
        // Preserve the original variant so callers can keep matching on
        // `Timeout` / `Transport` / `Execution`; only the `cause` /
        // message string is sanitized.
        match err {
            ToolError::Execution { name: n, cause } => ToolError::Execution {
                name: n,
                cause: wrap_external_content(
                    &cause,
                    ContentSource::ToolError {
                        tool: name.to_string(),
                    },
                ),
            },
            ToolError::Transport { name: n, cause } => ToolError::Transport {
                name: n,
                cause: wrap_external_content(
                    &cause,
                    ContentSource::ToolError {
                        tool: name.to_string(),
                    },
                ),
            },
            // Other variants either have no untrusted payload (NotFound,
            // PermissionDenied, Duplicate, ValidationFailed) or are
            // structured enough not to need wrapping (Timeout). Pass
            // through unchanged.
            other => other,
        }
    }
}

/// Which dispatch branch `execute_inner` is routing into. Kept as a
/// fieldless enum so the retry closure can capture it by value.
#[derive(Copy, Clone)]
enum RoutingTarget {
    Subagent,
    Inner,
    Missing,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::hooks::HookExecutor;
    use crate::extension::{HookAction, HookConfig, HookEvent, HookKind, HookPriority};
    use crate::tools::refresh::ToolRefreshSource;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use crate::sync_primitives::Arc as StdArc;

    // -------------------------------------------------------------------------
    // Stubs
    // -------------------------------------------------------------------------

    /// Noop tool service stub used as `parent_tools` for SubagentTool in tests.
    struct NoopParentTools;

    #[async_trait::async_trait]
    impl ToolService for NoopParentTools {
        async fn execute(&self, _name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
            Err(ToolError::NotFound {
                name: "test".into(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _: &str) -> Option<ToolDefinition> {
            None
        }
    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]> {
            Arc::from([])
        }
    }

    fn in_mem_session() -> Arc<dyn crate::session::service::SessionService> {
        use crate::session::in_process::InProcessActorSessionService;
        use crate::session::store::{
            migrate_add_session_events, SessionEventStore, SqliteEventStore,
        };
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    struct StubTool {
        tool_name: &'static str,
    }

    #[async_trait::async_trait]
    impl LoopTool for StubTool {
        fn name(&self) -> &str {
            self.tool_name
        }
        fn description(&self) -> &str {
            "stub"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, _input: Value) -> LoopToolResult {
            LoopToolResult::Success {
                output: json!({ "tool": self.tool_name }),
            }
        }
    }

    fn make_registry(names: &[&'static str]) -> Arc<LoopToolRegistry> {
        let mut r = LoopToolRegistry::new();
        for &name in names {
            r.register(Box::new(StubTool { tool_name: name }));
        }
        Arc::new(r)
    }

    // Stub refresh source that records whether fetch was called.
    struct StubRefresh {
        has_changes: AtomicBool,
        fetched: StdArc<AtomicBool>,
    }

    impl StubRefresh {
        fn new(has_changes: bool, fetched: StdArc<AtomicBool>) -> Self {
            Self {
                has_changes: AtomicBool::new(has_changes),
                fetched,
            }
        }
    }

    impl ToolRefreshSource for StubRefresh {
        fn poll_changes(&self) -> bool {
            self.has_changes.load(Ordering::Acquire)
        }
        fn fetch_tools(&self) -> Vec<Box<dyn LoopTool>> {
            self.fetched.store(true, Ordering::Release);
            vec![]
        }
    }

    // Stub hook decorator that counts calls.
    struct StubHook {
        before_count: StdArc<AtomicUsize>,
        after_count: StdArc<AtomicUsize>,
    }

    impl ToolHookDecorator for StubHook {
        fn before_execute(&self, _name: &str, _input: &Value) {
            self.before_count.fetch_add(1, Ordering::Relaxed);
        }
        fn after_execute(&self, _name: &str, _output: &Result<ToolOutput, ToolError>) {
            self.after_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    // -------------------------------------------------------------------------
    // Test 1: list filters by allowed set
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn list_filters_by_allowed_set() {
        let registry = make_registry(&["read_file", "write_file"]);
        let allowed = ["read_file".to_string()].into_iter().collect();
        let svc = ScopedToolService::new(registry, allowed);

        let defs = svc.list().await;
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "read_file");
    }

    // -------------------------------------------------------------------------
    // Test 2: list includes subagent tool when set
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn list_includes_subagent_tool_when_set() {
        use crate::agents::background_tracker::BackgroundAgentTracker;
        use crate::agents::AgentRegistry;
        use crate::harness::chain_context::ChainContext;
        use crate::providers::adapter::{ProviderResponse, RequestPayload};
        use crate::providers::AiProvider;
        use std::future::Future;
        use std::pin::Pin;

        struct MockProvider;
        impl AiProvider for MockProvider {
            fn process<'a>(
                &'a self,
                _p: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
            {
                Box::pin(async { Ok(ProviderResponse::text_only("ok".into())) })
            }
            fn name(&self) -> &str {
                "mock"
            }
            fn color(&self) -> &str {
                "#000"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider);
        let chain = ChainContext::new();
        let registry_arc = Arc::new(AgentRegistry::with_builtins());
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let st = Arc::new(crate::agents::subagent_tool::SubagentTool::new(
            provider,
            chain,
            registry_arc,
            tracker,
            in_mem_session(),
            Arc::new(NoopParentTools),
            Arc::new(crate::sandbox::NoopSandbox),
        ));

        let registry = make_registry(&["read_file"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new()).with_subagent_tool(st);

        let defs = svc.list().await;
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.contains(&"subagent"),
            "subagent must be in list; got: {:?}",
            names
        );
    }

    // -------------------------------------------------------------------------
    // Test 2b: subagent survives a non-empty allow set (production path).
    //
    // Regression for the gateway run_loop wiring: `allowed_names` is built
    // from the builtin tool registry's tool definitions, which never contains
    // "subagent" (SubagentTool is attached on top of the registry). Before
    // the is_allowed exemption, list / describe / execute / metadata_schema
    // all silently dropped subagent whenever the allow set was non-empty —
    // i.e. every real LLM-facing call.
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn subagent_survives_non_empty_allow_set() {
        use crate::agents::background_tracker::BackgroundAgentTracker;
        use crate::agents::AgentRegistry;
        use crate::harness::chain_context::ChainContext;
        use crate::providers::adapter::{ProviderResponse, RequestPayload};
        use crate::providers::AiProvider;
        use std::future::Future;
        use std::pin::Pin;

        struct MockProvider;
        impl AiProvider for MockProvider {
            fn process<'a>(
                &'a self,
                _p: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
            {
                Box::pin(async { Ok(ProviderResponse::text_only("ok".into())) })
            }
            fn name(&self) -> &str {
                "mock"
            }
            fn color(&self) -> &str {
                "#000"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider);
        let st = Arc::new(crate::agents::subagent_tool::SubagentTool::new(
            provider,
            ChainContext::new(),
            Arc::new(AgentRegistry::with_builtins()),
            Arc::new(BackgroundAgentTracker::new()),
            in_mem_session(),
            Arc::new(NoopParentTools),
            Arc::new(crate::sandbox::NoopSandbox),
        ));

        // Production-shaped allow set: only registry-known tool names, no
        // "subagent" entry — exactly what gateway run_loop produces.
        let registry = make_registry(&["read_file", "write_file"]);
        let allowed: BTreeSet<String> = ["read_file".into(), "write_file".into()].into();
        let svc = ScopedToolService::new(registry, allowed).with_subagent_tool(st);

        // (1) list() exposes subagent
        let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
        assert!(
            names.iter().any(|n| n == "subagent"),
            "list() must expose subagent under non-empty allow set; got {:?}",
            names
        );

        // (2) describe() returns subagent (used when LLM probes the schema)
        assert!(
            svc.describe("subagent").await.is_some(),
            "describe(subagent) must return Some under non-empty allow set"
        );

        // (3) metadata_schema (LLM-facing) includes subagent
        let schema_names: Vec<String> = svc
            .metadata_schema()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert!(
            schema_names.iter().any(|n| n == "subagent"),
            "metadata_schema must include subagent; got {:?}",
            schema_names
        );

        // (4) execute("subagent", …) is not rejected as NotFound by the
        //     allow-filter (mock provider lets the call complete).
        let result = svc.execute("subagent", json!({ "task": "ping" })).await;
        assert!(
            !matches!(&result, Err(ToolError::NotFound { name }) if name == "subagent"),
            "execute(subagent) must not be NotFound under non-empty allow set; got {:?}",
            result
        );
    }

    // -------------------------------------------------------------------------
    // Test 3: list triggers refresh on first call (when poll_changes is true)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn list_triggers_refresh_on_first_call() {
        let fetched = StdArc::new(AtomicBool::new(false));
        let refresh = Arc::new(StubRefresh::new(true, StdArc::clone(&fetched)));

        let registry = make_registry(&["tool_a"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new()).with_refresh(refresh);

        svc.list().await;
        assert!(
            fetched.load(Ordering::Acquire),
            "fetch_tools must be called when poll_changes returns true"
        );
    }

    // -------------------------------------------------------------------------
    // Test 4: execute routes to subagent tool by name
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn execute_routes_to_subagent_tool_by_name() {
        use crate::agents::background_tracker::BackgroundAgentTracker;
        use crate::agents::AgentRegistry;
        use crate::harness::chain_context::ChainContext;
        use crate::providers::adapter::{ProviderResponse, RequestPayload};
        use crate::providers::AiProvider;
        use std::future::Future;
        use std::pin::Pin;

        struct MockProvider;
        impl AiProvider for MockProvider {
            fn process<'a>(
                &'a self,
                _p: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
            {
                Box::pin(async { Ok(ProviderResponse::text_only("subagent result".into())) })
            }
            fn name(&self) -> &str {
                "mock"
            }
            fn color(&self) -> &str {
                "#000"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider);
        let chain = ChainContext::new();
        let registry_arc = Arc::new(AgentRegistry::with_builtins());
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let st = Arc::new(crate::agents::subagent_tool::SubagentTool::new(
            provider,
            chain,
            registry_arc,
            tracker,
            in_mem_session(),
            Arc::new(NoopParentTools),
            Arc::new(crate::sandbox::NoopSandbox),
        ));

        // Registry has NO "subagent" tool — proves routing goes to st, not inner
        let registry = make_registry(&["read_file"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new()).with_subagent_tool(st);

        // A valid subagent call; the mock provider returns "subagent result"
        let result = svc
            .execute("subagent", json!({ "task": "do something" }))
            .await;
        assert!(
            result.is_ok(),
            "subagent execute should succeed; got: {:?}",
            result.err()
        );
    }

    // -------------------------------------------------------------------------
    // Test 5: execute applies hook decorator (both before and after fire)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn execute_applies_hook_decorator() {
        let before = StdArc::new(AtomicUsize::new(0));
        let after = StdArc::new(AtomicUsize::new(0));
        let hook = Arc::new(StubHook {
            before_count: StdArc::clone(&before),
            after_count: StdArc::clone(&after),
        });

        let registry = make_registry(&["read_file"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new()).with_hook_decorator(hook);

        let _ = svc.execute("read_file", json!({})).await;

        assert_eq!(
            before.load(Ordering::Relaxed),
            1,
            "before_execute must fire once"
        );
        assert_eq!(
            after.load(Ordering::Relaxed),
            1,
            "after_execute must fire once"
        );
    }

    // -------------------------------------------------------------------------
    // Test 6: describe returns from filtered set (allowed / not-allowed)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn describe_returns_from_filtered_set() {
        let registry = make_registry(&["read_file", "write_file"]);
        let allowed = ["read_file".to_string()].into_iter().collect();
        let svc = ScopedToolService::new(registry, allowed);

        // Allowed tool: should return Some
        let def = svc.describe("read_file").await;
        assert!(def.is_some(), "read_file is allowed, must be found");
        assert_eq!(def.unwrap().name, "read_file");

        // Not-in-allowed tool: must return None
        let def = svc.describe("write_file").await;
        assert!(def.is_none(), "write_file is not in allowed set");

        // Totally unknown tool: must return None
        let def = svc.describe("nonexistent").await;
        assert!(def.is_none(), "unknown tool must return None");
    }

    // -------------------------------------------------------------------------
    // Test 7: metadata_schema caches when no refresh signal
    // -------------------------------------------------------------------------

    #[test]
    fn scoped_metadata_schema_caches_when_no_refresh_signal() {
        let registry = make_registry(&["a", "b"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new());
        let s1 = svc.metadata_schema();
        let s2 = svc.metadata_schema();
        assert!(
            Arc::ptr_eq(&s1, &s2),
            "without refresh signal cache should hold across calls"
        );
        assert_eq!(s1.len(), 2);
    }

    // -------------------------------------------------------------------------
    // Test 8: metadata_schema respects allowed filter
    // -------------------------------------------------------------------------

    #[test]
    fn scoped_metadata_schema_respects_allowed_filter() {
        let registry = make_registry(&["a", "b"]);
        let mut allowed = BTreeSet::new();
        allowed.insert("a".to_string());
        let svc = ScopedToolService::new(registry, allowed);
        let s = svc.metadata_schema();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "a");
    }

    // Health gate tests
    // -------------------------------------------------------------------------

    use crate::tool_metadata::{HealthReason, ProbeResult, ToolHealthCache, ToolHealthProbe};
    use std::borrow::Cow;

    struct AlwaysDead;

    #[async_trait::async_trait]
    impl ToolHealthProbe for AlwaysDead {
        async fn probe(&self) -> ProbeResult {
            ProbeResult::Unhealthy {
                reason: HealthReason::DependencyDown(Cow::Borrowed("test fixture")),
                retry_after: None,
            }
        }
    }

    // -------------------------------------------------------------------------
    // Extension HookExecutor wiring
    // -------------------------------------------------------------------------

    /// Capturing tool that echoes the input value it actually receives, so
    /// tests can assert on hook-rewritten input flowing into the underlying
    /// tool implementation.
    struct EchoTool;

    #[async_trait::async_trait]
    impl LoopTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echoes input"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, input: Value) -> LoopToolResult {
            LoopToolResult::Success { output: input }
        }
    }

    fn echo_registry() -> Arc<LoopToolRegistry> {
        let mut r = LoopToolRegistry::new();
        r.register(Box::new(EchoTool));
        Arc::new(r)
    }

    fn make_command_hook(event: HookEvent, kind: HookKind, command: &str) -> HookConfig {
        HookConfig {
            event,
            kind,
            priority: HookPriority::Normal,
            matcher: None,
            actions: vec![HookAction::Command {
                command: command.to_string(),
            }],
            plugin_name: "test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
        }
    }

    /// `apply_layer_two` always stringifies tool output (`Value::String`).
    /// Tests that care about the structured shape parse it back here.
    fn parse_tool_output(value: &Value) -> Value {
        match value {
            Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| value.clone()),
            other => other.clone(),
        }
    }

    #[tokio::test]
    async fn before_tool_hook_block_returns_execution_error() {
        let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
            HookEvent::BeforeToolCall,
            HookKind::Interceptor,
            "echo 'block: blocked by policy'",
        )]));
        let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
            .with_hook_executor(executor, "test-session");

        match svc.execute("echo", json!({})).await {
            Err(ToolError::Execution { name, cause }) => {
                assert_eq!(name, "echo");
                assert!(
                    cause.contains("blocked by policy"),
                    "unexpected cause: {cause}"
                );
            }
            other => panic!("expected Execution error from block hook, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn before_tool_hook_deny_returns_permission_denied() {
        let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
            HookEvent::BeforeToolCall,
            HookKind::Interceptor,
            "echo 'deny: hard policy stop'",
        )]));
        let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
            .with_hook_executor(executor, "test-session");

        match svc.execute("echo", json!({})).await {
            Err(ToolError::PermissionDenied { name, reason }) => {
                assert_eq!(name, "echo");
                assert!(
                    reason.contains("hard policy stop"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected PermissionDenied from deny hook, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn before_tool_hook_update_input_rewrites_args() {
        // Hook rewrites the tool input to a fixed JSON value. The EchoTool
        // returns whatever input it receives, so we can assert by reading the
        // tool output.
        let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
            HookEvent::BeforeToolCall,
            HookKind::Interceptor,
            r#"echo 'update_input: {"path":"/etc/hosts","force":true}'"#,
        )]));
        let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
            .with_hook_executor(executor, "test-session");

        let output = svc
            .execute("echo", json!({ "path": "/tmp/original" }))
            .await
            .expect("execute should succeed when hook only rewrites input");
        // Layer 2 stringifies tool output; parse it back to inspect fields.
        let parsed = parse_tool_output(&output.value);
        assert_eq!(parsed["path"], json!("/etc/hosts"));
        assert_eq!(parsed["force"], json!(true));
    }

    #[tokio::test]
    async fn before_tool_hook_context_wraps_tool_output_for_llm() {
        // BeforeToolCall hook emits `context:` lines. Historically these
        // landed in `HookResult.additional_contexts` but nothing consumed
        // them — the LLM never saw them. This test guards the wiring fix
        // (scoped.rs wraps them as `<system-reminder>` blocks on the tool
        // output value so they reach the model next turn).
        let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
            HookEvent::BeforeToolCall,
            HookKind::Interceptor,
            "echo 'context: file auto-formatted'\necho 'context: lint passed'",
        )]));
        let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
            .with_hook_executor(executor, "test-session");

        let output = svc
            .execute("echo", json!({ "path": "/tmp/x" }))
            .await
            .expect("execute should succeed");
        let s = output
            .value
            .as_str()
            .expect("hook contexts wrap result as a string");
        assert!(s.contains("<system-reminder>"), "missing reminder wrapper: {s}");
        assert!(s.contains("file auto-formatted"), "missing context line: {s}");
        assert!(s.contains("lint passed"), "missing context line: {s}");
    }

    #[tokio::test]
    async fn after_tool_hook_observer_fires_on_success() {
        // Observer writes the tool name to a tempfile so we can prove it
        // fired with the right context. Run inside a per-test tempdir to
        // avoid interference when tests run in parallel.
        let dir = tempfile::tempdir().expect("create tempdir");
        let marker = dir.path().join("after.flag");
        let marker_str = marker.to_string_lossy().to_string();
        let cmd = format!(r#"printf '%s' "$TOOL_NAME" > '{marker_str}'"#);
        let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
            HookEvent::AfterToolCall,
            HookKind::Observer,
            &cmd,
        )]));
        let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
            .with_hook_executor(executor, "test-session");

        let _ = svc
            .execute("echo", json!({}))
            .await
            .expect("execute should succeed");

        let contents = std::fs::read_to_string(&marker).expect("observer must have written marker");
        assert_eq!(contents.trim(), "echo");
    }

    #[tokio::test]
    async fn after_tool_failure_hook_fires_when_tool_errors() {
        // Construct a registry with a tool that always errors.
        struct ErrTool;
        #[async_trait::async_trait]
        impl LoopTool for ErrTool {
            fn name(&self) -> &str {
                "boom"
            }
            fn description(&self) -> &str {
                "always errors"
            }
            fn schema(&self) -> Value {
                json!({ "type": "object" })
            }
            async fn execute(&self, _input: Value) -> LoopToolResult {
                LoopToolResult::Error {
                    error: "kaboom".to_string(),
                    retryable: false,
                }
            }
        }
        let mut r = LoopToolRegistry::new();
        r.register(Box::new(ErrTool));
        let registry = Arc::new(r);

        let dir = tempfile::tempdir().expect("create tempdir");
        let marker = dir.path().join("fail.flag");
        let marker_str = marker.to_string_lossy().to_string();
        let cmd = format!(r#"printf '%s' "$TOOL_NAME" > '{marker_str}'"#);
        let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
            HookEvent::AfterToolCallFailure,
            HookKind::Observer,
            &cmd,
        )]));
        let svc = ScopedToolService::new(registry, BTreeSet::new())
            .with_hook_executor(executor, "test-session");

        let err = svc
            .execute("boom", json!({}))
            .await
            .expect_err("tool returns error");
        assert!(matches!(err, ToolError::Execution { .. }));

        let contents =
            std::fs::read_to_string(&marker).expect("failure observer must have written marker");
        assert_eq!(contents.trim(), "boom");
    }

    #[tokio::test]
    async fn no_hooks_means_no_change_in_behaviour() {
        // Sanity: without `.with_hook_executor`, ScopedToolService behaves
        // identically to before (regression guard).
        let svc = ScopedToolService::new(echo_registry(), BTreeSet::new());
        let out = svc
            .execute("echo", json!({ "k": "v" }))
            .await
            .expect("execute succeeds");
        // Layer 2 stringifies tool output; parse it back to compare structurally.
        assert_eq!(parse_tool_output(&out.value), json!({ "k": "v" }));
    }

    #[tokio::test]
    async fn list_strips_unhealthy_tools() {
        let registry = make_registry(&["alive", "dead"]);
        let health = Arc::new(ToolHealthCache::new());
        health.register_probe("dead", Arc::new(AlwaysDead));
        health.refresh("dead").await;
        let svc = ScopedToolService::new(registry, BTreeSet::new()).with_health(health);
        let defs = svc.list().await;
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"alive"));
        assert!(!names.contains(&"dead"), "got: {names:?}");
    }

    #[test]
    fn metadata_schema_strips_unhealthy_tools_and_invalidates_on_flip() {
        // Driven sync from outside an async runtime — populate the cache via
        // a small block_on island.
        let registry = make_registry(&["alive", "dead"]);
        let health = Arc::new(ToolHealthCache::new());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            health.register_probe("dead", Arc::new(AlwaysDead));
            health.refresh("dead").await;
        });
        let svc =
            ScopedToolService::new(registry, BTreeSet::new()).with_health(Arc::clone(&health));

        let s1 = svc.metadata_schema();
        let names: Vec<&str> = s1.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"alive"));
        assert!(!names.contains(&"dead"), "first call should strip dead");

        // Flip "dead" healthy via invalidation; the schema cache must
        // invalidate so the next call surfaces "dead" again.
        health.invalidate_all();
        let s2 = svc.metadata_schema();
        assert!(
            !Arc::ptr_eq(&s1, &s2),
            "cache must rotate when health generation flips"
        );
        let names2: Vec<&str> = s2.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names2.contains(&"dead"),
            "after invalidation, dead reappears; got: {names2:?}"
        );
    }

    // -------------------------------------------------------------------------
    // CRITICAL-1 — describe() populates per-tool budget + idempotency metadata
    // from the static tables, so the harness's per-tool wall-clock budget can
    // actually fire (it was always `None` while metadata was hardcoded default).
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn describe_populates_builtin_budget_metadata() {
        // `memory_search` is in both BUILTIN_TOOL_BUDGETS_MS (5_000ms) and
        // IDEMPOTENT_BUILTIN_TOOLS.
        let registry = make_registry(&["memory_search"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new());
        let def = svc.describe("memory_search").await.expect("tool present");
        assert_eq!(def.metadata.max_duration_ms, Some(5_000));
        assert!(def.metadata.idempotent);
    }

    #[tokio::test]
    async fn describe_leaves_metadata_default_for_unbudgeted_tool() {
        // A tool absent from both tables keeps the legacy `None` budget so it
        // inherits the harness-wide turn_timeout fallback.
        let registry = make_registry(&["some_custom_tool"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new());
        let def = svc
            .describe("some_custom_tool")
            .await
            .expect("tool present");
        assert_eq!(def.metadata.max_duration_ms, None);
        assert!(!def.metadata.idempotent);
    }
}
