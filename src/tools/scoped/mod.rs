//! `ScopedToolService` — bridges `LoopToolRegistry` + `SubagentTool` to `ToolService`.
//!
//! Adapts the Gateway-side `LoopToolRegistry` (with optional `SubagentTool`,
//! `ToolRefreshSource`, and hook decorator) to the `ToolService` trait consumed
//! by `AgentHarness`. This is a read-only adapter; it does not modify the
//! underlying registry.
//!
//! ## Layout
//! - [`traits`] — public extension points (`ToolDefinitionRewriter`, `ToolHookDecorator`)
//! - [`builder`] — `new` + every `with_*` + small shape/helper utilities
//! - [`gate_chain`] — the ordered rule chain that decides (and NAMES) what gates a call
//! - [`dispatch`] — execute pipeline (`execute_inner` + hook seams + Layer-2 + sanitize)
//! - [`artifact_harvest`] — settles `_media` tool output into the artifact store
//! - [`ledger`] — what the signed operation ledger records at this chokepoint
//! - this module — `ScopedToolService` struct + `ToolService` trait impl

// `pub(crate)` for one reason: the slash-command fast path bypasses
// `ScopedToolService` entirely and has to reach the same harvest, or its media
// would be the only kind that never lands in the artifact store.
pub(crate) mod artifact_harvest;
mod builder;
mod cat_guard;
mod deferred;
mod dispatch;
mod gate_chain;
mod ledger;
mod progressive_disclosure;
mod traits;

#[cfg(test)]
mod tests;

pub use deferred::DeferredTools;
pub use progressive_disclosure::ProgressiveDisclosureRewriter;
pub use traits::{ToolDefinitionRewriter, ToolHookDecorator};

use std::collections::BTreeSet;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agents::subagent_tool::SubagentTool;
use crate::extension::hooks::HookExecutor;
use crate::sandbox::exec_approval::gate::ApprovalRequester;
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tool_metadata::ToolHealthCache;
use crate::tools::runtime::{LoopTool, LoopToolRegistry};
use crate::tools::service::{to_metadata_form, ToolDefinition, ToolError, ToolService, ToolSource};

// =============================================================================
// ScopedToolService
// =============================================================================

/// Adapts a `LoopToolRegistry` snapshot to the `ToolService` consumer trait.
///
/// Construction:
/// ```text
/// ScopedToolService::new(registry, allowed)
///     .with_subagent_tool(tool)
///     .with_hook_decorator(decorator)
/// ```
///
/// `allowed` is a set of permitted tool names. Empty = allow-all.
pub struct ScopedToolService {
    pub(super) inner: Arc<LoopToolRegistry>,
    pub(super) allowed: BTreeSet<String>,
    pub(super) subagent_tool: Option<Arc<SubagentTool>>,
    pub(super) hook_decorator: Option<Arc<dyn ToolHookDecorator>>,
    /// Extension-shipped hook executor. Fires `BeforeToolCall` interceptors
    /// (`block/deny/ask/update_input`) and `AfterToolCall`/`AfterToolCallFailure`
    /// observers around every tool execution. `None` = no extension hooks.
    pub(super) hook_executor: Option<Arc<HookExecutor>>,
    /// Session identifier surfaced into `HookContext` for extension hooks
    /// (env var `SESSION_ID`, log correlation). Empty string when unset.
    pub(super) hook_session_id: String,
    /// Transport used to obtain user confirmation. `None` = no approval
    /// channel wired, so confirm-required tools fail closed (denied).
    pub(super) approval_requester: Option<Arc<dyn ApprovalRequester>>,
    /// Operator-targeted approval requester for config-tier tools (Phase 2b
    /// sudo). Distinct from `approval_requester` (which routes to the
    /// requester's OWN channel for `requires_confirmation`); this routes to the
    /// server operator. `None` → config gate hard-rejects (fail closed).
    pub(super) config_approval_requester: Option<Arc<dyn ApprovalRequester>>,
    /// Routing context of the agent turn this service serves. When set,
    /// `execute()` scopes it into the `TURN_CONTEXT` task-local so HITL tools
    /// (sandbox escalation, `requires_confirmation`, `ask_user`) can route a
    /// prompt back to the originating channel.
    pub(super) turn_context: Option<crate::tools::turn_context::TurnContext>,
    /// Layer 2 of the tool-result budget: persists oversized outputs to
    /// disk and replaces them with a compact marker before they enter
    /// the conversation history.
    pub(super) result_store: Option<Arc<crate::tools::result_store::ToolResultStore>>,
    pub(super) schema_cache: ArcSwap<Option<(u64, Arc<[crate::tool_metadata::ToolDefinition]>)>>,
    pub(super) cache_generation: std::sync::atomic::AtomicU64,
    /// Optional runtime health cache. When set, `list()` and
    /// `metadata_schema()` strip tools whose probe reports Unhealthy
    /// (and the entry hasn't expired) so the LLM never sees a tool whose
    /// dependencies are dead.
    pub(super) health: Option<Arc<ToolHealthCache>>,
    /// Last observed health-cache generation. Bumping this on a generation
    /// drift invalidates the `schema_cache`, so a tool flipping
    /// healthy↔unhealthy retransmits to the LLM on the next turn.
    pub(super) last_health_generation: std::sync::atomic::AtomicU64,
    /// Request-time tool definition rewriters. Applied per tool in
    /// attachment order from `list()` and (on cache miss) from
    /// `metadata_schema()`. See [`ToolDefinitionRewriter`].
    pub(super) definition_rewriters: Vec<Arc<dyn ToolDefinitionRewriter>>,
    /// Tools dropped from `list()` / `metadata_schema()` (the "deferred"
    /// exposure tier) but kept executable + describable. Empty = no-op.
    /// Populated at the request seam from the MCP tool set when
    /// `[tools] defer_mcp_tools` is on.
    ///
    /// Shared (not owned) with `ToolSearchTool`, which SHRINKS it: a tool the
    /// model discovers via `tool_search` is promoted back into the native tool
    /// array so it can actually be called. It used to be an immutable set, which
    /// made every deferred tool permanently uncallable — see [`DeferredTools`].
    pub(super) deferred: Arc<DeferredTools>,
    /// Last `deferred.generation()` folded into `cache_generation`. Mirrors
    /// `last_health_generation`: a discovery must invalidate the cached schema,
    /// or the promoted tool stays absent from the array anyway.
    pub(super) last_deferred_generation: std::sync::atomic::AtomicU64,
    /// Merged tool permission policy (global → agent → channel, most
    /// restrictive wins; see `ToolPermissionsConfig::merge`). `Deny` tools are
    /// hidden from `list()` / `describe()` and rejected at `execute()`;
    /// `Ask` tools route through the confirmation gate exactly like
    /// `confirm_tools`. `None` = no policy configured (allow-all default,
    /// byte-identical to pre-wiring behavior).
    pub(super) tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
    /// Effective execution tier for this turn (global → session → channel
    /// clamp, resolved by the run loop). Consulted by `permission_for` only
    /// for tools no explicit override names, and it reads the tool's DECLARED
    /// metadata, not its name. `None` = no tier wired (unit tests, pre-boot);
    /// the production path always wires one via `build_request_tool_service`.
    pub(super) exec_tier: Option<crate::config::types::policies::ExecTier>,
    /// True when this service serves an UNATTENDED run (an autonomous goal
    /// continuation — no human on the channel). Confirm-gated tools fail closed
    /// (auto-denied) instead of awaiting an approval that can never arrive.
    /// Defaults `false`; interactive turns are unaffected.
    pub(super) unattended: bool,
}

// =============================================================================
// ToolService trait impl
// =============================================================================

impl ScopedToolService {
    /// Run any probe whose cached verdict is missing or expired, then snapshot.
    ///
    /// Nothing else in the process drives `ToolHealthCache::refresh`, so without
    /// this the cache stays empty forever, `is_healthy` answers `true` for every
    /// missing entry, and the three retain gates below — plus the
    /// `<tool_runtime_state>` block built from `unhealthy_iter()` — are
    /// unreachable. The probes were registered at boot and never ran: on a
    /// machine with no Chromium the model still received all ~24 `browser_*`
    /// schemas every turn and burned a call finding out.
    ///
    /// Cost is bounded by construction: `needs_refresh` is false for a name with
    /// no probe and TTL-gated otherwise (the browser probe's TTL is well over
    /// 30 s), `refresh` is single-flight through a `OnceCell`, and each probe is
    /// hard-capped at `PROBE_DEADLINE` (200 ms). Running the stale ones
    /// concurrently makes the worst case one 200 ms wait per TTL window, not one
    /// per probe.
    ///
    /// `metadata_schema()` is sync and cannot await this; it reads whatever the
    /// cache holds and re-bumps its own generation when `health.generation()`
    /// moves, so a refresh performed here reaches it on the next read.
    async fn refreshed_health_snapshot(&self) -> Option<crate::tool_metadata::HealthSnapshot> {
        let health = self.health.as_ref()?;
        let stale: Vec<String> = health
            .probe_names()
            .into_iter()
            .filter(|name| health.needs_refresh(name))
            .collect();
        if !stale.is_empty() {
            futures::future::join_all(stale.iter().map(|name| health.refresh(name))).await;
        }
        Some(health.snapshot())
    }
}

#[async_trait]
impl ToolService for ScopedToolService {
    async fn list(&self) -> Vec<ToolDefinition> {
        // Take a single health snapshot so the filter is consistent across
        // every tool in this list call.
        let health_snap = self.refreshed_health_snapshot().await;

        let mut defs: Vec<ToolDefinition> = self
            .inner
            .tool_definitions()
            .into_iter()
            .map(|d| {
                let metadata = Self::builtin_metadata(
                    &d.name,
                    d.concurrent_safe,
                    d.requires_confirmation,
                    d.max_duration_ms,
                );
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

        // Permission-policy gate: a `Deny` tool is invisible to the LLM —
        // listing it would only invite calls that `execute()` rejects.
        defs.retain(|d| !self.is_permission_denied(&d.name));

        // Health gate: strip any tool whose probe reports a non-expired
        // Unhealthy. Tools without a registered probe pass through (the
        // snapshot reports them healthy by default).
        if let Some(snap) = &health_snap {
            defs.retain(|d| snap.is_healthy(&d.name));
        }

        // Deferred-tier drop: remove tools deferred out of the model's initial
        // list. They stay executable (execute resolves against self.inner) and
        // describable, and are discoverable via `tool_search`.
        if !self.deferred.is_empty() {
            defs.retain(|d| !self.is_deferred(&d.name));
        }

        // Request-time rewriter pass: extensions / host code may rewrite
        // descriptions or schemas without touching the underlying
        // registry entry. Runs after gating so removed tools are not
        // visited.
        self.apply_definition_rewriters(&mut defs);

        defs
    }

    /// `list()` plus the deferred tier: everything `execute()` would actually
    /// dispatch. Deferred tools are hidden from the model's tool array but are
    /// still executable, so tool-name repair must consider them — otherwise a
    /// correct call to a deferred tool misses the Exact tier and the Fuzzy tier
    /// rewrites it into whichever resident tool happens to look similar.
    ///
    /// The allow / deny / health gates still apply: those tools are not
    /// dispatchable, and offering them to the repairer would let it "fix" a call
    /// into something `execute()` will reject.
    async fn dispatchable_list(&self) -> Vec<ToolDefinition> {
        let mut defs = self.list().await;
        if self.deferred.is_empty() {
            return defs;
        }
        let visible: std::collections::BTreeSet<String> =
            defs.iter().map(|d| d.name.clone()).collect();
        let health_snap = self.health.as_ref().map(|h| h.snapshot());
        for name in self.deferred.snapshot().iter() {
            if visible.contains(name) {
                continue;
            }
            if !self.is_allowed(name) || self.is_permission_denied(name) {
                continue;
            }
            if health_snap.as_ref().is_some_and(|s| !s.is_healthy(name)) {
                continue;
            }
            if let Some(def) = self.describe(name).await {
                defs.push(def);
            }
        }
        defs
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        // Enforce allowed filter first.
        if !self.is_allowed(name) {
            return None;
        }
        // Permission-policy gate mirrors list(): Deny tools don't exist
        // from the consumer's point of view.
        if self.is_permission_denied(name) {
            return None;
        }

        // Check subagent tool.
        let mut def = if let Some(ref st) = self.subagent_tool {
            if st.name() == name {
                Some(Self::subagent_definition(st.as_ref()))
            } else {
                self.inner.get(name).map(Self::loop_tool_to_definition)
            }
        } else {
            self.inner.get(name).map(Self::loop_tool_to_definition)
        };

        // Honor request-time rewriters so a `describe()` consumer (per-tool
        // budget probe, single-tool catalogue refresh) sees the same
        // definition that `list()` / `metadata_schema()` would publish.
        if let Some(ref mut d) = def {
            self.apply_definition_rewriters(std::slice::from_mut(d));
        }
        def
    }

    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        // Backward-compat shim: callers that don't have a CancellationToken
        // get a never-fired one. The real per-call cancel is plumbed via
        // `execute_with_cancel` from the harness Act phase.
        self.execute_with_cancel(name, input, CancellationToken::new())
            .await
    }

    async fn execute_with_cancel(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        use tracing::Instrument;

        // Per-call span mirrors opencode's `Effect.withSpan("Tool.execute", …)`.
        // Attributes mirror its `tool.name` / `session.id` / `tool.call_id`
        // contract; `tool.idempotent` is Aleph's extra (drives the one-shot
        // retry gate). Span lives across the whole dispatch — confirm gate,
        // pre-/post-hooks, retry, layer-2 budget — so downstream tracing
        // consumers see a single span tree per LLM tool call.
        let idempotent = crate::tools::retry::is_idempotent_builtin_name(name);
        let span = tracing::info_span!(
            "tool.execute",
            "tool.name" = %name,
            "tool.idempotent" = idempotent,
            "session.id" = %self.hook_session_id,
        );

        // Scope the turn's routing context so HITL tools (sandbox escalation,
        // `requires_confirmation`, `ask_user`) can reach the originating
        // channel, and the turn's `session_key` into `SESSION_ID` so exec-class
        // tools (`code_exec` / `bash` / `code_check`) discover the active
        // session via `sandbox::context::current_session()` and target the
        // right per-session workspace. The gateway path never funnels through
        // `invoke_with_session_trace`, so without this it would refuse every
        // exec call with "no active session context" and the model would loop
        // on the deterministic failure. Both are scoped here — the immediate
        // caller of every tool's `execute` — so they stay visible without
        // crossing a `tokio::spawn`.
        let fut = async move {
            match self.turn_context.clone() {
                Some(turn) => {
                    let session = turn.session_key.clone();
                    crate::sandbox::context::SESSION_ID
                        .scope(
                            session,
                            crate::tools::turn_context::TURN_CONTEXT
                                .scope(turn, self.execute_inner(name, input, cancel)),
                        )
                        .await
                }
                None => self.execute_inner(name, input, cancel).await,
            }
        };
        fut.instrument(span).await
    }

    async fn call_concurrency_claim(
        &self,
        name: &str,
        input: &Value,
    ) -> crate::tools::concurrency::ConcurrencyClaim {
        use crate::tools::concurrency::ConcurrencyClaim;
        // Canonicalize the emitted name first, mirroring `execute_inner`:
        // the gates below and the inner claim lookup must judge the SAME
        // spelling the registry will actually execute. Without this the two
        // sides diverge on alias forms (`file.ops` vs `file_ops`) — the
        // allow-filter misses on the literal name and the inner claim lookup
        // falls to the conservative `Global` instead of the tool's real
        // bounded scope.
        let canonical = self.inner.resolve(name).map(|t| t.name().to_string());
        let name: &str = canonical.as_deref().unwrap_or(name);
        // Subagent dispatch and disallowed tools are whole-world exclusive so
        // they can never join a parallel batch. Everything else — INCLUDING
        // approval-gated calls — surfaces the inner tool's bounded scope.
        if let Some(ref st) = self.subagent_tool {
            if st.name() == name {
                return ConcurrencyClaim::global();
            }
        }
        if !self.is_allowed(name) {
            return ConcurrencyClaim::global();
        }
        // Approval gates (confirm / permission-Ask / tier-argument / operator /
        // hook-Ask) no longer force `Global`. They used to, because the
        // approval path recovered the gated call's id by scanning for the
        // newest `ToolCallRequested` for this tool NAME — unambiguous only
        // when a gated call could never share a batch with a same-name
        // sibling. Correlation is now the ambient
        // [`crate::approval::CallIdentity`] the harness scopes around each
        // execute future: exact per call, immune to guardrail rewrites and
        // same-name siblings. The pending-approval store is a keyed map with
        // one oneshot per entry (`ExecApprovalManager`), so multiple cards
        // pend concurrently, each stamped with its own call id — disjoint
        // gated mutations (two Auto-tier `file_ops` deletes on different
        // paths, `node_invoke` on different nodes) now parallelize, and their
        // approval cards stack instead of queueing behind 120 s serial waits.
        // The underlying RESOURCE claim still governs: a gated `bash` remains
        // `Global` because bash's own claim is `Global`, not because it is
        // gated.
        self.inner
            .call_concurrency_claim(name, input)
            .unwrap_or_else(ConcurrencyClaim::global)
    }

    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]> {
        use std::sync::atomic::Ordering;

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

        // Same treatment for the deferred tier: `tool_search` promoting a tool
        // out of it must show up in the very next array the provider sees.
        {
            let live_gen = self.deferred.generation();
            let prev = self
                .last_deferred_generation
                .swap(live_gen, Ordering::AcqRel);
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
                let metadata = Self::builtin_metadata(
                    &d.name,
                    d.concurrent_safe,
                    d.requires_confirmation,
                    d.max_duration_ms,
                );
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
        // Mirror list() — Deny tools never reach the LLM-visible schema.
        defs.retain(|d| !self.is_permission_denied(&d.name));
        if let Some(snap) = &health_snap {
            defs.retain(|d| snap.is_healthy(&d.name));
        }
        if !self.deferred.is_empty() {
            defs.retain(|d| !self.is_deferred(&d.name));
        }
        // Mirror `list()`: rewriters run after gating, before the
        // metadata-form conversion. Cached output reflects the rewrite,
        // so subsequent O(1) hits don't re-pay the cost.
        self.apply_definition_rewriters(&mut defs);
        let schema = to_metadata_form(&defs);
        self.schema_cache
            .store(Arc::new(Some((gen_now, Arc::clone(&schema)))));
        schema
    }
}
