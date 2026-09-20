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

/// Role-grouped sub-config — default provider + `provider_hint` override map.
pub(super) struct ProviderRouting {
    pub(super) provider: Arc<dyn AiProvider>,
    pub(super) provider_overrides: HashMap<String, Arc<dyn AiProvider>>,
}

/// Role-grouped sub-config — registry, plugin-registry handle, teammate /
/// messaging plumbing, parent agent identity.
pub(super) struct AgentResolution {
    pub(super) agent_registry: Arc<AgentRegistry>,
    pub(super) teammate_manager: Option<Arc<TeammateManager>>,
    pub(super) message_router: Option<Arc<MessageRouter>>,
    pub(super) inbox: Option<Arc<Inbox>>,
    pub(super) parent_agent_id: String,
    pub(super) plugin_registry:
        Option<Arc<tokio::sync::RwLock<crate::extension::registry::PluginRegistry>>>,
}

/// Role-grouped sub-config — background tracker, shared concurrency cap,
/// parent run's cancellation token.
pub(super) struct BackgroundConfig {
    pub(super) background_tracker: Arc<BackgroundAgentTracker>,
    pub(super) subagent_semaphore: Arc<tokio::sync::Semaphore>,
    pub(super) parent_cancel: Option<CancellationToken>,
}

/// Role-grouped sub-config — Delegation hook emit (RawMemory + capture
/// filter) and parent session id stamped onto emitted rows.
pub(super) struct MemoryCapture {
    pub(super) raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
    pub(super) capture_registry: Option<Arc<crate::memory::extensions::MemoryExtensionRegistry>>,
    pub(super) parent_session_id: Option<String>,
}

/// Role-grouped sub-config — session actor, parent tool service, chain depth.
/// Held together because the spawner reads them as a unit when constructing
/// the child `AgentRuntime`.
pub(super) struct ToolingContext {
    pub(super) session: Arc<dyn SessionService>,
    pub(super) parent_tools: Arc<dyn ToolService>,
    pub(super) chain: crate::harness::chain_context::ChainContext,
}

/// Role-grouped sub-config — guardrails, resilience knobs (stall /
/// consecutive-failure / per-turn timeout), and run-style inheritance
/// (strategy body, session mode).
pub(super) struct PolicyInheritance {
    pub(super) guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    pub(super) strategy: Option<String>,
    pub(super) session_mode: Option<crate::config::types::policies::SessionMode>,
    pub(super) stall_config: Option<crate::harness::StallConfig>,
    pub(super) consecutive_failure_cap: Option<usize>,
    pub(super) turn_timeout: Option<std::time::Duration>,
}

/// Role-grouped sub-config — iteration cap, parallel-tool cap,
/// `[context_budget]` config + per-run refiner + cheap-tier summarizer,
/// and verifier chain.
pub(super) struct BudgetInheritance {
    pub(super) default_max_iterations: Option<usize>,
    pub(super) parallel_tool_concurrency: Option<usize>,
    pub(super) context_budget_config: Option<crate::context::budget::ContextBudgetConfig>,
    pub(super) context_budget_refiner:
        Option<crate::orchestrator::deps_builder::ContextBudgetRefiner>,
    pub(super) primary_context_window: Option<u32>,
    pub(super) cheap_summary_provider: Option<Arc<dyn AiProvider>>,
    pub(super) verifier_chain: Option<Arc<crate::verification::VerifierChain>>,
}

/// Role-grouped sub-config — routing-experience store threaded into every
/// child `AgentRuntime`.
pub(super) struct RoutingExperience {
    pub(super) routing_store: Option<Arc<crate::routing::RoutingExperienceStore>>,
}

/// Role-grouped sub-config — parent trace sink threaded into background
/// subagents via `ForwardingTraceSink` for progress observation.
pub(super) struct TraceContext {
    pub(super) trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
}

/// A `LoopTool` that delegates tasks to a temporary `AgentLoop`.
///
/// Decomposed into role-grouped sub-configs so each concern lives in exactly
/// one place. The full field set (33 fields) was previously a flat struct;
/// now grouped by responsibility:
///   * `providers`             — default provider + `provider_hint` overrides
///   * `agent_resolution`      — registry, plugin-registry, teammate/messaging, parent id
///   * `background`            — tracker, shared concurrency cap, parent cancel
///   * `memory`                — Delegation hook emit + parent session
///   * `tools`                 — session actor, parent tool service, chain depth
///   * `policy_inheritance`    — guardrails + resilience + run-style
///   * `budget_inheritance`    — iteration/parallel/context-budget/verifier
///   * `routing`               — routing-experience store
///   * `trace`                 — trace sink
pub struct SubagentTool {
    pub(super) providers: ProviderRouting,
    pub(super) agent_resolution: AgentResolution,
    pub(super) background: BackgroundConfig,
    pub(super) memory: MemoryCapture,
    pub(super) tools: ToolingContext,
    pub(super) policy_inheritance: PolicyInheritance,
    pub(super) budget_inheritance: BudgetInheritance,
    pub(super) routing: RoutingExperience,
    pub(super) trace: TraceContext,
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
            providers: ProviderRouting {
                provider,
                provider_overrides: HashMap::new(),
            },
            agent_resolution: AgentResolution {
                agent_registry,
                teammate_manager: None,
                message_router: None,
                inbox: None,
                parent_agent_id: "primary".to_string(),
                plugin_registry: None,
            },
            background: BackgroundConfig {
                background_tracker,
                // W27 — the operator's `[execution] max_concurrent_subagents`,
                // falling back to `DEFAULT_MAX_CONCURRENT_SUBAGENTS` in any process
                // that never installed one (CLI, tests). Private until a session is
                // named: `with_parent_session_id` swaps in that session's shared
                // semaphore, because background children outlive the run.
                subagent_semaphore: types::subagent_semaphore_for(None),
                parent_cancel: None,
            },
            memory: MemoryCapture {
                raw_memory_writer: None,
                capture_registry: None,
                parent_session_id: None,
            },
            tools: ToolingContext {
                session,
                parent_tools,
                chain,
            },
            policy_inheritance: PolicyInheritance {
                guardrails: None,
                strategy: None,
                session_mode: None,
                stall_config: None,
                consecutive_failure_cap: None,
                turn_timeout: None,
            },
            budget_inheritance: BudgetInheritance {
                default_max_iterations: None,
                parallel_tool_concurrency: None,
                context_budget_config: None,
                context_budget_refiner: None,
                primary_context_window: None,
                cheap_summary_provider: None,
                verifier_chain: None,
            },
            routing: RoutingExperience { routing_store: None },
            trace: TraceContext { trace_sink: None },
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
        self.budget_inheritance.context_budget_config = Some(cfg);
        self
    }

    /// Wire the parent runner's per-run budget refiner + window override so a
    /// spawned child's prompt budget is sized to the model IT will run on
    /// (the main loop already refines its own; the spawner used to derive
    /// straight from the chain-minimum config). Pairs with
    /// [`Self::with_context_budget_config`]: that one sizes the child's
    /// history budget, this one re-keys both onto the serving model.
    #[must_use]
    pub fn with_context_budget_refinement(
        mut self,
        refiner: crate::orchestrator::deps_builder::ContextBudgetRefiner,
        primary_context_window: Option<u32>,
    ) -> Self {
        self.budget_inheritance.context_budget_refiner = Some(refiner);
        self.budget_inheritance.primary_context_window = primary_context_window;
        self
    }

    /// Wire the parent runner's cheap-tier summarizer so the child's compactor
    /// routes its side-channel call to the same flash sibling. Pairs with
    /// [`Self::with_context_budget_config`]: that one gives the child a
    /// compactor at all, this one stops it billing the main model to run it.
    #[must_use]
    pub fn with_cheap_summary_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.budget_inheritance.cheap_summary_provider = Some(provider);
        self
    }

    /// Wire the parent runner's verifier chain so the spawned subagent is
    /// caught by the same structural watchdogs (ToolLoopVerifier,
    /// StopHookVerifier, ScratchpadGoalVerifier, MutationEvidenceVerifier)
    /// as the main run. Without this, every spawned child silently ran with
    /// `verifier_chain: None` and was unprotected against death loops
    /// (AGENTS-R4-01). `None` (no chain on the main harness, or mocks /
    /// simple engine) leaves the child on the legacy no-verifier path —
    /// matching pre-2026-09 behaviour but explicitly opted-in.
    #[must_use]
    pub fn with_verifier_chain(mut self, chain: Arc<crate::verification::VerifierChain>) -> Self {
        self.budget_inheritance.verifier_chain = Some(chain);
        self
    }

    /// B15 — wire the parent runner's boot-time iteration cap so a spawned
    /// child with no declared `max_iterations` inherits it.
    #[must_use]
    pub const fn with_default_max_iterations(mut self, max_iterations: usize) -> Self {
        self.budget_inheritance.default_max_iterations = Some(max_iterations);
        self
    }

    /// Wire the parent runner's `[tool_service] parallel_tool_concurrency`
    /// so a spawned child's Act-phase cap matches the operator's configured
    /// value instead of the hardcoded default.
    #[must_use]
    pub const fn with_parallel_tool_concurrency(mut self, cap: usize) -> Self {
        self.budget_inheritance.parallel_tool_concurrency = Some(cap);
        self
    }

    /// Stage 5a (#9) — wire the guardrail registry inherited by subagents.
    pub fn with_guardrails(mut self, registry: Arc<crate::guardrails::GuardrailRegistry>) -> Self {
        self.policy_inheritance.guardrails = Some(registry);
        self
    }

    /// Phase 3 — wire the per-`provider_hint` override registry. A subagent
    /// whose `AgentDef.provider_hint` matches a key runs on that provider.
    #[must_use]
    pub fn with_provider_overrides(
        mut self,
        overrides: HashMap<String, Arc<dyn AiProvider>>,
    ) -> Self {
        self.providers.provider_overrides = overrides;
        self
    }

    /// B2 — wire the shared plugin-registry handle for per-agent MCP scope.
    #[must_use]
    pub fn with_plugin_registry(
        mut self,
        registry: Arc<tokio::sync::RwLock<crate::extension::registry::PluginRegistry>>,
    ) -> Self {
        self.agent_resolution.plugin_registry = Some(registry);
        self
    }

    /// B3 — wire the stall watchdog config inherited by subagents.
    #[must_use]
    pub const fn with_stall_config(mut self, config: crate::harness::StallConfig) -> Self {
        self.policy_inheritance.stall_config = Some(config);
        self
    }

    /// B3 — wire the consecutive-failure cap inherited by subagents.
    #[must_use]
    pub const fn with_consecutive_failure_cap(mut self, cap: usize) -> Self {
        self.policy_inheritance.consecutive_failure_cap = Some(cap);
        self
    }

    /// B3 — wire the per-turn wall-clock timeout inherited by subagents.
    #[must_use]
    pub const fn with_turn_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.policy_inheritance.turn_timeout = Some(timeout);
        self
    }

    /// Set the teammate manager (`team_name` → team id resolution for the
    /// messaging faces).
    #[must_use]
    pub fn with_teammate_manager(mut self, mgr: Arc<TeammateManager>) -> Self {
        self.agent_resolution.teammate_manager = Some(mgr);
        self
    }

    /// Set the message router for `send_message` actions.
    #[must_use]
    pub fn with_message_router(mut self, router: Arc<MessageRouter>) -> Self {
        self.agent_resolution.message_router = Some(router);
        self
    }

    /// Set the inbox for `read_inbox` actions.
    #[must_use]
    pub fn with_inbox(mut self, inbox: Arc<Inbox>) -> Self {
        self.agent_resolution.inbox = Some(inbox);
        self
    }

    /// Set the parent agent id (identifies the calling agent).
    pub fn with_parent_agent_id(mut self, id: impl Into<String>) -> Self {
        self.agent_resolution.parent_agent_id = id.into();
        self
    }

    /// Spec 1 G2 — wire the raw-memory writer for delegation hook emit.
    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.memory.raw_memory_writer = Some(writer);
        self
    }

    /// Spec 1 G2 — wire an optional capture-filter registry alongside the writer.
    pub fn with_capture_registry(
        mut self,
        registry: Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        self.memory.capture_registry = Some(registry);
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
        self.background.subagent_semaphore = types::subagent_semaphore_for(Some(&sid));
        self.memory.parent_session_id = Some(sid);
        self
    }

    /// Stage F (P2) — thread the parent trace sink so background subagents can
    /// be observed via `ForwardingTraceSink`. Only wired on the background path.
    pub fn with_trace_sink(mut self, sink: Arc<dyn crate::harness::TraceSink>) -> Self {
        self.trace.trace_sink = Some(sink);
        self
    }

    /// VESR v1.1 (b) — thread the routing-experience store so spawned subagents
    /// capture their own routing experience.
    #[must_use]
    pub fn with_routing_store(
        mut self,
        store: Arc<crate::routing::RoutingExperienceStore>,
    ) -> Self {
        self.routing.routing_store = Some(store);
        self
    }

    /// A3 — wire the parent run's cancellation token so spawned subagents
    /// stop when the parent is cancelled.
    #[must_use]
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.background.parent_cancel = Some(token);
        self
    }

    /// Wire the parent run's welded strategy `<strategy>` body so every
    /// spawned subagent's inline prompt carries it. `None` keeps subagents
    /// strategy-free.
    #[must_use]
    pub fn with_strategy(mut self, strategy: String) -> Self {
        self.policy_inheritance.strategy = Some(strategy);
        self
    }

    /// Wire the parent run's usage mode so every spawned child's prompt
    /// names the partition its inherited tool surface was built with.
    #[must_use]
    pub const fn with_session_mode(
        mut self,
        mode: crate::config::types::policies::SessionMode,
    ) -> Self {
        self.policy_inheritance.session_mode = Some(mode);
        self
    }
}
