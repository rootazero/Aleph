//! ExecutionEngine — bridges Gateway requests to the AgentLoop.
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

/// Execution engine that bridges Gateway to the AgentLoop
pub struct ExecutionEngine<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> {
    pub(super) config: ExecutionEngineConfig,
    pub(super) active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
    /// Provider registry for LLM access
    pub(super) provider_registry: Arc<P>,
    /// Tool registry for tool execution
    pub(super) tool_registry: Arc<R>,
    /// Available tools for all agents
    pub(super) tools: Arc<Vec<UnifiedTool>>,
    /// Workspace manager for workspace-scoped profile resolution
    pub(super) workspace_manager: Option<Arc<AgentEnvStore>>,
    /// Memory backend for auto-memorization of conversations
    pub(super) memory_backend: Option<crate::memory::store::MemoryBackend>,
    /// Optional task router for pre-classification and escalation handling
    pub(super) task_router: Option<Arc<dyn crate::routing::TaskRouter>>,
    /// Compression service for turn-based fact extraction
    pub(super) compression_service: Option<Arc<crate::memory::compression::CompressionService>>,
    /// Memory context provider for SQLite-backed prompt augmentation
    pub(super) memory_context_provider: Option<Arc<crate::thinker::MemoryContextProvider>>,
    /// Global tool permission policy
    pub(super) global_tool_permissions: crate::config::types::policies::ToolPermissionsConfig,
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
    /// Message router for sub-agent send_message actions.
    pub(super) message_router: Option<Arc<crate::teams::messages::router::MessageRouter>>,
    /// Inbox for sub-agent read_inbox actions.
    pub(super) inbox: Option<Arc<crate::teams::messages::inbox::Inbox>>,
    /// Orchestrator handle injected after boot assembly. Populated via
    /// `with_orchestrator` once `initialize_orchestrator` completes.
    pub(super) orchestrator: Arc<std::sync::OnceLock<Arc<crate::orchestrator::Orchestrator>>>,
    /// Deferred channel-registry handle for the R5 progress side-channel.
    /// Shares the same `OnceCell` the boot path populates once channels are
    /// up (see `agent_init` / boot wiring); empty until then, so the progress
    /// sink degrades to a no-op decorator. Only read when
    /// `config.scratchpad_progress_push` is enabled.
    pub(super) channel_registry: crate::tasks::shared::targets::ChannelRegistryCell,
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
        Self {
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            provider_registry,
            tool_registry,
            tools: Arc::new(tools),
            workspace_manager: None,
            memory_backend,
            task_router: None,
            compression_service: None,
            memory_context_provider: None,
            global_tool_permissions: Default::default(),
            session_compactor: None,
            session_manager: None,
            event_bus: None,
            media_processor: None,
            state_database: None,
            teammate_manager: None,
            message_router: None,
            inbox: None,
            orchestrator: Arc::new(std::sync::OnceLock::new()),
            channel_registry: Arc::new(tokio::sync::OnceCell::new()),
        }
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

    /// Return the orchestrator OnceLock handle so boot code can inject the
    /// `Arc<Orchestrator>` after `initialize_orchestrator` completes.
    pub fn orchestrator_cell(
        &self,
    ) -> Arc<std::sync::OnceLock<Arc<crate::orchestrator::Orchestrator>>> {
        self.orchestrator.clone()
    }

    /// Set a task router for pre-classification of incoming requests.
    pub fn with_task_router(mut self, router: Arc<dyn crate::routing::TaskRouter>) -> Self {
        self.task_router = Some(router);
        self
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
    pub fn with_memory_context_provider(
        mut self,
        provider: Arc<crate::thinker::MemoryContextProvider>,
    ) -> Self {
        self.memory_context_provider = Some(provider);
        self
    }

    /// Set a session compactor for hierarchical session summarization.
    pub fn with_session_compactor(
        mut self,
        compactor: Arc<crate::memory::session_compactor::SessionCompactor>,
    ) -> Self {
        self.session_compactor = Some(compactor);
        self
    }

    /// Set global tool permission policy.
    pub fn with_global_tool_permissions(
        mut self,
        permissions: crate::config::types::policies::ToolPermissionsConfig,
    ) -> Self {
        self.global_tool_permissions = permissions;
        self
    }

    /// Set session manager and event bus for auto-topic generation.
    pub fn with_session_topic_support(
        mut self,
        session_manager: Arc<dyn crate::gateway::session_store::SessionStore>,
        event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
    ) -> Self {
        self.session_manager = Some(session_manager);
        self.event_bus = Some(event_bus);
        self
    }

    /// Set the media processor for multimodal attachment handling.
    pub fn with_media_processor(
        mut self,
        processor: Arc<crate::media::processor::MediaProcessor>,
    ) -> Self {
        self.media_processor = Some(processor);
        self
    }

    /// Set the resilience state database for task/trace persistence.
    pub fn with_state_database(mut self, state_database: Arc<StateDatabase>) -> Self {
        self.state_database = Some(state_database);
        self
    }

    /// Set the workspace manager for workspace-scoped profile resolution.
    ///
    /// When set, the engine resolves the user's active workspace at the start
    /// of each run and injects the workspace profile into the prompt builder
    /// and the workspace_id into the request context metadata.
    pub fn with_workspace_manager(mut self, manager: Arc<AgentEnvStore>) -> Self {
        self.workspace_manager = Some(manager);
        self
    }

    /// Set the teammate manager for named sub-agent team creation/registration.
    pub fn with_teammate_manager(
        mut self,
        mgr: Arc<crate::agents::teammates::TeammateManager>,
    ) -> Self {
        self.teammate_manager = Some(mgr);
        self
    }

    /// Set the message router for sub-agent send_message actions.
    pub fn with_message_router(
        mut self,
        router: Arc<crate::teams::messages::router::MessageRouter>,
    ) -> Self {
        self.message_router = Some(router);
        self
    }

    /// Set the inbox for sub-agent read_inbox actions.
    pub fn with_inbox(mut self, inbox: Arc<crate::teams::messages::inbox::Inbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    /// Get the number of currently active (non-completed) runs
    pub async fn active_run_count(&self) -> usize {
        self.active_runs.read().await.len()
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
}
