//! `ToolService` — consumer-side façade over tool dispatch.
//!
//! See: docs/superpowers/specs/2026-04-18-tool-service-facade-design.md

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::session::events::ToolOutput;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {name}")]
    NotFound { name: String },

    #[error("permission denied for tool {name}: {reason}")]
    PermissionDenied { name: String, reason: String },

    #[error("invalid input for tool {name}: {cause}")]
    ValidationFailed { name: String, cause: String },

    #[error("tool {name} execution failed: {cause}")]
    Execution { name: String, cause: String },

    #[error("tool {name} timed out after {elapsed_ms}ms")]
    Timeout { name: String, elapsed_ms: u64 },

    /// The approval card expired with nobody answering it. Distinct from
    /// `PermissionDenied` and from a plain `Execution` failure, because nobody
    /// said no — nobody said anything, and the human may simply be away.
    /// `DenialLedger::record_denial` already drops a `DenialReason::Timeout`
    /// for exactly this reason ("an expired card is not a decision"); folding
    /// the expiry into a non-retryable error one layer up put the permanent ban
    /// back and told the model the user had refused.
    #[error(
        "approval for tool {name} expired after {waited_ms}ms with no response — nobody \
         answered; the user did not decline this call"
    )]
    ApprovalExpired { name: String, waited_ms: u64 },

    #[error("tool {name} transport error: {cause}")]
    Transport { name: String, cause: String },

    /// The run was cancelled while this call was in flight (or before it was
    /// polled). Distinct from every other variant for the same reason
    /// `ApprovalExpired` is: nobody judged the call. Collapsing it into
    /// `Execution` made `is_retryable` false, which entered the call into the
    /// harness's cross-batch failure memo — so pressing stop once *banned* that
    /// exact call for the rest of the run — and handed the model a persistence
    /// hint telling it to climb the tool ladder because the user had pressed
    /// stop.
    #[error("tool {name} was cancelled — the run was stopped, this is not a verdict on the call")]
    Cancelled { name: String },

    #[error("duplicate tool name: {name}")]
    Duplicate { name: String },

    #[error("{0}")]
    Other(String),
}

impl ToolError {
    /// "This failure was not a verdict on the call itself." Two consumers read
    /// it, and they want different things from it:
    ///
    /// - `tools::retry::execute_with_one_shot_backoff` respins the call
    ///   (subject to per-tool idempotence). Only `Timeout` / `Transport` can
    ///   reach it: it lives *below* the approval gate, so an
    ///   `ApprovalExpired` is returned long before the retry layer is entered
    ///   and is never silently respun into a second approval card. That helper
    ///   excludes `Cancelled` explicitly — respinning after `/stop` would sleep
    ///   the backoff and fail again.
    /// - the harness's cross-batch failure memo (`agent/act.rs`) refuses to
    ///   *ban* a call whose error was retryable. An expired approval must not
    ///   be banned — the human was away, they did not refuse. Neither must a
    ///   cancelled one: the user stopped the run, they did not judge the call.
    ///
    /// For richer, prompt-side classification (rate-limited,
    /// unauthorized, blocked-by-policy, …) see [`Self::kind`].
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout { .. }
                | Self::Transport { .. }
                | Self::ApprovalExpired { .. }
                | Self::Cancelled { .. }
        )
    }

    /// Coarse, stable taxonomy used to render the LLM-facing routing
    /// hint in `SessionEvent::ToolError`. Delegates to
    /// [`crate::tools::error_kind::classify_tool_error`].
    ///
    /// This is a strict superset of `is_retryable` — every transient
    /// variant returns a `kind()` whose `is_transient()` is `true`,
    /// but the kind further distinguishes "switch source"
    /// (Unauthorized/BlockedByPolicy) from "switch tool"
    /// (Timeout/Transport).
    #[must_use]
    pub fn kind(&self) -> crate::tools::error_kind::ToolErrorKind {
        crate::tools::error_kind::classify_tool_error(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolSource {
    Builtin,
    Mcp { server_id: String },
    Extension { plugin_id: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolDefinitionMetadata {
    #[serde(default)]
    pub hidden_from_llm: bool,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    /// True when re-running this tool with the same input is safe even if
    /// the previous attempt may have reached the server. Read-only / pure
    /// query tools (`memory_search`, search, `web_fetch`, `recall_context`) set
    /// this to true; side-effecting tools (write/exec/send_*) leave it
    /// false. Consumed by `tools::retry::execute_with_one_shot_backoff` to
    /// gate retries on `Timeout` / `Transport` errors — non-idempotent
    /// tools never auto-retry to avoid duplicate side effects.
    #[serde(default)]
    pub idempotent: bool,
    /// Per-tool wall-clock execution budget.
    ///
    /// Every definition a production `ToolService` publishes carries one:
    /// the builders resolve it through `tools::budget::resolve_tool_budget_ms`
    /// (the tool's own `LoopTool::max_duration_ms` → `BUILTIN_TOOL_BUDGETS_MS`
    /// → `DEFAULT_TOOL_BUDGET_MS`), so MCP / plugin / skill tools and the ~100
    /// builtins outside the table are budgeted too. That matters because the
    /// harness treats an overrun of a *declared* budget as a recoverable tool
    /// error, and a `None` budget as a run-level stall — an unbudgeted tool
    /// turned one slow call into an aborted run.
    ///
    /// Still `Option` because the type is also the wire form (`#[serde(default)]`
    /// for legacy JSON) and test doubles need a "says nothing" value.
    #[serde(default)]
    pub max_duration_ms: Option<u64>,
    /// Static "always safe under parallel dispatch" hint, mirroring the
    /// inner [`crate::tools::runtime::ToolDefinition::concurrent_safe`].
    /// Tools that mutate shared state (`file_write`, bash, `code_exec`,
    /// send_*) leave this `false`; pure read-only queries set it `true`.
    /// The harness uses [`ToolService::call_concurrency_claim`] for the
    /// authoritative per-call answer at dispatch time, since some tools
    /// (e.g. `file_ops`) flip based on the operation in their input.
    #[serde(default)]
    pub concurrent_safe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub source: ToolSource,
    #[serde(default)]
    pub metadata: ToolDefinitionMetadata,
}

#[async_trait]
pub trait ToolService: Send + Sync + 'static {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError>;

    async fn list(&self) -> Vec<ToolDefinition>;

    /// Every tool the model may legitimately CALL — the visible list plus the
    /// deferred tier, which is hidden from the tool array but stays executable.
    ///
    /// Distinct from [`Self::list`] because tool-name repair must be judged
    /// against what is *dispatchable*, not what is *displayed*. Building the
    /// repairer's candidate set from `list()` meant a deferred tool's correct
    /// name missed the Exact tier, leaving the Fuzzy tier free to silently
    /// rewrite it into a different, resident tool — a correct call turned into
    /// the wrong action.
    ///
    /// Defaults to `list()`: for services with no deferred tier the two sets are
    /// identical.
    async fn dispatchable_list(&self) -> Vec<ToolDefinition> {
        self.list().await
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition>;

    /// Return the metadata-form tool schema the LLM expects, as an `Arc`
    /// for O(1) per-turn cloning. Implementations cache internally and
    /// invalidate on their own mutation signal (e.g., registry snapshot
    /// change for `CoreDispatch`, MCP `poll_changes()` for `ScopedToolService`).
    ///
    /// REQUIRED — no default impl. A default returning empty would silently
    /// hide the LLM's tool list on any forgotten override. Test mocks must
    /// also implement, typically returning `Arc::from([])`.
    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]>;

    /// The execution tier this service will actually enforce on the **next**
    /// call, when it knows one.
    ///
    /// Exists so a prompt can state the approval regime without deriving it a
    /// second time. The sub-agent spawner is the caller that forced it: a
    /// spawned child gets no tool service of its own — `subagent_spawner`
    /// wraps the PARENT's `ScopedToolService` (`parent_view_for_children`), so
    /// the tier and the very same `PlanGate` `Arc` are enforced on every call
    /// the child makes. The child's prompt nonetheless said nothing about it,
    /// because the spawner held only an `Arc<dyn ToolService>` and had no way
    /// to ask. Under [`ExecTier::Plan`](crate::config::types::policies::ExecTier::Plan)
    /// — which `subagent` is deliberately reachable from, so a plan for a
    /// large codebase can be researched by delegation — that silence costs the
    /// child its whole iteration budget discovering by refusal what one
    /// sentence would have told it.
    ///
    /// **Asking the enforcer is not a second derivation.** `ScopedToolService`
    /// answers with the identical call its own gate makes
    /// (`effective_exec_tier`), which is why this is a trait method rather
    /// than a tier snapshot threaded through `SpawnRequest`: a snapshot taken
    /// at spawn time would go stale the moment a human approves a plan, while
    /// this reads the live gate.
    ///
    /// `None` = this service has no tier wired (test stubs, pre-boot, the
    /// null service). Callers must render nothing rather than a default —
    /// stating a regime nobody enforces is the expensive half of 判据 §0.
    ///
    /// Every production DECORATOR must forward it. The default is `None`, so
    /// a decorator that forgets degrades to silence rather than to a wrong
    /// answer — safe, but silent, which is why each one has a named test.
    fn enforced_exec_tier(&self) -> Option<crate::config::types::policies::ExecTier> {
        None
    }

    /// Resource-scope-aware concurrency claim for the named call, given the
    /// concrete input the LLM emitted. The harness parallel fast path admits a
    /// batch only when [`crate::tools::concurrency::batch_parallelizable`]
    /// returns `true` for the claims of every call — letting disjoint-scope
    /// mutations (e.g. writes to different files) run concurrently while
    /// same-path or whole-world mutations fall back to serial.
    ///
    /// Conservative default: whole-world exclusive. Every production impl
    /// (`ScopedToolService` / `AllowlistToolService` / `McpScopedToolService`)
    /// overrides this with real registry-visible path scopes.
    async fn call_concurrency_claim(
        &self,
        _name: &str,
        _input: &serde_json::Value,
    ) -> crate::tools::concurrency::ConcurrencyClaim {
        crate::tools::concurrency::ConcurrencyClaim::global()
    }

    /// Execute a tool call with an opencode-parity `AbortSignal` token.
    ///
    /// The harness Act phase forks a per-call child token from the run's
    /// cancellation token and threads it here. Production wrappers
    /// (`ScopedToolService`, `McpScopedToolService`, `AllowlistToolService`)
    /// override this to thread `cancel` all the way to
    /// [`crate::tools::runtime::LoopTool::execute`] so individual tools can
    /// `select!` against it or have their futures dropped on cancel
    /// (`kill_on_drop` for subprocesses, reqwest abort for HTTP).
    ///
    /// The default impl discards `cancel` and forwards to [`Self::execute`]
    /// — sufficient for test stubs that don't model cancellation. Anywhere
    /// a real run loop calls a tool, the production wrapper's override
    /// fires the token end-to-end.
    async fn execute_with_cancel(
        &self,
        name: &str,
        input: serde_json::Value,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.execute(name, input).await
    }
}

/// Convert a slice of loop-side `ToolDefinition`s into the `tool_metadata`
/// `ToolDefinition` representation expected by LLM providers.
///
/// This is the single source of truth for the conversion. Per Stage 2
/// of the 12-module roadmap, `ToolService` impls cache the output of this
/// helper (keyed on their internal mutation signal) so each turn's tool
/// list is an O(1) `Arc::clone` rather than an O(n) `Vec` allocation.
///
/// Information loss (e.g., `ToolSource::Mcp` collapses to `category: Builtin`,
/// `metadata.requires_approval` is dropped) is preserved as-is from the
/// pre-Stage-2 behavior. Fixing the lossy mapping is out of Stage 2 scope.
#[must_use]
pub fn to_metadata_form(defs: &[ToolDefinition]) -> Arc<[crate::tool_metadata::ToolDefinition]> {
    defs.iter()
        .map(|def| crate::tool_metadata::ToolDefinition {
            name: def.name.clone(),
            description: def.description.clone(),
            parameters: def.input_schema.clone(),
            requires_confirmation: false,
            category: crate::tool_metadata::ToolCategory::Builtin,
            strict: false,
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
mod metadata_form_tests {
    use super::*;
    use crate::tool_metadata::{ToolCategory, ToolDefinition as MetadataToolDefinition};
    use serde_json::json;

    fn loop_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("desc {name}"),
            input_schema: json!({"type": "object"}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    #[test]
    fn empty_input_yields_empty_arc() {
        let out = to_metadata_form(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn single_def_converts_field_by_field() {
        let inputs = vec![loop_def("alpha")];
        let out = to_metadata_form(&inputs);
        assert_eq!(out.len(), 1);
        let d: &MetadataToolDefinition = &out[0];
        assert_eq!(d.name, "alpha");
        assert_eq!(d.description, "desc alpha");
        assert_eq!(d.parameters, json!({"type": "object"}));
        assert!(!d.requires_confirmation);
        assert!(matches!(d.category, ToolCategory::Builtin));
        assert!(!d.strict);
    }

    #[test]
    fn preserves_order_for_multi_input() {
        let inputs = vec![loop_def("a"), loop_def("b"), loop_def("c")];
        let out = to_metadata_form(&inputs);
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn metadata_max_duration_ms_round_trips_through_json() {
        let original = ToolDefinitionMetadata {
            hidden_from_llm: false,
            requires_approval: false,
            tags: Vec::new(),
            idempotent: true,
            max_duration_ms: Some(5_000),
            concurrent_safe: false,
        };
        let serialized = serde_json::to_string(&original).unwrap();
        let parsed: ToolDefinitionMetadata = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed.max_duration_ms, Some(5_000));
    }

    #[test]
    fn metadata_max_duration_ms_defaults_to_none_when_field_absent() {
        let legacy_json =
            r#"{"hidden_from_llm":false,"requires_approval":false,"tags":[],"idempotent":false}"#;
        let parsed: ToolDefinitionMetadata = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed.max_duration_ms, None);
        assert!(
            !parsed.concurrent_safe,
            "absent concurrent_safe must default to false on legacy JSON",
        );
    }
}
