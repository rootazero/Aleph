//! Agent loop execution and streaming callback.
//!
//! Contains `run_agent_loop` (the think-act two-step loop), the `StreamCallback`
//! adapter that bridges `LoopCallback` to Gateway `StreamEvent`s, the
//! `ExecutionAdapter` trait implementation, and background memory persistence.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::sync_primitives::{
    Arc, AtomicBool, AtomicU32, AtomicU64, Mutex, Ordering,
};

use super::{ExecutionError, RunRequest, RunStatus};
use crate::gateway::agent_instance::{AgentInstance, MessageRole};
use crate::gateway::event_emitter::{DynEventEmitter, EventEmitter, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::media::{PendingMedia};

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::engine::ExecutionEngine;

// Phase 6b Task 4c: `ExtensionSkillDiscoverySource` previously wrapped
// `SkillSystem` as an `agent_loop::SkillDiscoverySource` implementor for
// `AgentLoop::with_skill_prefetcher`. The harness now owns prefetch via
// `HarnessDeps.skill_prefetcher`, so this adapter has no call site in this
// file. Retained via the noop cast below; Phase 6c removes outright.

fn plugin_tool_to_unified_tool(
    tool: crate::extension::ToolRegistration,
) -> crate::dispatcher::UnifiedTool {
    let mut unified = crate::dispatcher::UnifiedTool::new(
        format!("plugin:{}:{}", tool.plugin_id, tool.name),
        &tool.name,
        &tool.description,
        crate::dispatcher::ToolSource::Plugin {
            plugin_id: tool.plugin_id.clone(),
        },
    );
    unified.parameters_schema = Some(tool.parameters);
    unified
}

fn active_plugin_tools_for_agent(
    extension_manager: &crate::extension::ExtensionManager,
    agent: &crate::gateway::agent_instance::AgentInstance,
) -> Vec<crate::dispatcher::UnifiedTool> {
    extension_manager
        .active_plugin_tools_snapshot()
        .into_iter()
        .filter(|tool| agent.is_tool_allowed(&tool.name))
        .map(plugin_tool_to_unified_tool)
        .collect()
}

#[derive(Clone)]
struct ExtensionToolRefreshSource<R: ToolRegistry + 'static> {
    extension_manager: Arc<crate::extension::ExtensionManager>,
    tool_registry: Arc<R>,
    agent: Arc<AgentInstance>,
    base_tools: Vec<crate::dispatcher::UnifiedTool>,
    default_working_dir: Option<String>,
    last_seen_revision: Arc<Mutex<u64>>,
}

impl<R: ToolRegistry + 'static> ExtensionToolRefreshSource<R> {
    fn new(
        extension_manager: Arc<crate::extension::ExtensionManager>,
        tool_registry: Arc<R>,
        agent: Arc<AgentInstance>,
        base_tools: Vec<crate::dispatcher::UnifiedTool>,
        default_working_dir: Option<String>,
    ) -> Self {
        Self {
            last_seen_revision: Arc::new(Mutex::new(extension_manager.plugin_tool_revision())),
            extension_manager,
            tool_registry,
            agent,
            base_tools,
            default_working_dir,
        }
    }

    fn merged_tools(&self) -> Vec<crate::dispatcher::UnifiedTool> {
        let mut tools = self.base_tools.clone();
        tools.extend(active_plugin_tools_for_agent(
            &self.extension_manager,
            &self.agent,
        ));
        tools
    }
}

impl<R: ToolRegistry + 'static> crate::tools::refresh::ToolRefreshSource
    for ExtensionToolRefreshSource<R>
{
    fn poll_changes(&self) -> bool {
        let current_revision = self.extension_manager.plugin_tool_revision();
        let mut last_seen = self
            .last_seen_revision
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if current_revision != *last_seen {
            *last_seen = current_revision;
            return true;
        }

        false
    }

    fn fetch_tools(&self) -> Vec<Box<dyn crate::tools::runtime::LoopTool>> {
        crate::harness::adapters::build_tool_adapters_from_tools(
            self.tool_registry.clone(),
            &self.merged_tools(),
            self.default_working_dir.clone(),
        )
    }
}

// ============================================================================
// Agent loop execution
// ============================================================================

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Run the agent loop (think->act two-step, Claude Code-inspired).
    ///
    /// Uses the flat `LoopToolRegistry` and single-layer `SafetyGuard`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
        trace_task_id: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<String, ExecutionError> {
        use crate::providers::model_behaviors::{load_model_behavior, protocol_to_behavior};

        info!(run_id = run_id, "Starting agent loop (think->act)");

        // Write workspace-scoped output paths to tool context handle
        if let Some(tc_handle) = self.tool_registry.tool_context_handle() {
            let workspace_path = agent.workspace();
            match crate::tools::ToolContext::from_workspace(workspace_path) {
                Ok(ctx) => {
                    let mut tc = tc_handle.write().await;
                    *tc = ctx;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to create ToolContext from workspace {}: {}",
                        workspace_path.display(),
                        e
                    );
                }
            }
        }

        // === Pre-compute values reusable across retry attempts ===

        let default_working_dir = Some(agent.workspace().to_string_lossy().to_string());

        // Resolve soul for prompt building (constant across retries).
        //
        // Identity files (SOUL.md / IDENTITY.md / AGENTS.md / MEMORY.md / …)
        // live in the AGENT identity directory (`~/.aleph/agents/{agent_id}/`).
        //
        // Phase 6b Task 4c flip: these identity fragments are now assembled by
        // the `AgentHarness` prompt sections layer (`src/harness/sections`), so
        // we no longer construct `IdentityFiles` / `SoulLayer` / `IdentityResolver`
        // directly here. The harness boot wires the same sources through
        // `HarnessDeps` at startup.
        let _ = agent.agent_dir();

        let extension_manager: Option<Arc<crate::extension::ExtensionManager>> =
            crate::gateway::handlers::plugins::get_extension_manager()
                .ok()
                .map(Arc::clone);

        if let Some(ext_manager) = extension_manager.as_ref() {
            if let Err(e) = ext_manager.ensure_loaded().await {
                tracing::warn!("Failed to ensure extension manager is loaded: {}", e);
            }
        }

        // Phase 6b Task 4c flip: skill prefetch + hook executor are now owned
        // by `AgentHarnessRunner` (via `HarnessDeps.skill_prefetcher` / the
        // `ToolHookDecorator` pathway in `ScopedToolService`). These locals
        // remain as call-site breadcrumbs for Phase 6c cleanup.
        let _eligible_skills: Option<Vec<crate::domain::skill::SkillManifest>> =
            if let Some(ext_manager) = extension_manager.as_ref() {
                let snapshot = ext_manager.skill_system().current_snapshot().await;
                if !snapshot.eligible_manifests.is_empty() {
                    Some(snapshot.eligible_manifests)
                } else {
                    None
                }
            } else {
                None
            };
        let _hook_executor = if let Some(ext_manager) = extension_manager.as_ref() {
            let snapshot = ext_manager.hook_executor_snapshot().await;
            (snapshot.hook_count() > 0).then(|| Arc::new(snapshot))
        } else {
            None
        };
        let _skill_system = extension_manager
            .as_ref()
            .map(|ext_manager| ext_manager.skill_system().clone());

        // Build tool registry inputs (filtered by agent whitelist).
        // Static builtins come from engine startup; plugin tools are resolved
        // from the live extension snapshot so hot-reload can refresh them.
        let base_allowed_tools: Vec<crate::dispatcher::UnifiedTool> = self
            .tools
            .iter()
            .filter(|t| !matches!(t.source, crate::dispatcher::ToolSource::Plugin { .. }))
            .filter(|t| agent.is_tool_allowed(&t.name))
            .cloned()
            .collect();
        let mut allowed_tools = base_allowed_tools.clone();
        if let Some(ext_manager) = extension_manager.as_ref() {
            allowed_tools.extend(active_plugin_tools_for_agent(ext_manager, &agent));
        } else {
            allowed_tools.extend(
                self.tools
                    .iter()
                    .filter(|t| matches!(t.source, crate::dispatcher::ToolSource::Plugin { .. }))
                    .filter(|t| agent.is_tool_allowed(&t.name))
                    .cloned(),
            );
        }

        // Agent config values — `max_loops` / `agent_perms` moved into the
        // AgentHarnessRunner boot path (harness owns iteration cap +
        // safety guard assembly now). Token budget is still used by the
        // session compactor below for history prep.
        let _agent_perms = agent.config().tool_permissions();
        let _max_loops = agent.config().max_loops as usize;
        let token_budget = agent.config().max_tokens.unwrap_or(500_000);

        // Load conversation history from session (for multi-turn context)
        // Compression time excluded from agent's timeout budget
        let before_compress = tokio::time::Instant::now();
        let history = if let Some(ref sc) = self.session_compactor {
            sc.prepare_history(
                &agent,
                &request.session_key,
                &request.input,
                token_budget as u64,
            )
            .await
        } else {
            build_loop_history(&agent, &request.session_key, &request.input).await
        };
        let compress_elapsed = before_compress.elapsed();
        if !compress_elapsed.is_zero() {
            *deadline.lock().await += compress_elapsed;
        }

        // Pre-process multimodal attachments (constant across retries)
        let multimodal_messages: Option<Vec<crate::providers::message::UnifiedMessage>> =
            if let (false, Some(media_processor)) = (
                request.attachments.is_empty(),
                self.media_processor.as_ref(),
            ) {
                let supports_vision = true;
                let media_blocks = media_processor
                    .process(
                        &request.attachments,
                        supports_vision,
                        &request.session_key.to_key_string(),
                        run_id,
                    )
                    .await;

                let mut content = vec![crate::providers::message::ContentBlock::Text {
                    text: request.input.clone(),
                    cache_control: None,
                }];
                content.extend(media_blocks);

                let has_images = content
                    .iter()
                    .any(|b| matches!(b, crate::providers::message::ContentBlock::Image { .. }));
                let has_transcripts = content.iter().any(|b| {
                    if let crate::providers::message::ContentBlock::Text { text, .. } = b {
                        text.starts_with("[Voice message transcript]")
                    } else {
                        false
                    }
                });
                tracing::info!(
                    target: "multimodal",
                    probe = "P5_inject",
                    run_id = %request.run_id,
                    content_blocks = content.len(),
                    has_images = has_images,
                    has_transcripts = has_transcripts,
                    "Multimodal UnifiedMessage built"
                );

                let mut msgs = history.clone();
                msgs.push(crate::providers::message::UnifiedMessage::user_with_content(content));
                Some(msgs)
            } else {
                None
            };

        // === Retry loop: resolve provider → build agent loop → run ===
        // On transient provider failure, report degraded, re-resolve, and retry
        // with a different provider. Max 3 attempts to prevent infinite loops.
        const MAX_FALLBACK_ATTEMPTS: usize = 3;
        let mut attempt = 0usize;
        let callback_state = Arc::new(StreamCallbackState::new(trace_task_id.and_then(
            |task_id| {
                self.state_database
                    .as_ref()
                    .map(|db| Arc::new(TracePersistence::new(db.clone(), task_id)))
            },
        )));

        loop {
            attempt += 1;

            // Resolve model with health-aware fallback
            let resolved = self
                .provider_registry
                .resolve_with_fallback(&agent.config().model, &agent.config().fallback_models)
                .map_err(|e| ExecutionError::Failed(e.to_string()))?;

            if resolved.is_fallback {
                info!(
                    run_id = run_id,
                    attempt = attempt,
                    original = %resolved.original_model,
                    fallback_provider = %resolved.provider_name,
                    fallback_model = %resolved.model,
                    "Using fallback model"
                );
            }

            // Emit ModelResolved so the Panel can show fallback indicators
            let _ = emitter
                .emit(StreamEvent::ModelResolved {
                    run_id: run_id.to_string(),
                    model_info: crate::providers::health::ModelInfo {
                        model: resolved.model.clone(),
                        provider: resolved.provider_name.clone(),
                        is_fallback: resolved.is_fallback,
                        original_model: if resolved.is_fallback {
                            Some(resolved.original_model.clone())
                        } else {
                            None
                        },
                    },
                })
                .await;

            // Resolve model behavior: config override > protocol auto-mapping.
            // Behaviour content is no longer consumed at this seam (harness
            // owns provider prompt assembly), but we still load it for the
            // side-effect of surfacing config/protocol mismatches via
            // tracing.
            {
                let provider = self
                    .provider_registry
                    .get(&resolved.provider_name)
                    .unwrap_or_else(|| self.provider_registry.default_provider());
                let behavior_name = provider
                    .model_behavior_override()
                    .or_else(|| protocol_to_behavior(provider.protocol()));
                let content = match behavior_name {
                    Some(name) => load_model_behavior(name).await,
                    None => None,
                };
                info!(
                    run_id = run_id,
                    protocol = %provider.protocol(),
                    behavior_name = ?behavior_name,
                    loaded = content.is_some(),
                    "Model behavior resolved"
                );
            }

            // -- Phase 6b Task 4c: flip from AgentLoop::new to Orchestrator::dispatch -----
            //
            // The retry-loop outer frame stays identical: provider resolution +
            // `ModelResolved` emission + health-aware fallback are unchanged.
            // Inside each attempt we now:
            //   1. Build a per-request `ScopedToolService` carrying SubagentTool
            //      + MCP refresh (was: `AgentLoop::register(subagent_tool)`).
            //   2. Build a `GatewayTraceSink` wrapping the existing callback
            //      state for persistence.
            //   3. Construct a `FlowRequest` + dispatch via the orchestrator
            //      helper, which drains events into the Gateway emitter.
            //   4. On `DispatchFailure::Transient`, report outcome + retry.
            //
            // Design dropped deliberately (per PHASE_6B_BUILDER_AUDIT + §5
            // resolution design): `with_chain`, `with_shared_snapshot`,
            // `with_provider_name`, `with_platform_name`, `with_session_id`,
            // `with_hook_executor`, `with_tool_refresh` (hook_executor is
            // user-tool-hooks, not yet re-wired), streaming_sink (replaced by
            // FlowStreamEvent::Delta), prompt_builder (harness owns prompt
            // assembly via `src/harness/sections`).

            // 1. Build per-request ToolService with SubagentTool + optional
            //    MCP refresh. `LoopToolRegistry` is built from the resolved
            //    `allowed_tools` so the view is identical to the retiring
            //    registry path.
            let loop_registry = Arc::new(crate::harness::adapters::build_registry_from_tools(
                self.tool_registry.clone(),
                &allowed_tools,
                default_working_dir.clone(),
            ));

            let allowed_names: std::collections::BTreeSet<String> = allowed_tools
                .iter()
                .map(|t| t.name.clone())
                .collect();

            let tool_refresh: Option<Arc<dyn crate::tools::refresh::ToolRefreshSource>> =
                extension_manager.as_ref().map(|ext_manager| {
                    Arc::new(ExtensionToolRefreshSource::new(
                        Arc::clone(ext_manager),
                        self.tool_registry.clone(),
                        agent.clone(),
                        base_allowed_tools.clone(),
                        default_working_dir.clone(),
                    )) as Arc<dyn crate::tools::refresh::ToolRefreshSource>
                });

            // First build a "parent view" ToolService WITHOUT the subagent tool.
            // This becomes the child subagents' parent_tools — `subagent_spawner`
            // wraps it with `AllowlistToolService(child agent_def)` to produce
            // the child harness's tool service. Omitting subagent_tool here
            // avoids an Arc cycle (parent tool_service -> SubagentTool ->
            // parent_tools -> ...) and prevents recursive subagent spawns
            // without relying on the child's allowlist for that guarantee.
            let parent_view_for_children: Arc<dyn crate::tools::service::ToolService> =
                super::build_request_tool_service(
                    loop_registry.clone(),
                    allowed_names.clone(),
                    None,
                    tool_refresh.clone(),
                );

            // SubagentTool is attached to the ScopedToolService (not registered
            // in LoopToolRegistry) so the service surfaces it via `list()`.
            let subagent_tool = {
                use crate::agents::background_tracker::BackgroundAgentTracker;
                use crate::agents::subagent_tool::SubagentTool;
                use crate::agents::AgentRegistry;
                let sub_provider = self.provider_registry.default_provider();
                let agent_registry = Arc::new(AgentRegistry::with_builtins());
                let background_tracker = Arc::new(BackgroundAgentTracker::new());
                // ChainContext is used by SubagentTool for chain-id correlation.
                // The retiring path shared this between AgentLoop and SubagentTool;
                // under the orchestrator flip only SubagentTool sees it since
                // AgentHarness has its own tracing path via TraceSink.
                let run_chain = crate::harness::chain_context::ChainContext::new();

                // Phase 7 Task 6 + follow-up: SubagentTool now owns the child
                // harness deps (session / parent_tools / sandbox). Session is
                // sourced from the orchestrator so child ephemeral sessions
                // share the parent's SessionActor. parent_tools is the
                // `parent_view_for_children` built above — `subagent_spawner`
                // wraps it with `AllowlistToolService(child agent_def)` to
                // produce the child harness's tool view. Sandbox defaults to
                // `NoopSandbox` (child harness runs in-process and does not
                // use the sandbox factory today).
                let sub_session: Arc<dyn crate::session::service::SessionService> =
                    match self.orchestrator.get() {
                        Some(o) => o.session_service.clone(),
                        None => {
                            // Boot ordering fallback — surface via the same
                            // error path the orchestrator dispatch uses below.
                            error!(
                                run_id = run_id,
                                "Orchestrator not wired when constructing SubagentTool"
                            );
                            return Err(ExecutionError::Orchestrator(
                                "orchestrator not yet initialised — boot ordering error".to_string(),
                            ));
                        }
                    };
                let sub_sandbox: Arc<dyn crate::sandbox::Sandbox> =
                    Arc::new(crate::sandbox::NoopSandbox);

                let mut t = SubagentTool::new(
                    sub_provider,
                    run_chain,
                    agent_registry,
                    background_tracker,
                    sub_session,
                    parent_view_for_children,
                    sub_sandbox,
                );
                if let Some(ref mgr) = self.teammate_manager {
                    t = t.with_teammate_manager(mgr.clone());
                }
                if let Some(ref router) = self.message_router {
                    t = t.with_message_router(router.clone());
                }
                if let Some(ref inbox) = self.inbox {
                    t = t.with_inbox(inbox.clone());
                }
                Arc::new(t)
            };

            let tool_service = super::build_request_tool_service(
                loop_registry,
                allowed_names,
                Some(subagent_tool),
                tool_refresh,
            );

            // 2. Trace sink: wraps callback_state so TracePersistence continues
            //    to run. `flush` is called by AgentHarnessRunner::run after the
            //    inner harness loop completes.
            let trace_sink: Arc<dyn crate::harness::TraceSink> =
                Arc::new(super::GatewayTraceSink::new(Arc::new(
                    CallbackStateFlushHandle::new(callback_state.clone()),
                )));

            // 3. Build FlowRequest — history replay + fresh prompt. Multimodal
            //    messages currently degrade to text via History (attachments
            //    are handled through the media pipeline + session events;
            //    harness does not yet accept UnifiedMessage directly).
            let flow_input = super::helpers::history_to_flow_input(
                history.clone(),
                request.input.clone(),
            );

            let req = crate::orchestrator::FlowRequest {
                flow_id: None,
                agent_id: agent.id().to_string(),
                input: flow_input,
                channel: request.metadata.get("platform").cloned(),
                session_hint: Some(request.session_key.to_key_string()),
                parent_session: None,
                depth: 0,
                tool_service: Some(tool_service),
                trace_sink: Some(trace_sink),
            };

            // 4. Dispatch via the orchestrator + classify.
            let orchestrator = match self.orchestrator.get() {
                Some(o) => o.clone(),
                None => {
                    error!(
                        run_id = run_id,
                        "Orchestrator not wired; ExecutionEngine::orchestrator empty"
                    );
                    return Err(ExecutionError::Orchestrator(
                        "orchestrator not yet initialised — boot ordering error".to_string(),
                    ));
                }
            };

            let locale = crate::gateway::i18n::Locale::from_config(
                request.metadata.get("locale").map(|s| s.as_str()),
            );

            let emitter_dyn: Arc<dyn crate::gateway::event_emitter::EventEmitter> =
                emitter.clone() as Arc<dyn crate::gateway::event_emitter::EventEmitter>;

            let dispatch_result = super::helpers::run_dispatch_and_drain_classified(
                orchestrator,
                req,
                emitter_dyn,
                run_id,
                cancel_token.clone(),
                locale,
            )
            .await;

            match dispatch_result {
                Ok(response) => {
                    self.provider_registry
                        .report_outcome(&resolved.provider_name, Ok(()));
                    if multimodal_messages.is_some() {
                        if let Some(mp) = self.media_processor.as_ref() {
                            mp.cleanup(&request.session_key.to_key_string());
                        }
                    }
                    info!(
                        run_id = run_id,
                        attempt = attempt,
                        "Orchestrator dispatch completed"
                    );
                    return Ok(response);
                }
                Err(super::helpers::DispatchFailure::Cancelled) => {
                    if multimodal_messages.is_some() {
                        if let Some(mp) = self.media_processor.as_ref() {
                            mp.cleanup(&request.session_key.to_key_string());
                        }
                    }
                    return Err(ExecutionError::Cancelled);
                }
                Err(super::helpers::DispatchFailure::Transient { provider: prov_name, message })
                    if attempt < MAX_FALLBACK_ATTEMPTS =>
                {
                    // Mark the provider degraded and try again. The outer
                    // `loop` re-resolves on the next iteration.
                    self.provider_registry.report_outcome(
                        &resolved.provider_name,
                        Err(crate::providers::health::ProviderError::Transient(
                            crate::providers::health::TransientError::ConnectionFailed,
                        )),
                    );
                    warn!(
                        run_id = run_id,
                        attempt = attempt,
                        failed_provider = %prov_name,
                        message = %message,
                        "Provider transient failure via Orchestrator::dispatch — retrying"
                    );
                    continue;
                }
                Err(super::helpers::DispatchFailure::Transient { provider: prov_name, message }) => {
                    self.provider_registry.report_outcome(
                        &resolved.provider_name,
                        Err(crate::providers::health::ProviderError::Transient(
                            crate::providers::health::TransientError::ConnectionFailed,
                        )),
                    );
                    if multimodal_messages.is_some() {
                        if let Some(mp) = self.media_processor.as_ref() {
                            mp.cleanup(&request.session_key.to_key_string());
                        }
                    }
                    error!(
                        run_id = run_id,
                        attempt = attempt,
                        failed_provider = %prov_name,
                        message = %message,
                        "Orchestrator dispatch exhausted retries"
                    );
                    return Err(ExecutionError::Failed(format!(
                        "provider {prov_name} transient: {message}"
                    )));
                }
                Err(super::helpers::DispatchFailure::Fatal(msg)) => {
                    if multimodal_messages.is_some() {
                        if let Some(mp) = self.media_processor.as_ref() {
                            mp.cleanup(&request.session_key.to_key_string());
                        }
                    }
                    error!(
                        run_id = run_id,
                        attempt = attempt,
                        error = %msg,
                        "Orchestrator dispatch failed"
                    );
                    return Err(ExecutionError::Failed(msg));
                }
            }
        }
    }
}

/// Adapter that translates the existing `StreamCallbackState` into the
/// `TraceFlushHandle` the orchestrator-side `GatewayTraceSink` expects.
///
/// Keeps `TracePersistence::flush` wired — same persistence path as the
/// retiring `StreamCallback::flush_trace_persistence`. `on_trace` routes each
/// `LoopTraceEvent` into the same queue.
struct CallbackStateFlushHandle {
    state: Arc<StreamCallbackState>,
}

impl CallbackStateFlushHandle {
    fn new(state: Arc<StreamCallbackState>) -> Self {
        Self { state }
    }
}

impl super::trace_sink_adapter::TraceFlushHandle for CallbackStateFlushHandle {
    fn on_trace(&self, event: &crate::harness::trace::LoopTraceEvent) {
        // `agent_loop::LoopTraceEvent` is a re-export of `harness::trace::LoopTraceEvent`
        // (see `src/agent_loop/trace.rs`), so no translation needed today. The
        // trait boundary is kept distinct in case Phase 6c splits them.
        self.state.persist_trace(event);
    }

    fn flush_blocking(&self) {
        // Fire-and-forget blocking spawn; the existing flush is async and
        // returns only after all pending persistence handles drain.
        let state = self.state.clone();
        // We can't `.await` in a non-async function; use a current-thread
        // tokio handle if available. If no runtime is active (tests), the
        // block_on is inert since there are no pending handles to flush.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                state.flush_trace_persistence().await;
            });
        }
    }
}

// Phase 6b Task 4c: `contains_http_status` used to classify anyhow-wrapped
// provider errors. Classification now lives in
// `orchestrator::harness_bridge::classify_harness_error`, so this function
// moved there. (Kept out of this module to minimise dead code.)

/// Build loop history from the agent's session, excluding the current user input.
async fn build_loop_history(
    agent: &AgentInstance,
    session_key: &crate::gateway::router::SessionKey,
    current_input: &str,
) -> Vec<crate::providers::message::UnifiedMessage> {
    use crate::providers::message::UnifiedMessage;

    let session_history = agent.get_history(session_key, Some(50)).await;
    let mut msgs: Vec<UnifiedMessage> = Vec::new();

    // Skip the last message if it's the current user input we just stored
    let history_slice = if session_history
        .last()
        .map(|m| m.role == MessageRole::User && m.content == current_input)
        .unwrap_or(false)
    {
        // safe: last() returned Some, so len() >= 1
        &session_history[..session_history.len().saturating_sub(1)]
    } else {
        &session_history
    };

    for msg in history_slice {
        match msg.role {
            MessageRole::User => msgs.push(UnifiedMessage::user(msg.content.clone())),
            MessageRole::Assistant => msgs.push(UnifiedMessage::assistant(msg.content.clone())),
            _ => {}
        }
    }
    msgs
}

// ============================================================================
// StreamCallback — bridges LoopCallback to Gateway StreamEvents
// ============================================================================

struct TracePersistence {
    db: Arc<crate::resilience::StateDatabase>,
    task_id: String,
    next_step_index: AtomicU32,
    pending_writes: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl TracePersistence {
    fn new(db: Arc<crate::resilience::StateDatabase>, task_id: String) -> Self {
        Self {
            db,
            task_id,
            next_step_index: AtomicU32::new(0),
            pending_writes: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, event: &crate::harness::trace::LoopTraceEvent) {
        let step_index = self.next_step_index.fetch_add(1, Ordering::Relaxed);
        let db = self.db.clone();
        let task_id = self.task_id.clone();
        let trace_event: aleph_protocol::AgentTraceEvent = event.clone().into();

        let handle = tokio::spawn(async move {
            let trace = crate::resilience::TaskTrace::new(task_id.clone(), step_index, trace_event);
            if let Err(error) = db.insert_trace(&trace).await {
                tracing::warn!(
                    task_id = %task_id,
                    step_index,
                    error = %error,
                    "Failed to persist task trace"
                );
            }
        });

        self.pending_writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }

    async fn flush(&self) {
        let handles = {
            let mut pending = self
                .pending_writes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *pending)
        };

        for handle in handles {
            if let Err(error) = handle.await {
                tracing::warn!(error = %error, "Task trace persistence task failed");
            }
        }
    }
}

#[allow(dead_code)] // seq/chunk_index kept for StreamCallback wiring (cfg(test) only post-flip)
struct StreamCallbackState {
    seq: AtomicU64,
    chunk_index: AtomicU32,
    trace_persistence: Option<Arc<TracePersistence>>,
}

#[allow(dead_code)] // next_seq/next_chunk_index kept for StreamCallback (cfg(test) post-flip)
impl StreamCallbackState {
    fn new(trace_persistence: Option<Arc<TracePersistence>>) -> Self {
        Self {
            seq: AtomicU64::new(0),
            chunk_index: AtomicU32::new(0),
            trace_persistence,
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn next_chunk_index(&self) -> u32 {
        self.chunk_index.fetch_add(1, Ordering::SeqCst)
    }

    fn persist_trace(&self, event: &crate::harness::trace::LoopTraceEvent) {
        if let Some(trace_persistence) = self.trace_persistence.as_ref() {
            trace_persistence.record(event);
        }
    }

    async fn flush_trace_persistence(&self) {
        if let Some(trace_persistence) = self.trace_persistence.as_ref() {
            trace_persistence.flush().await;
        }
    }
}

/// Callback adapter that bridges AgentLoop events to Gateway StreamEvents.
#[allow(dead_code)] // retained for cfg(test) coverage of the trace-persistence seam
pub(super) struct StreamCallback<E: EventEmitter + Send + Sync + 'static> {
    emitter: Arc<E>,
    run_id: String,
    pending_media: PendingMedia,
    /// True when a StreamingDeltaSink is active for this run.
    /// When true, text tokens that were already delivered via DeltaSink are skipped.
    streaming_active: bool,
    /// Shared flag set by StreamingDeltaSink after each token delivery.
    /// StreamCallback swaps it to false and skips the duplicate on_text call.
    has_emitted_text: Arc<AtomicBool>,
    shared: Arc<StreamCallbackState>,
}

#[allow(dead_code)] // all methods retained for cfg(test) fixture
impl<E: EventEmitter + Send + Sync + 'static> StreamCallback<E> {
    fn new(
        emitter: Arc<E>,
        run_id: String,
        pending_media: PendingMedia,
        streaming_active: bool,
        has_emitted_text: Arc<AtomicBool>,
        shared: Arc<StreamCallbackState>,
    ) -> Self {
        Self {
            emitter,
            run_id,
            pending_media,
            streaming_active,
            has_emitted_text,
            shared,
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.shared.next_seq()
    }

    fn next_chunk_index(&self) -> u32 {
        self.shared.next_chunk_index()
    }

    fn emit_async(&self, event: StreamEvent) {
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            if let Err(e) = emitter.emit(event).await {
                tracing::warn!(error = %e, "StreamCallback: emit failed");
            }
        });
    }

    async fn flush_trace_persistence(&self) {
        self.shared.flush_trace_persistence().await;
    }
}

// ============================================================================
// ExecutionAdapter trait implementation
// ============================================================================

/// Implement ExecutionAdapter for the ExecutionEngine.
///
/// This allows InboundMessageRouter to use ExecutionEngine via a trait object,
/// enabling routing without being generic over provider and tool registry types.
#[async_trait]
impl<P, R> ExecutionAdapter for ExecutionEngine<P, R>
where
    P: ThinkerProviderRegistry + Send + Sync + 'static,
    R: ToolRegistry + Send + Sync + 'static,
{
    async fn execute(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        // Wrap the dyn trait object in DynEventEmitter to make it Sized,
        // then delegate to the existing generic execute method
        let wrapper = Arc::new(DynEventEmitter::new(emitter));
        ExecutionEngine::execute(self, request, agent, wrapper).await
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        ExecutionEngine::cancel(self, run_id).await
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        ExecutionEngine::get_status(self, run_id).await
    }

    async fn active_run_count(&self) -> usize {
        ExecutionEngine::active_run_count(self).await
    }
}

// ============================================================================
// Background memory persistence
// ============================================================================

/// Write a conversation turn to the memory system (Layer 1).
///
/// With SessionStore removed, this is a no-op. Raw conversations are
/// already stored in SessionManager's SQLite. Retained for API compatibility.
pub(super) async fn write_conversation_memory(
    _memory_backend: crate::memory::store::MemoryBackend,
    _session_key: String,
    _agent_id: String,
    _user_input: String,
    _ai_output: String,
) {
    // Raw memory persistence removed — SessionStore no longer exists.
    // Conversations are stored in SessionManager's SQLite.
    debug!("Conversation memory write skipped (SessionStore removed)");
}

// Phase 6b Task 4c: `detect_git_branch` produced the `git_branch` field on
// `agent_loop::SessionContext` consumed by `AgentLoop::with_session_context`.
// Under the orchestrator flip the harness assembles session context from its
// own sources, so this helper is no longer called from `run_agent_loop`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::trace::LoopTraceEvent;
    use crate::resilience::{AgentTask, RiskLevel};

    #[tokio::test]
    async fn stream_callback_persists_agent_trace_events() {
        let db = Arc::new(crate::resilience::StateDatabase::in_memory().unwrap());
        db.insert_agent_task(&AgentTask::new(
            "run-1",
            "session-1",
            "coder",
            "persist trace",
            RiskLevel::High,
        ))
        .await
        .unwrap();
        db.update_task_status("run-1", crate::resilience::TaskStatus::Running)
            .await
            .unwrap();

        let shared = Arc::new(StreamCallbackState::new(Some(Arc::new(
            TracePersistence::new(db.clone(), "run-1".to_string()),
        ))));

        // Test persistence directly via StreamCallbackState (post-flip: StreamCallback
        // is dead code; the production path uses GatewayTraceSink/CallbackStateFlushHandle).
        shared.persist_trace(&LoopTraceEvent::TurnStarted { iteration: 1 });
        shared.flush_trace_persistence().await;

        let traces = db.get_traces_by_task("run-1").await.unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].event.kind(), "turn_started");
        assert_eq!(
            traces[0].event,
            aleph_protocol::AgentTraceEvent::TurnStarted { iteration: 1 }
        );
    }
}
