//! Agent loop execution and streaming callback.
#![allow(dead_code)]
//!
//! Contains `run_agent_loop` (the think-act two-step loop).

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::sync_primitives::Arc;

use super::{ExecutionError, RunRequest};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::engine::ExecutionEngine;

// Re-export submodules for internal use
pub(super) use super::callback::{CallbackStateFlushHandle, StreamCallbackState, TracePersistence};
pub(super) use super::history::build_loop_history;
pub(super) use super::tool_refresh::{active_plugin_tools_for_agent, ExtensionToolRefreshSource};

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

        // Snapshot the extension HookExecutor for this request — fires
        // `BeforeToolCall` / `AfterToolCall` / `AfterToolCallFailure` hooks
        // around every tool dispatch inside `ScopedToolService`. `None` when
        // no extension manager is present or no hooks are registered, so the
        // tool path skips the executor entirely.
        let hook_executor = if let Some(ext_manager) = extension_manager.as_ref() {
            let snapshot = ext_manager.hook_executor_snapshot().await;
            (snapshot.hook_count() > 0).then(|| Arc::new(snapshot))
        } else {
            None
        };
        let hook_session_id = request.session_key.to_key_string();

        // Build tool registry inputs (filtered by agent whitelist).
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

        // When a Skill slash command kicks off this run, restrict the tool
        // surface to the skill's declared `allowed_tools` (set by execute.rs
        // from the parsed CommandContext::Skill). Without this the LLM sees
        // the agent's full toolset and the skill's intent to scope tool use
        // is silently ignored. Empty / missing key preserves legacy behavior.
        //
        // FOLLOW-UP: `slash_skill_instructions` is also written into
        // request.metadata by execute.rs but only consumed via PromptBuilder
        // when the orchestrator path is used (see harness_bridge.rs).
        // The legacy gateway path here does not yet thread that string into
        // the system prompt overlay — the skill's `<instructions>` are
        // still relying on the `<available_skills>` block in the system
        // prompt to nudge the LLM to follow the skill's spec. A dedicated
        // overlay slot in PromptBuilder is the right place to plumb it.
        if let Some(raw) = request.metadata.get("slash_skill_allowed_tools") {
            let skill_whitelist: std::collections::HashSet<&str> =
                raw.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            if !skill_whitelist.is_empty() {
                let before = allowed_tools.len();
                allowed_tools.retain(|t| skill_whitelist.contains(t.name.as_str()));
                info!(
                    run_id = run_id,
                    before,
                    after = allowed_tools.len(),
                    skill_whitelist = ?skill_whitelist,
                    "Applied slash-skill allowed_tools restriction"
                );
            }
        }

        let _agent_perms = agent.config().tool_permissions();
        let _max_loops = agent.config().max_loops as usize;
        let token_budget = agent.config().max_tokens.unwrap_or(500_000);

        // Load conversation history from session (for multi-turn context)
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
        const MAX_FALLBACK_ATTEMPTS: usize = 3;
        let mut attempt = 0usize;
        let callback_state = Arc::new(StreamCallbackState::new(trace_task_id.and_then(
            |task_id| {
                self.state_database
                    .as_ref()
                    .map(|db| Arc::new(TracePersistence::new(db.clone(), task_id)))
            },
        )));

        // Routing context for HITL tools (sandbox escalation,
        // `requires_confirmation`, `ask_user`). Constant across retries.
        // Channel id / conversation id come from the inbound router's metadata;
        // empty for non-channel turns (cron, webhook) — HITL tools degrade.
        let turn_context = crate::tools::turn_context::TurnContext {
            session_key: request.session_key.clone(),
            channel_id: request
                .metadata
                .get("channel_id")
                .cloned()
                .unwrap_or_default(),
            conversation_id: request
                .metadata
                .get("conversation_id")
                .cloned()
                .unwrap_or_default(),
        };

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

            // Build per-request ToolService with SubagentTool + optional MCP refresh
            let loop_registry = Arc::new(crate::tools::adapters::build_registry_from_tools(
                self.tool_registry.clone(),
                &allowed_tools,
                default_working_dir.clone(),
            ));

            let allowed_names: std::collections::BTreeSet<String> =
                allowed_tools.iter().map(|t| t.name.clone()).collect();

            // Compose refresh sources: plugin tools (when an extension manager
            // is wired) + markdown CLI skills (always — installed via the
            // `skills.install` RPC into the process-wide markdown server).
            let mut refresh_sources: Vec<Arc<dyn crate::tools::refresh::ToolRefreshSource>> =
                Vec::new();
            if let Some(ext_manager) = extension_manager.as_ref() {
                refresh_sources.push(Arc::new(ExtensionToolRefreshSource::new(
                    Arc::clone(ext_manager),
                    self.tool_registry.clone(),
                    agent.clone(),
                    base_allowed_tools.clone(),
                    default_working_dir.clone(),
                ))
                    as Arc<dyn crate::tools::refresh::ToolRefreshSource>);
            }
            refresh_sources.push(Arc::new(
                super::markdown_skill_refresh::MarkdownSkillRefreshSource::new(),
            )
                as Arc<dyn crate::tools::refresh::ToolRefreshSource>);
            let tool_refresh: Option<Arc<dyn crate::tools::refresh::ToolRefreshSource>> = Some(
                Arc::new(crate::tools::refresh::CompositeRefreshSource::new(
                    refresh_sources,
                )) as Arc<dyn crate::tools::refresh::ToolRefreshSource>,
            );

            // Build parent view ToolService WITHOUT the subagent tool
            let parent_view_for_children: Arc<dyn crate::tools::service::ToolService> =
                super::build_request_tool_service(
                    loop_registry.clone(),
                    allowed_names.clone(),
                    None,
                    tool_refresh.clone(),
                    Some(turn_context.clone()),
                    hook_executor.clone(),
                    hook_session_id.clone(),
                );

            // Trace sink — built before SubagentTool so it can be inherited by
            // runtime-spawned subagents (B3): their run events flow into the
            // same gateway sink as the main runner, and the background path's
            // ForwardingTraceSink populates check_status progress.
            let trace_sink: Arc<dyn crate::harness::TraceSink> =
                Arc::new(super::GatewayTraceSink::new(Arc::new(
                    CallbackStateFlushHandle::new(callback_state.clone()),
                )));

            // SubagentTool construction
            let subagent_tool = {
                use crate::agents::background_tracker::BackgroundAgentTracker;
                use crate::agents::subagent_tool::SubagentTool;
                use crate::agents::AgentRegistry;

                let orchestrator = match self.orchestrator.get() {
                    Some(o) => o,
                    None => {
                        error!(
                            run_id = run_id,
                            "Orchestrator not wired when constructing SubagentTool"
                        );
                        return Err(ExecutionError::Orchestrator(
                            "orchestrator not yet initialised — boot ordering error".to_string(),
                        ));
                    }
                };

                // Reuse the boot-time `AgentRegistry` (same Arc the harness
                // received). Falling back to `with_builtins()` only protects
                // test fixtures / the simple engine that skip
                // `Orchestrator::with_agent_registry` — production must
                // always have it set, otherwise subagents cannot resolve
                // user-defined agents.
                let agent_registry = orchestrator
                    .agent_registry
                    .clone()
                    .unwrap_or_else(|| {
                        warn!(
                            run_id = run_id,
                            "Orchestrator has no agent_registry; subagent will only see built-ins (boot wiring gap)"
                        );
                        Arc::new(AgentRegistry::with_builtins())
                    });
                let background_tracker = Arc::new(BackgroundAgentTracker::new());
                let run_chain = crate::harness::chain_context::ChainContext::new();
                let sub_session = orchestrator.session_service.clone();
                // Phase 3 — route spawned subagents through the same
                // FailoverProvider chain the main harness uses, and surface the
                // per-`provider_hint` override registry. Without this the
                // gateway handed subagents a bare provider, bypassing failover.
                // Falls back to the bare default provider only when the
                // orchestrator carries no chain (test / simple-engine paths).
                let (sub_provider, agent_overrides) = match &orchestrator.subagent_routing {
                    Some(chain) => (chain.default.current(), chain.agent_overrides.clone()),
                    None => (
                        self.provider_registry.default_provider(),
                        std::collections::HashMap::new(),
                    ),
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
                )
                .with_parent_agent_id(request.session_key.agent_id().to_string())
                .with_parent_session_id(request.session_key.to_key_string())
                .with_cancel_token(cancel_token.clone())
                .with_trace_sink(trace_sink.clone())
                .with_provider_overrides(agent_overrides);
                if let Some(ref mgr) = self.teammate_manager {
                    t = t.with_teammate_manager(mgr.clone());
                }
                if let Some(ref router) = self.message_router {
                    t = t.with_message_router(router.clone());
                }
                if let Some(ref inbox) = self.inbox {
                    t = t.with_inbox(inbox.clone());
                }
                if let Some(ref mb) = self.memory_backend {
                    t = t.with_raw_memory_writer(
                        mb.clone() as Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>
                    );
                }
                Arc::new(t)
            };

            let tool_service = super::build_request_tool_service(
                loop_registry,
                allowed_names,
                Some(subagent_tool),
                tool_refresh,
                Some(turn_context.clone()),
                hook_executor.clone(),
                hook_session_id.clone(),
            );

            // Build FlowRequest
            let flow_input =
                super::helpers::history_to_flow_input(history.clone(), request.input.clone());

            // Phase 4 (F4): derive the channel's InteractionManifest from
            // the platform string already carried in request.metadata.
            // `paradigm_for_channel_type` maps "cli" → CLI, the messaging
            // channels (telegram/feishu/slack/whatsapp/etc.) → Messaging,
            // web variants → WebRich, and anything unknown → Background.
            // The harness bridge will adapt `OperationalGuidelinesLayer`,
            // `ProtocolTokensLayer`, etc. accordingly.
            let interaction_manifest = request.metadata.get("platform").map(|p| {
                crate::thinker::interaction::InteractionManifest::new(
                    crate::gateway::channel::paradigm_for_channel_type(p),
                )
            });

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
                interaction_manifest,
                // G2 — forward the per-run sandbox override so the team
                // dispatcher's WorktreeSandbox replaces the orchestrator's
                // sandbox_factory output for this run.
                sandbox_override: request.sandbox_override.clone(),
            };

            // Dispatch via the orchestrator
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
                Err(super::helpers::DispatchFailure::Transient {
                    provider: prov_name,
                    message,
                }) if attempt < MAX_FALLBACK_ATTEMPTS => {
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
                Err(super::helpers::DispatchFailure::Transient {
                    provider: prov_name,
                    message,
                }) => {
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
