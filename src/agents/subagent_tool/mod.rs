//! `SubagentTool` — delegates tasks to a temporary child harness.
//!
//! When the parent agent needs to run a complex sub-task autonomously,
//! it calls the `subagent` tool. `AgentRuntime::execute_via_harness` spawns a
//! fresh `AgentHarness` (via `subagent_spawner`) with its parent tool service
//! wrapped by `AllowlistToolService`. SubAgent-mode agents are denied
//! invocation of this tool via `AgentDef::is_tool_allowed` (recursion
//! guard); see `agents/types.rs` for the rule.
//!
//! Supports agent role selection via `agent_type`, optional context
//! injection via `context_summary`, and background execution via
//! `run_in_background`.
//!
//! ## Layout
//! - [`types`] — `SubagentAction` / `RunArgs` / `BatchTask`
//! - [`parse`] — JSON → `SubagentAction`
//! - [`spawn`] — `cancel_for_child*` + `spawn_background` + `build_runtime`
//! - [`loop_tool`] — `impl LoopTool for SubagentTool` (execute pipeline)
//! - this module — `SubagentTool` struct + `new` + every `with_*` builder

mod loop_tool;
mod parse;
mod recovery;
mod spawn;
mod types;

pub use types::{
    clamp_max_concurrent_subagents, max_concurrent_subagents, set_max_concurrent_subagents,
    DEFAULT_MAX_CONCURRENT_SUBAGENTS, MAX_CONCURRENT_SUBAGENTS_CEILING, MIN_CONCURRENT_SUBAGENTS,
};

/// The wire name of this tool — the string the model actually types.
///
/// A single source because the name is also *spoken about* in prose that ships
/// on every Full-mode prompt: `AgentCatalogLayer` tells the model which tool to
/// delegate with. That sentence said `` `delegate` `` for as long as the layer
/// has existed and no such tool has ever been registered (`delegate` is a
/// `groups.rs` *category* id holding `session_send` / `gateway_route` / …), so
/// a model that followed the instruction spent a turn on tool-not-found. A
/// prose reference to a tool is a second copy of its name, and the copy that
/// went stale was the one being sent to the model — CLAUDE.md §0's "同一事实的
/// 两份表述，只改一份就是静默说谎", with the lying half in the prompt.
///
/// `agent_catalog::tests::every_tool_the_catalog_names_is_a_real_tool` resolves
/// every backticked name in that sentence against the real tool universe, so
/// this cannot rot again by rename *or* by invention.
pub const SUBAGENT_TOOL_NAME: &str = "subagent";

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use tokio_util::sync::CancellationToken;

use crate::agents::background_tracker::BackgroundAgentTracker;
use crate::agents::teammates::TeammateManager;
use crate::agents::AgentRegistry;
use crate::providers::AiProvider;
use crate::session::service::SessionService;
use crate::sync_primitives::Arc;
use crate::teams::messages::inbox::Inbox;
use crate::teams::messages::router::MessageRouter;
use crate::tools::service::ToolService;

// =============================================================================
// SubagentTool struct
// =============================================================================

/// A `LoopTool` that delegates tasks to a temporary `AgentLoop`.
pub struct SubagentTool {
    pub(super) provider: Arc<dyn AiProvider>,
    pub(super) chain: crate::harness::chain_context::ChainContext,
    pub(super) agent_registry: Arc<AgentRegistry>,
    pub(super) background_tracker: Arc<BackgroundAgentTracker>,
    /// Shared session actor threaded to child `AgentRuntime` instances.
    pub(super) session: Arc<dyn SessionService>,
    /// Parent tool service; the harness decorates it with an allowlist.
    pub(super) parent_tools: Arc<dyn ToolService>,
    /// Optional teammate manager — resolves `team_name` → team id (creating
    /// on first use) for the `send_message` / `read_inbox` faces.
    pub(super) teammate_manager: Option<Arc<TeammateManager>>,
    /// Optional message router for `send_message` actions.
    pub(super) message_router: Option<Arc<MessageRouter>>,
    /// Optional inbox for `read_inbox` actions.
    pub(super) inbox: Option<Arc<Inbox>>,
    /// Identifies the calling agent (default: "primary").
    pub(super) parent_agent_id: String,
    /// Spec 1 G2 — threaded into child `AgentRuntime`s so the spawner emits
    /// `RawMemory(Delegation)` after each successful local subagent run.
    pub(super) raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
    /// Optional capture-filter registry threaded with the writer.
    pub(super) capture_registry: Option<Arc<crate::memory::extensions::MemoryExtensionRegistry>>,
    /// Parent session id stamped onto emitted Delegation rows. `None` leaves
    /// the row untagged for session-level lookups.
    pub(super) parent_session_id: Option<String>,
    /// Stage F (P2) — parent trace sink threaded into background subagent
    /// runtimes wrapped by `ForwardingTraceSink` for progress observation.
    /// Sync subagents do NOT receive this wrapper (Stage A inheritance suffices).
    pub(super) trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
    /// A2 — shared concurrency cap; one per tool instance (= per agent run).
    pub(super) subagent_semaphore: Arc<tokio::sync::Semaphore>,
    /// A3 — parent run's cancellation token. Each spawn path derives a
    /// `child_token()` so a cancelled parent stops its subagents.
    pub(super) parent_cancel: Option<CancellationToken>,
    /// B2 — shared plugin-registry handle, threaded into each `AgentRuntime`.
    /// `Arc<RwLock<..>>` (not an owned snapshot) so `McpScope::provision` reads
    /// the live registry under its own guard at spawn time.
    pub(super) plugin_registry:
        Option<Arc<tokio::sync::RwLock<crate::extension::registry::PluginRegistry>>>,
    /// B3 — stall watchdog config inherited by subagents.
    pub(super) stall_config: Option<crate::harness::StallConfig>,
    /// B3 — consecutive-failure cap inherited by subagents.
    pub(super) consecutive_failure_cap: Option<usize>,
    /// B3 — per-turn wall-clock timeout inherited by subagents.
    pub(super) turn_timeout: Option<std::time::Duration>,
    /// Phase 3 — `provider_hint` → pinned-then-fall-through provider. An empty
    /// map (the `new()` default) means every subagent uses `provider`.
    pub(super) provider_overrides: HashMap<String, Arc<dyn AiProvider>>,
    /// Stage 5a (#9) — the main harness's guardrail registry, threaded into
    /// every child `AgentRuntime` so subagents enforce the same
    /// Input/Output/ToolCall checks as the spawning harness. `None` (the
    /// `new()` default) keeps subagents unguarded, matching a main harness
    /// that has no registry configured.
    pub(super) guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    /// Welded strategy `<strategy>` body for the parent run. Threaded into
    /// every child `AgentRuntime` via `build_runtime` so spawned subagents
    /// share the run-global strategy. `None` (the `new()` default) keeps
    /// subagents strategy-free, byte-identical to the pre-strategy build.
    pub(super) strategy: Option<String>,
    /// Parent run's usage mode (chat / work / code), threaded into every
    /// spawn so the child prompt names its inherited partition. `None` (the
    /// `new()` default, and the Work identity partition the wiring site
    /// skips) keeps child prompts byte-identical.
    pub(super) session_mode: Option<crate::config::types::policies::SessionMode>,
    /// VESR v1.1 (b) — routing store threaded into every child `AgentRuntime`
    /// so spawned subagents capture their own routing experience. `None` (the
    /// `new()` default) keeps subagents capture-free.
    pub(super) routing_store: Option<Arc<crate::routing::RoutingExperienceStore>>,
    /// B15 — the parent runner's boot-time `[execution] max_iterations`,
    /// inherited by children that declare no cap of their own. `None` (the
    /// `new()` default) still yields a capped child — the spawner falls back to
    /// `FALLBACK_MAX_ITERATIONS` — it just isn't the operator's configured value.
    pub(super) default_max_iterations: Option<usize>,
    /// The parent runner's `[tool_service] parallel_tool_concurrency`,
    /// inherited so a child's Act-phase cap (including 0/1 = disabled)
    /// matches the operator's configured value. `None` (the `new()` default)
    /// leaves the spawner on the config default.
    pub(super) parallel_tool_concurrency: Option<usize>,
    /// The parent runner's `[context_budget]` config, inherited so a spawned
    /// child builds its own budget / compactor / preflight pipeline. `None`
    /// (the `new()` default, or `[context_budget]` disabled) leaves the child
    /// context-unmanaged, matching the main harness under the same config.
    pub(super) context_budget_config: Option<crate::context::budget::ContextBudgetConfig>,
    /// The parent runner's cheap-tier summarization provider, inherited so the
    /// child's compactor bills its side-channel to the same flash sibling
    /// rather than the main reasoning model. `None` (the `new()` default, or no
    /// cheap tier resolved) summarizes on the main LLM, as before.
    pub(super) cheap_summary_provider: Option<Arc<dyn AiProvider>>,
}

impl SubagentTool {
    /// Create a new `SubagentTool`.
    ///
    /// - `provider`: the AI provider for the sub-agent's LLM calls
    /// - `chain`: the parent's chain context for depth tracking
    /// - `agent_registry`: registry of available agent definitions
    /// - `background_tracker`: tracker for background sub-agent tasks
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn AiProvider>,
        chain: crate::harness::chain_context::ChainContext,
        agent_registry: Arc<AgentRegistry>,
        background_tracker: Arc<BackgroundAgentTracker>,
        session: Arc<dyn SessionService>,
        parent_tools: Arc<dyn ToolService>,
    ) -> Self {
        Self {
            provider,
            chain,
            agent_registry,
            background_tracker,
            session,
            parent_tools,
            teammate_manager: None,
            message_router: None,
            inbox: None,
            parent_agent_id: "primary".to_string(),
            raw_memory_writer: None,
            capture_registry: None,
            parent_session_id: None,
            trace_sink: None,
            // W27 — the operator's `[execution] max_concurrent_subagents`,
            // falling back to `DEFAULT_MAX_CONCURRENT_SUBAGENTS` in any process
            // that never installed one (CLI, tests). Private until a session is
            // named: `with_parent_session_id` swaps in that session's shared
            // semaphore, because background children outlive the run.
            subagent_semaphore: types::subagent_semaphore_for(None),
            parent_cancel: None,
            plugin_registry: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            provider_overrides: HashMap::new(),
            guardrails: None,
            strategy: None,
            session_mode: None,
            routing_store: None,
            default_max_iterations: None,
            parallel_tool_concurrency: None,
            context_budget_config: None,
            cheap_summary_provider: None,
        }
    }

    /// Wire the parent runner's `[context_budget]` config so a spawned child
    /// builds its own budget / compactor / preflight pipeline instead of
    /// running with no context management at all.
    #[must_use]
    pub fn with_context_budget_config(
        mut self,
        cfg: crate::context::budget::ContextBudgetConfig,
    ) -> Self {
        self.context_budget_config = Some(cfg);
        self
    }

    /// Wire the parent runner's cheap-tier summarizer so the child's compactor
    /// routes its side-channel call to the same flash sibling. Pairs with
    /// [`Self::with_context_budget_config`]: that one gives the child a
    /// compactor at all, this one stops it billing the main model to run it.
    #[must_use]
    pub fn with_cheap_summary_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.cheap_summary_provider = Some(provider);
        self
    }

    /// B15 — wire the parent runner's boot-time iteration cap so a spawned
    /// child with no declared `max_iterations` inherits it.
    #[must_use]
    pub const fn with_default_max_iterations(mut self, max_iterations: usize) -> Self {
        self.default_max_iterations = Some(max_iterations);
        self
    }

    /// Wire the parent runner's `[tool_service] parallel_tool_concurrency`
    /// so a spawned child's Act-phase cap matches the operator's configured
    /// value instead of the hardcoded default.
    #[must_use]
    pub const fn with_parallel_tool_concurrency(mut self, cap: usize) -> Self {
        self.parallel_tool_concurrency = Some(cap);
        self
    }

    /// Stage 5a (#9) — wire the guardrail registry inherited by subagents.
    pub fn with_guardrails(mut self, registry: Arc<crate::guardrails::GuardrailRegistry>) -> Self {
        self.guardrails = Some(registry);
        self
    }

    /// Phase 3 — wire the per-`provider_hint` override registry. A subagent
    /// whose `AgentDef.provider_hint` matches a key runs on that provider.
    #[must_use]
    pub fn with_provider_overrides(
        mut self,
        overrides: HashMap<String, Arc<dyn AiProvider>>,
    ) -> Self {
        self.provider_overrides = overrides;
        self
    }

    /// B2 — wire the shared plugin-registry handle for per-agent MCP scope.
    #[must_use]
    pub fn with_plugin_registry(
        mut self,
        registry: Arc<tokio::sync::RwLock<crate::extension::registry::PluginRegistry>>,
    ) -> Self {
        self.plugin_registry = Some(registry);
        self
    }

    /// B3 — wire the stall watchdog config inherited by subagents.
    #[must_use]
    pub const fn with_stall_config(mut self, config: crate::harness::StallConfig) -> Self {
        self.stall_config = Some(config);
        self
    }

    /// B3 — wire the consecutive-failure cap inherited by subagents.
    #[must_use]
    pub const fn with_consecutive_failure_cap(mut self, cap: usize) -> Self {
        self.consecutive_failure_cap = Some(cap);
        self
    }

    /// B3 — wire the per-turn wall-clock timeout inherited by subagents.
    #[must_use]
    pub const fn with_turn_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.turn_timeout = Some(timeout);
        self
    }

    /// Set the teammate manager (`team_name` → team id resolution for the
    /// messaging faces).
    #[must_use]
    pub fn with_teammate_manager(mut self, mgr: Arc<TeammateManager>) -> Self {
        self.teammate_manager = Some(mgr);
        self
    }

    /// Set the message router for `send_message` actions.
    #[must_use]
    pub fn with_message_router(mut self, router: Arc<MessageRouter>) -> Self {
        self.message_router = Some(router);
        self
    }

    /// Set the inbox for `read_inbox` actions.
    #[must_use]
    pub fn with_inbox(mut self, inbox: Arc<Inbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    /// Set the parent agent id (identifies the calling agent).
    pub fn with_parent_agent_id(mut self, id: impl Into<String>) -> Self {
        self.parent_agent_id = id.into();
        self
    }

    /// Spec 1 G2 — wire the raw-memory writer for delegation hook emit.
    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// Spec 1 G2 — wire an optional capture-filter registry alongside the writer.
    pub fn with_capture_registry(
        mut self,
        registry: Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        self.capture_registry = Some(registry);
        self
    }

    /// Spec 1 G2 — set the parent session id stamped onto Delegation rows.
    pub fn with_parent_session_id(mut self, sid: impl Into<String>) -> Self {
        let sid = sid.into();
        // Re-bind the concurrency semaphore to the SESSION now that we know
        // which one this is. `new()` cannot do it — the session is not known
        // there — and the cap has to be shared across a session's runs because
        // background children outlive the run that spawned them. See
        // `types::subagent_semaphore_for`.
        self.subagent_semaphore = types::subagent_semaphore_for(Some(&sid));
        self.parent_session_id = Some(sid);
        self
    }

    /// Stage F (P2) — thread the parent trace sink so background subagents can
    /// be observed via `ForwardingTraceSink`. Only wired on the background path.
    pub fn with_trace_sink(mut self, sink: Arc<dyn crate::harness::TraceSink>) -> Self {
        self.trace_sink = Some(sink);
        self
    }

    /// VESR v1.1 (b) — thread the routing-experience store so spawned subagents
    /// capture their own routing experience.
    #[must_use]
    pub fn with_routing_store(
        mut self,
        store: Arc<crate::routing::RoutingExperienceStore>,
    ) -> Self {
        self.routing_store = Some(store);
        self
    }

    /// A3 — wire the parent run's cancellation token so spawned subagents
    /// stop when the parent is cancelled.
    #[must_use]
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.parent_cancel = Some(token);
        self
    }

    /// Wire the parent run's welded strategy `<strategy>` body so every
    /// spawned subagent's inline prompt carries it. `None` keeps subagents
    /// strategy-free.
    #[must_use]
    pub fn with_strategy(mut self, strategy: String) -> Self {
        self.strategy = Some(strategy);
        self
    }

    /// Wire the parent run's usage mode so every spawned child's prompt
    /// names the partition its inherited tool surface was built with.
    #[must_use]
    pub const fn with_session_mode(
        mut self,
        mode: crate::config::types::policies::SessionMode,
    ) -> Self {
        self.session_mode = Some(mode);
        self
    }
}
