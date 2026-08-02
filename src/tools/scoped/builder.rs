//! Construction / builder helpers + small definition-shape utilities for
//! [`ScopedToolService`].
//!
//! Lives alongside [`super`] so the trait impl in `mod.rs` and the dispatch
//! body in [`super::dispatch`] can share these inherent helpers.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::agents::subagent_tool::SubagentTool;
use crate::extension::hooks::HookExecutor;
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tool_metadata::ToolHealthCache;
use crate::tools::runtime::{LoopTool, LoopToolRegistry};
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};

use super::traits::{ToolDefinitionRewriter, ToolHookDecorator};
use super::ScopedToolService;

impl ScopedToolService {
    /// Create a new `ScopedToolService`.
    ///
    /// `allowed` — set of tool names visible through this service. Empty = allow all.
    #[must_use]
    pub fn new(inner: Arc<LoopToolRegistry>, allowed: BTreeSet<String>) -> Self {
        Self {
            inner,
            allowed,
            subagent_tool: None,
            hook_decorator: None,
            hook_executor: None,
            hook_session_id: String::new(),
            approval_requester: None,
            config_approval_requester: None,
            turn_context: None,
            result_store: None,
            schema_cache: arc_swap::ArcSwap::from_pointee(None),
            cache_generation: std::sync::atomic::AtomicU64::new(0),
            health: None,
            last_health_generation: std::sync::atomic::AtomicU64::new(0),
            definition_rewriters: Vec::new(),
            deferred: super::DeferredTools::empty(),
            last_deferred_generation: std::sync::atomic::AtomicU64::new(0),
            tool_permissions: None,
            exec_tier: None,
            unattended: false,
        }
    }

    /// Attach the merged tool permission policy (global → agent → channel,
    /// most restrictive wins). `Deny` tools are hidden from listings and
    /// rejected at execute; `Ask` tools route through the confirmation gate.
    ///
    /// Callers should skip this for an all-default policy (`Allow` + no
    /// overrides) so the hot path stays a `None` check.
    pub fn with_tool_permissions(
        mut self,
        permissions: crate::config::types::policies::ToolPermissionsConfig,
    ) -> Self {
        self.tool_permissions = Some(permissions);
        self
    }

    /// Attach the effective execution tier for this turn. The tier decides
    /// every tool no explicit override names — see [`Self::permission_for`].
    #[must_use]
    pub fn with_exec_tier(mut self, tier: crate::config::types::policies::ExecTier) -> Self {
        self.exec_tier = Some(tier);
        self
    }

    /// Mark this service as serving an unattended (autonomous continuation)
    /// run. Confirm-gated tools then fail closed. See [`Self::unattended`].
    #[must_use]
    pub fn with_unattended(mut self, unattended: bool) -> Self {
        self.unattended = unattended;
        self
    }

    /// Attach a [`ToolDefinitionRewriter`]. Rewriters run in attachment
    /// order on every `list()` / `metadata_schema()` build (the latter
    /// only on cache miss; call [`Self::bump_cache_generation`] when
    /// external state that the rewriter consults has changed).
    pub fn with_definition_rewriter(mut self, rewriter: Arc<dyn ToolDefinitionRewriter>) -> Self {
        self.definition_rewriters.push(rewriter);
        self
    }

    /// Attach the shared deferred tier. The SAME `Arc` must be handed to
    /// `ToolSearchTool`, which shrinks it when the model discovers a tool —
    /// that is what makes a deferred tool callable rather than merely visible.
    #[must_use]
    pub fn with_deferred(mut self, deferred: Arc<super::DeferredTools>) -> Self {
        self.deferred = deferred;
        self
    }

    /// Whether `name` is in the deferred tier (dropped from LLM-visible lists).
    pub(super) fn is_deferred(&self, name: &str) -> bool {
        self.deferred.is_deferred(name)
    }

    /// Invalidate the cached `metadata_schema()` output so the next call
    /// re-runs the rewriter chain. Use this when external state that a
    /// rewriter reads (e.g. an agent's permission set, an extension's
    /// description override) has changed since the last build.
    pub fn bump_cache_generation(&self) {
        self.cache_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// Apply every attached [`ToolDefinitionRewriter`] to each `def` in
    /// `defs`. No-op when no rewriters are attached.
    pub(super) fn apply_definition_rewriters(&self, defs: &mut [ToolDefinition]) {
        if self.definition_rewriters.is_empty() {
            return;
        }
        for def in defs.iter_mut() {
            for rewriter in &self.definition_rewriters {
                rewriter.rewrite(def);
            }
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

    /// Wire the transport that obtains user confirmation.
    ///
    /// When a confirm-gated tool is invoked — one declaring
    /// `LoopTool::requires_confirmation()`, or one the merged permission policy
    /// resolves to `Ask` — `execute()` first routes a confirmation request
    /// through `requester`; the tool runs only on an `Approved` outcome. With
    /// no requester wired, confirm-gated tools fail closed.
    pub fn with_confirmation(mut self, requester: Arc<dyn ApprovalRequester>) -> Self {
        self.approval_requester = Some(requester);
        self
    }

    /// Wire the operator-targeted config approval requester (Phase 2b sudo).
    pub fn with_config_approval(mut self, requester: Arc<dyn ApprovalRequester>) -> Self {
        self.config_approval_requester = Some(requester);
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

    /// Attach a hook decorator for observing tool execution.
    pub fn with_hook_decorator(mut self, hook: Arc<dyn ToolHookDecorator>) -> Self {
        self.hook_decorator = Some(hook);
        self
    }

    /// Attach an extension-shipped `HookExecutor`. Wires `BeforeToolCall`
    /// interceptors (block / deny / ask / `update_input`) and
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
    // Helpers (shared with the trait impl in mod.rs and dispatch.rs)
    // -------------------------------------------------------------------------

    /// Effective permission for `name` — the loop's enforcement chokepoint,
    /// which every permission gate here funnels through.
    ///
    /// Precedence, most specific first:
    /// 1. **explicit exact-name** entry in the merged [`ToolPermissionsConfig`]
    ///    — an operator who names a tool has made a deliberate decision;
    /// 2. **explicit glob** entry (same call: [`ToolPermissionsConfig::resolve_explicit`]);
    /// 3. the configured `default` (`Allow` when no policy is attached),
    ///    TIGHTENED by the exec tier's rule ([`ExecTier::rule_for`], read off the
    ///    tool's declared metadata, never off its name). The tier can raise a
    ///    tool to `Ask`; it can never lower a `Deny`.
    ///
    /// The precedence itself lives in
    /// [`crate::config::types::policies::effective_permission`] — shared with the
    /// gateway slash-command fast path so the two surfaces cannot drift.
    ///
    /// [`ToolPermissionsConfig`]: crate::config::types::policies::ToolPermissionsConfig
    /// [`ToolPermissionsConfig::resolve_explicit`]: crate::config::types::policies::ToolPermissionsConfig::resolve_explicit
    /// [`ExecTier::rule_for`]: crate::config::types::policies::ExecTier::rule_for
    pub(super) fn permission_for(&self, name: &str) -> crate::extension::PermissionAction {
        crate::config::types::policies::effective_permission(
            self.tool_permissions.as_ref(),
            self.exec_tier,
            self.tool_facts(name),
        )
    }

    /// The permission an override entry explicitly states for `name`, if any.
    fn explicit_permission(&self, name: &str) -> Option<crate::extension::PermissionAction> {
        self.tool_permissions
            .as_ref()
            .and_then(|p| p.resolve_explicit(name))
    }

    /// The tool's DECLARED facts, as the tier rules consume them. An unknown
    /// name yields the fail-closed shape (non-idempotent = mutating), which is
    /// what makes the `Ask` tier hold for tools nobody has classified.
    pub(super) fn tool_facts<'a>(
        &self,
        name: &'a str,
    ) -> crate::config::types::policies::ToolFacts<'a> {
        crate::config::types::policies::ToolFacts {
            name,
            // The declaration seam answers for every tool that reached the
            // registry (builtins via the allowlist, MCP via the server's
            // hints). The name-list fallback keeps any builtin NOT routed
            // through `RegistryToolAdapter` resolving exactly as before; a
            // tool unknown to both stays `false`, so `Ask` is fail-closed.
            idempotent: self.inner.is_idempotent(name)
                || crate::tools::retry::is_idempotent_builtin_name(name),
            requires_approval: self.inner.requires_confirmation(name),
        }
    }

    /// `true` when this *call*'s arguments trip the tier's destructive-argument
    /// hard filter (`file_ops` delete/move/…), which a name-keyed rule cannot
    /// see. Skipped when an override explicitly names the tool: the operator
    /// already decided, and the tier — argument filter included — only speaks
    /// when nobody did.
    pub(super) fn tier_asks_for_arguments(&self, name: &str, input: &Value) -> bool {
        if self.explicit_permission(name).is_some() {
            return false;
        }
        self.exec_tier
            .is_some_and(|tier| tier.asks_for_arguments(name, input))
    }

    /// `true` when the permission policy denies `name` outright.
    pub(super) fn is_permission_denied(&self, name: &str) -> bool {
        self.permission_for(name) == crate::extension::PermissionAction::Deny
    }

    /// `true` when the permission policy requires confirmation for `name`.
    pub(super) fn is_permission_ask(&self, name: &str) -> bool {
        self.permission_for(name) == crate::extension::PermissionAction::Ask
    }

    pub(super) fn is_allowed(&self, name: &str) -> bool {
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

    /// Build `ToolDefinitionMetadata` for a loop tool from what the tool
    /// declares plus the static budget + idempotency tables — the same data
    /// `BuiltinHandler::definition()` surfaces through the handler path.
    /// `ScopedToolService` is the harness's production `ToolService`, so
    /// without this the per-tool wall-clock budget consulted by `act.rs`
    /// via `describe()` would always be `None` and never fire.
    ///
    /// `declared_budget_ms` is [`crate::tools::runtime::LoopTool::max_duration_ms`]
    /// and wins over the table: an MCP tool's budget is only knowable from its
    /// owning server's configured request timeout. The resolution always yields
    /// `Some`. While the table was the only source, every tool outside it (all
    /// MCP / plugin / skill tools, ~100 builtins) advertised no budget at all —
    /// and the harness reads a missing budget as a run-level stall rather than
    /// a recoverable tool error.
    ///
    /// `concurrent_safe` flows through from the inner
    /// [`crate::tools::runtime::ToolDefinition::concurrent_safe`] so the
    /// harness can advertise / inspect parallel-safety per tool. The
    /// authoritative per-call dispatch decision still goes through
    /// [`crate::tools::service::ToolService::call_concurrency_claim`].
    pub(super) fn builtin_metadata(
        name: &str,
        concurrent_safe: bool,
        requires_approval: bool,
        declared_budget_ms: Option<u64>,
    ) -> ToolDefinitionMetadata {
        ToolDefinitionMetadata {
            idempotent: crate::tools::retry::is_idempotent_builtin_name(name),
            max_duration_ms: Some(crate::tools::budget::resolve_tool_budget_ms(
                name,
                declared_budget_ms,
            )),
            concurrent_safe,
            requires_approval,
            ..ToolDefinitionMetadata::default()
        }
    }

    pub(super) fn loop_tool_to_definition(tool: &dyn LoopTool) -> ToolDefinition {
        let name = tool.name();
        let concurrent_safe = tool.is_concurrent_safe(&Value::Null);
        ToolDefinition {
            name: name.to_string(),
            description: tool.description().to_string(),
            input_schema: tool.schema(),
            source: ToolSource::Builtin,
            metadata: Self::builtin_metadata(
                name,
                concurrent_safe,
                tool.requires_confirmation(),
                tool.max_duration_ms(),
            ),
        }
    }

    pub(super) fn subagent_definition(tool: &SubagentTool) -> ToolDefinition {
        ToolDefinition {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.schema(),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata {
                // The subagent tool is attached beside the loop registry, so it
                // never passed through `builtin_metadata` and shipped a hardcoded
                // default: no budget → the harness killed the PARENT run on any
                // delegation slower than its own fallback.
                max_duration_ms: Some(crate::tools::budget::resolve_tool_budget_ms(
                    tool.name(),
                    tool.max_duration_ms(),
                )),
                ..ToolDefinitionMetadata::default()
            },
        }
    }

    pub(super) fn tool_result_to_output(
        name: &str,
        result: crate::tools::runtime::ToolResult,
    ) -> Result<ToolOutput, ToolError> {
        use crate::session::events::ToolOutputMetadata;
        use crate::tools::runtime::ToolResult;
        match result {
            ToolResult::Success { output } => Ok(ToolOutput {
                value: output,
                metadata: ToolOutputMetadata::default(),
            }),
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
