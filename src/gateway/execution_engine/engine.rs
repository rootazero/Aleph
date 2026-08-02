//! `ExecutionEngine` — bridges Gateway requests to the `AgentLoop`.
//!
//! This file contains the struct definition, constructor, builder methods,
//! and the main `execute()` / `get_status()` / `cancel()` public API.
//!
//! Slash command handling lives in `slash_command.rs`.
//! Agent loop execution and streaming live in `run_loop.rs`.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use tokio::sync::RwLock;
use tracing::info;

use super::{ActiveRun, ExecutionEngineConfig, ExecutionError, RunStatus};
use crate::gateway::agent_env::AgentEnvStore;
use crate::resilience::StateDatabase;

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;
use crate::tool_metadata::UnifiedTool;

/// Deferred-injected deps for the post-run autonomous-continuation hook.
/// Named struct replacing the old 2-tuple, and carries the goal-loop's objective-gate handler.
/// When `gate` is `None` (no `config.toml [[stop_hooks]]`), loop behavior is unchanged.
#[derive(Clone)]
pub struct ContinuationDeps {
    pub registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    pub adapter: Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>,
    /// Objective-gate handler (shares the same instance as `StopHookVerifier`), guards
    /// the completion decision of autonomous continuations. `None` → no gate, a `complete`
    /// claim terminates immediately.
    pub gate: Option<Arc<Vec<Arc<dyn crate::verification::stop_hooks::StopHookHandler>>>>,
    /// Gateway event bus for broadcasting autonomous-continuation runs so the
    /// Panel and `aleph watch` see pursuit progress live, and the final reply
    /// fans out to the session's origin channel (G1, R5/R6). `None` in tests /
    /// non-gateway contexts → the continuation falls back to a collect-and-drop
    /// emitter, keeping those paths behavior-identical to before.
    pub event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

/// Execution engine that bridges Gateway to the `AgentLoop`
pub struct ExecutionEngine<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> {
    pub(super) config: ExecutionEngineConfig,
    pub(super) active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
    /// Per-session run mutual-exclusion (replaces the per-agent `AgentState`
    /// gate — Task 6). Exactly one run may be `Running` per session; sessions
    /// of the same agent no longer contend with each other.
    pub(super) session_run_registry: Arc<super::session_run_registry::SessionRunRegistry>,
    /// Run-lifetime concurrency caps (global + per-agent), held for the whole
    /// run by the [`super::gate::RunSlot`] guard `execute()` binds at
    /// admission (Task 6).
    pub(super) concurrency: Arc<super::concurrency::ConcurrencyLimiter>,
    /// Provider registry for LLM access
    pub(super) provider_registry: Arc<P>,
    /// Tool registry for tool execution
    pub(super) tool_registry: Arc<R>,
    /// Available tools for all agents
    pub(super) tools: Arc<Vec<UnifiedTool>>,
    /// Workspace manager for workspace-scoped profile resolution
    pub(super) workspace_manager: Option<Arc<AgentEnvStore>>,
    /// Runtime tool-health cache, shared with the `ToolCatalog` that registers
    /// the probes. The per-request tool service consults it so the model never
    /// receives the schema of a tool whose dependency is dead (no Chromium →
    /// no `browser_*`). `None` in tests / non-gateway contexts, which is
    /// fail-open: every tool stays visible.
    pub(super) tool_health: Option<Arc<crate::tool_metadata::ToolHealthCache>>,
    /// Memory backend for auto-memorization of conversations
    pub(super) memory_backend: Option<crate::memory::store::MemoryBackend>,
    /// Compression service for turn-based fact extraction
    pub(super) compression_service: Option<Arc<crate::memory::compression::CompressionService>>,
    /// Memory context provider for SQLite-backed prompt augmentation
    pub(super) memory_context_provider: Option<Arc<crate::thinker::MemoryContextProvider>>,
    /// Live app config handle. Read once per turn (not snapshotted at boot) so
    /// an `[policies.exec_tier]` / `[policies.tool_permissions]` change takes
    /// effect on the very next tool call without a restart. `None` in tests /
    /// non-gateway contexts → the global permission layer is all-default.
    pub(super) app_config: Option<Arc<RwLock<crate::Config>>>,
    /// Session compactor for hierarchical session summarization
    pub(super) session_compactor: Option<Arc<crate::memory::session_compactor::SessionCompactor>>,
    /// Session manager for auto-topic generation
    pub(super) session_manager: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    /// Event bus for broadcasting session updates
    pub(super) event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    /// Media processor for multimodal attachment handling (images, audio, etc.)
    pub(super) media_processor: Option<Arc<crate::media::processor::MediaProcessor>>,
    /// Optional resilience database for task/trace persistence.
    pub(super) state_database: Option<Arc<StateDatabase>>,
    /// Teammate manager for named sub-agent team creation/registration.
    pub(super) teammate_manager: Option<Arc<crate::agents::teammates::TeammateManager>>,
    /// Message router for sub-agent `send_message` actions.
    pub(super) message_router: Option<Arc<crate::teams::messages::router::MessageRouter>>,
    /// Inbox for sub-agent `read_inbox` actions.
    pub(super) inbox: Option<Arc<crate::teams::messages::inbox::Inbox>>,
    /// Process-lifetime registry of background sub-agent runs. Shared across
    /// every run this engine executes so `check_status` / `list` / `cancel`
    /// keep working on later turns — a background sub-agent's whole point is
    /// to outlive the turn that spawned it. (Previously constructed fresh per
    /// request inside `run_agent_loop_inner`, which silently dropped results
    /// and cancel tokens the moment the spawning turn ended.) Completed
    /// entries are TTL-pruned by the tracker itself, bounding growth.
    pub(super) background_tracker: Arc<crate::agents::background_tracker::BackgroundAgentTracker>,
    /// Orchestrator handle injected after boot assembly. Populated via
    /// `with_orchestrator` once `initialize_orchestrator` completes.
    pub(super) orchestrator: Arc<std::sync::OnceLock<Arc<crate::orchestrator::Orchestrator>>>,
    /// Continuation deps injected after boot assembly so the post-run hook
    /// can enqueue an autonomous continuation in the SAME session when the
    /// session's standing goal has `PursuitMode::Active` and `should_continue`.
    /// Holds (`AgentInstanceRegistry`, self-as-ExecutionAdapter). `None` / empty
    /// until populated via `continuation_cell()` + boot wiring; absent in all
    /// tests and non-production paths, so the hook is a safe no-op by default.
    pub(super) continuation_deps: Arc<std::sync::OnceLock<ContinuationDeps>>,
    /// Deferred channel-registry handle for the R5 progress side-channel.
    /// Shares the same `OnceCell` the boot path populates once channels are
    /// up (see `agent_init` / boot wiring); empty until then, so the progress
    /// sink degrades to a no-op decorator. Only read when
    /// `config.scratchpad_progress_push` is enabled.
    pub(super) channel_registry: crate::tasks::shared::targets::ChannelRegistryCell,
    /// Strategic-planner provider for the naked agent-loop StraTA planner
    /// (round 2). The same provider the goal/loop/workflow tools use, but
    /// gated additionally on `[strategy].plan_naked_loop`: `None` when either
    /// `enabled` or `plan_naked_loop` is off, so the first-message planner
    /// trigger in `execute.rs` stays dormant (fail-soft, P7).
    pub(super) planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    /// Capture-filter registry threaded into spawned subagents. When set, a
    /// delegated child's completion fires `on_delegation` memory-extension hooks
    /// and the child's own memory writes go through the capture filter. Shares
    /// the same `Arc<MemoryExtensionRegistry>` the session compactor uses;
    /// `None` (tests / simple engine / no memory extensions) leaves subagent
    /// delegation capture dormant — `dispatch_on_delegation` no-ops on an empty
    /// registry, so this is a fail-soft default (P7).
    pub(super) capture_registry: Option<Arc<crate::memory::extensions::MemoryExtensionRegistry>>,
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Create a new execution engine
    pub fn new(
        config: ExecutionEngineConfig,
        provider_registry: Arc<P>,
        tool_registry: Arc<R>,
        tools: Vec<UnifiedTool>,
        memory_backend: Option<crate::memory::store::MemoryBackend>,
    ) -> Self {
        let engine = Self {
            session_run_registry: Arc::new(Default::default()),
            concurrency: Arc::new(super::concurrency::ConcurrencyLimiter::new(
                config.max_runs_global,
                config.max_runs_per_agent,
            )),
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            provider_registry,
            tool_registry,
            tools: Arc::new(tools),
            workspace_manager: None,
            tool_health: None,
            memory_backend,
            compression_service: None,
            memory_context_provider: None,
            app_config: None,
            session_compactor: None,
            session_manager: None,
            event_bus: None,
            media_processor: None,
            state_database: None,
            teammate_manager: None,
            message_router: None,
            inbox: None,
            // Share the process-global tracker so the `subagent.tree` RPC reads
            // the same live state this engine's spawn path writes.
            background_tracker: crate::agents::background_tracker::BackgroundAgentTracker::global(),
            orchestrator: Arc::new(std::sync::OnceLock::new()),
            continuation_deps: Arc::new(std::sync::OnceLock::new()),
            channel_registry: Arc::new(tokio::sync::OnceCell::new()),
            planner_provider: None,
            capture_registry: None,
        };
        super::concurrency_handle::install_global(&engine.concurrency);
        engine
    }

    /// Inject the deferred channel-registry cell so the R5 progress sink can
    /// reach user channels. Pass the same cell the boot path populates
    /// (`tool_registry.channel_registry_cell()`); injecting a clone shares the
    /// underlying `OnceCell`, so the sink sees the registry once boot sets it.
    pub fn with_channel_registry_cell(
        mut self,
        cell: crate::tasks::shared::targets::ChannelRegistryCell,
    ) -> Self {
        self.channel_registry = cell;
        self
    }

    /// Return the orchestrator `OnceLock` handle so boot code can inject the
    /// `Arc<Orchestrator>` after `initialize_orchestrator` completes.
    #[must_use]
    pub fn orchestrator_cell(
        &self,
    ) -> Arc<std::sync::OnceLock<Arc<crate::orchestrator::Orchestrator>>> {
        self.orchestrator.clone()
    }

    /// Return the continuation-deps `OnceLock` handle so boot code can inject
    /// `(AgentRegistry, self-as-ExecutionAdapter)` after the engine is wrapped
    /// in `Arc`. Enables the post-run autonomous-continuation hook in
    /// `execute.rs` without storing a self-referential field at construction
    /// time (same deferred-injection pattern as `orchestrator_cell`).
    #[must_use]
    pub fn continuation_cell(&self) -> Arc<std::sync::OnceLock<ContinuationDeps>> {
        self.continuation_deps.clone()
    }

    /// Set a compression service for automatic turn-based compression.
    pub fn with_compression_service(
        mut self,
        service: Arc<crate::memory::compression::CompressionService>,
    ) -> Self {
        self.compression_service = Some(service);
        self
    }

    /// Set a memory context provider for SQLite-backed prompt augmentation.
    #[must_use]
    pub fn with_memory_context_provider(
        mut self,
        provider: Arc<crate::thinker::MemoryContextProvider>,
    ) -> Self {
        self.memory_context_provider = Some(provider);
        self
    }

    /// Thread the capture-filter registry so spawned subagents inherit it — the
    /// child's memory writes go through the capture filter and its completion
    /// fires `on_delegation` memory-extension hooks. Shares the same registry
    /// the session compactor uses.
    #[must_use]
    pub fn with_capture_registry(
        mut self,
        registry: Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        self.capture_registry = Some(registry);
        self
    }

    /// Inject the strategic-planner provider for the naked agent-loop planner.
    /// `None` (the default) keeps the first-message planner trigger dormant.
    #[must_use]
    pub fn with_planner_provider(
        mut self,
        provider: Option<Arc<dyn crate::providers::AiProvider>>,
    ) -> Self {
        self.planner_provider = provider;
        self
    }

    /// Set a session compactor for hierarchical session summarization.
    #[must_use]
    pub fn with_session_compactor(
        mut self,
        compactor: Arc<crate::memory::session_compactor::SessionCompactor>,
    ) -> Self {
        self.session_compactor = Some(compactor);
        self
    }

    /// Share the live app-config handle so each turn resolves the current
    /// execution tier / tool permissions instead of a boot snapshot.
    #[must_use]
    pub fn with_app_config(mut self, config: Arc<RwLock<crate::Config>>) -> Self {
        self.app_config = Some(config);
        self
    }

    /// Set session manager and event bus for auto-topic generation.
    pub fn with_session_topic_support(
        mut self,
        session_manager: Arc<dyn crate::gateway::session_store::SessionStore>,
        event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
    ) -> Self {
        self.session_manager = Some(session_manager);
        self.session_run_registry.set_event_bus(event_bus.clone());
        self.event_bus = Some(event_bus);
        self
    }

    /// Announce a session update on the global event bus so UI sidebars can
    /// refresh their session list without polling.
    ///
    /// Published directly on the bus (mirrors
    /// `SessionManager::emit_session_updated`) rather than on the per-run
    /// emitter: channel runs use `ReplyEmitter`, which only routes
    /// channel-facing events, so an emitter-side announcement never reaches
    /// the Panel for runs originating from Telegram/Slack/etc.
    /// `origin_channel` is the triggering channel (`metadata["channel_id"]`),
    /// `None` for Panel-originated runs.
    pub(super) fn publish_session_updated(
        &self,
        session_key: &crate::gateway::router::SessionKey,
        origin_channel: Option<&str>,
    ) {
        if let Some(bus) = &self.event_bus {
            let frame = crate::gateway::events::GatewayEventFrame::SessionUpdated {
                session_key: session_key.to_key_string(),
                origin_channel: origin_channel
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string()),
            };
            let _ = bus.publish_frame(&frame);
        }
    }

    /// Set the media processor for multimodal attachment handling.
    #[must_use]
    pub fn with_media_processor(
        mut self,
        processor: Arc<crate::media::processor::MediaProcessor>,
    ) -> Self {
        self.media_processor = Some(processor);
        self
    }

    /// Share the runtime tool-health cache the `ToolCatalog` writes probes into.
    #[must_use]
    pub fn with_tool_health(mut self, health: Arc<crate::tool_metadata::ToolHealthCache>) -> Self {
        self.tool_health = Some(health);
        self
    }

    /// Set the resilience state database for task/trace persistence.
    #[must_use]
    pub fn with_state_database(mut self, state_database: Arc<StateDatabase>) -> Self {
        self.state_database = Some(state_database);
        self
    }

    /// Set the workspace manager for workspace-scoped profile resolution.
    ///
    /// When set, the engine resolves the user's active workspace at the start
    /// of each run and injects the workspace profile into the prompt builder
    /// and the `workspace_id` into the request context metadata.
    #[must_use]
    pub fn with_workspace_manager(mut self, manager: Arc<AgentEnvStore>) -> Self {
        self.workspace_manager = Some(manager);
        self
    }

    /// Set the teammate manager for named sub-agent team creation/registration.
    #[must_use]
    pub fn with_teammate_manager(
        mut self,
        mgr: Arc<crate::agents::teammates::TeammateManager>,
    ) -> Self {
        self.teammate_manager = Some(mgr);
        self
    }

    /// Set the message router for sub-agent `send_message` actions.
    #[must_use]
    pub fn with_message_router(
        mut self,
        router: Arc<crate::teams::messages::router::MessageRouter>,
    ) -> Self {
        self.message_router = Some(router);
        self
    }

    /// Set the inbox for sub-agent `read_inbox` actions.
    #[must_use]
    pub fn with_inbox(mut self, inbox: Arc<crate::teams::messages::inbox::Inbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    /// Get the number of currently active (non-completed) runs
    pub async fn active_run_count(&self) -> usize {
        self.active_runs.read().await.len()
    }

    /// Snapshot of the run-lifetime `ConcurrencyLimiter`'s slot usage
    /// (`global_in_use` / `global_total`, per-agent rows, and queue depth),
    /// surfaced via `gateway.metrics.run_concurrency` (Task 8, audit 3.4).
    #[must_use]
    pub fn concurrency_snapshot(&self) -> super::ConcurrencySnapshot {
        self.concurrency.snapshot()
    }

    /// Session keys with a run currently in flight — the authoritative
    /// in-memory admission gate (`SessionRunRegistry`), not the cosmetic
    /// persisted store marker. Surfaced beside `concurrency_snapshot` in
    /// `gateway.metrics.run_concurrency` so a Panel can render per-session
    /// running indicators independent of which interface started the run.
    #[must_use]
    pub fn running_sessions(&self) -> Vec<String> {
        self.session_run_registry.running_keys()
    }

    /// Get the status of a run
    pub async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        let runs = self.active_runs.read().await;
        runs.get(run_id).map(|run| RunStatus {
            run_id: run_id.to_string(),
            state: run.state.clone(),
            started_at: Some(run.started_at),
            completed_at: run.completed_at,
            steps_completed: run.steps_completed,
            current_tool: run.current_tool.clone(),
        })
    }

    /// Cancel a run
    pub async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        let cancel_tx = {
            let runs = self.active_runs.read().await;
            match runs.get(run_id) {
                Some(run) => match run.cancel_tx {
                    Some(ref tx) => tx.clone(),
                    None => return Err(ExecutionError::RunNotActive(run_id.to_string())),
                },
                None => return Err(ExecutionError::RunNotFound(run_id.to_string())),
            }
        };
        // Lock released before await
        let _ = cancel_tx.send(()).await;
        info!("Sent cancellation signal for run {}", run_id);
        Ok(())
    }

    /// Cancel the `Running` run on `session_key`, if any, AND any in-flight
    /// delegated child runs it owns. Returns the leader's own cancelled run
    /// id, or `None` when nothing was running on the session itself (the
    /// child walk still happens either way).
    ///
    /// Channel-facing seam for `/stop` and Panel `chat.abort`: channels know
    /// their session key, not the engine-internal run id. Reuses the same
    /// same-session predicate as the busy-input paths (`find_steering_target_id`,
    /// with no run excluded — the empty exclusion id never matches a real run).
    ///
    /// A leader that delegated work (`task_manage` / bash-code / delegate
    /// workflows) registers its member run's REAL engine `run_id` in the
    /// `BackgroundAgentTracker` under `root_session = leader`. Without this
    /// walk, cancelling the leader left that member DETACHED — still running,
    /// still burning tokens, with no way back to it. Firing the same
    /// cooperative per-run token (`cancel`) for each tracked child closes that
    /// leak; a child id that is not a live engine run (e.g. an in-process
    /// subagent) is simply a harmless `cancel` miss.
    pub async fn cancel_session(
        &self,
        session_key: &crate::routing::session_key::SessionKey,
    ) -> Result<Option<String>, ExecutionError> {
        let target = {
            let runs = self.active_runs.read().await;
            super::steering::find_steering_target_id(&runs, "", session_key)
        };
        // Cancel the leader's own run (if any).
        let own = match target {
            Some(run_id) => {
                self.cancel(&run_id).await?;
                Some(run_id)
            }
            None => None,
        };
        // Also cancel any in-flight delegated child runs owned by this
        // session, regardless of whether the leader had its own run.
        let children = crate::agents::background_tracker::BackgroundAgentTracker::global()
            .running_runs_of_session(&session_key.to_key_string());
        for child in children {
            let _ = self.cancel(&child).await;
        }
        Ok(own)
    }
}
