//! Agent loop execution and streaming callback.
//!
//! Contains `run_agent_loop` (the think-act two-step loop).

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::sync_primitives::Arc;

use super::{ExecutionError, RunRequest};
use crate::extension::hooks::{HookContext, HookExecutor};
use crate::extension::HookEvent;
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
        // Resolve the extension manager + snapshot its HookExecutor once for
        // the whole run. Both flow into `run_agent_loop_inner` so tool
        // dispatch and history compaction can fire hooks without
        // re-snapshotting per turn.
        let extension_manager: Option<Arc<crate::extension::ExtensionManager>> =
            crate::gateway::handlers::plugins::get_extension_manager()
                .ok()
                .map(Arc::clone);
        if let Some(ext_manager) = extension_manager.as_ref() {
            if let Err(e) = ext_manager.ensure_loaded().await {
                warn!("Failed to ensure extension manager is loaded: {}", e);
            }
        }
        let hook_executor = if let Some(ext_manager) = extension_manager.as_ref() {
            let snapshot = ext_manager.hook_executor_snapshot().await;
            (snapshot.hook_count() > 0).then(|| Arc::new(snapshot))
        } else {
            None
        };
        let hook_session_id = request.session_key.to_key_string();

        // BeforeAgentStart — interceptor-kind hooks may abort the run before
        // any provider call; observer-kind hooks just witness the start.
        if let Some(executor) = hook_executor.as_ref() {
            let ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
            match executor
                .execute_interceptors(HookEvent::BeforeAgentStart, ctx)
                .await
            {
                Ok((_ctx, hr)) if hr.denied || hr.blocked => {
                    let reason = hr
                        .deny_reason
                        .or(hr.block_reason)
                        .unwrap_or_else(|| "agent start blocked by hook".to_string());
                    warn!(
                        run_id = run_id,
                        reason = %reason,
                        "BeforeAgentStart hook aborted the run"
                    );
                    return Err(ExecutionError::Failed(format!(
                        "BeforeAgentStart hook aborted the run: {reason}"
                    )));
                }
                Ok(_) => {}
                Err(e) => warn!(run_id = run_id, error = %e, "BeforeAgentStart hook failed"),
            }
        }

        // Publish the project root as a task-local for the duration of the
        // think→act loop so child runs spawned mid-loop (session.send, team
        // dispatcher worker tasks, etc.) inherit the project context.
        // `None` is also published explicitly so a nested run cannot leak
        // an outer scope's project into a non-project agent.
        let result = crate::projects::with_project_root(
            request.workspace_override.clone(),
            self.run_agent_loop_inner(
                run_id,
                request,
                agent.clone(),
                emitter,
                deadline,
                trace_task_id,
                cancel_token,
                extension_manager,
                hook_executor.clone(),
                hook_session_id.clone(),
            ),
        )
        .await;

        // AgentEnd — observers only; the run is already over, nothing to block.
        if let Some(executor) = hook_executor.as_ref() {
            let mut ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
            ctx = ctx.with_env("AGENT_OUTCOME", if result.is_ok() { "ok" } else { "error" });
            if let Err(ref e) = result {
                ctx = ctx.with_env("AGENT_ERROR", e.to_string());
            }
            executor.execute_observers(HookEvent::AgentEnd, &ctx).await;
        }
        result
    }

    /// Inner body of the think→act loop. `run_agent_loop` wraps this with the
    /// `BeforeAgentStart` / `AgentEnd` lifecycle hooks and supplies the
    /// pre-resolved extension manager + hook executor snapshot.
    #[allow(clippy::too_many_arguments)]
    async fn run_agent_loop_inner<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
        trace_task_id: Option<String>,
        cancel_token: CancellationToken,
        extension_manager: Option<Arc<crate::extension::ExtensionManager>>,
        hook_executor: Option<Arc<HookExecutor>>,
        hook_session_id: String,
    ) -> Result<String, ExecutionError> {
        use crate::providers::model_behaviors::{load_model_behavior, protocol_to_behavior};

        info!(run_id = run_id, "Starting agent loop (think->act)");

        // Effective workspace: if the request carries a per-run `workspace_override`
        // (project mode), use it; otherwise fall back to the agent's stable
        // `~/.aleph/workspaces/{agent_id}` directory.
        //
        // The override was validated as an existing absolute directory by the
        // gateway handler that constructed the RunRequest, but that check is
        // TOCTOU: between handler dispatch and `run_agent_loop` firing, the
        // directory can be unmounted, deleted by another process, or replaced
        // by a non-directory. Re-verify and fail the run cleanly with a
        // user-facing reason rather than discovering the disappearance
        // mid-tool-call.
        let effective_workspace: std::path::PathBuf = match &request.workspace_override {
            Some(p) => {
                if !p.is_dir() {
                    error!(
                        run_id = run_id,
                        project_root = %p.display(),
                        "project_root vanished between request and execution"
                    );
                    return Err(ExecutionError::Failed(format!(
                        "project folder no longer exists: {} — pick another folder \
                         or re-create it before retrying",
                        p.display()
                    )));
                }
                p.clone()
            }
            None => agent.workspace().to_path_buf(),
        };

        // Bump `last_used_at` in the project catalogue so the Panel picker
        // can sort by recency without each entry point (CLI, OpenAI-compat,
        // resume, cron, heartbeat) having to remember to fire `projects.touch`
        // themselves. Best-effort: the run continues even when the store
        // write fails (read-only home dir, etc.).
        if request.workspace_override.is_some() {
            let path = effective_workspace.clone();
            tokio::task::spawn_blocking(move || {
                let store = crate::projects::ProjectStore::new();
                match store.find_by_path(&path) {
                    Ok(Some(project)) => {
                        if let Err(e) = store.touch(&project.id) {
                            tracing::debug!(
                                error = %e,
                                project = %path.display(),
                                "projects.touch failed; recents ordering may be stale"
                            );
                        }
                    }
                    Ok(None) => {
                        // The user ran with a project_root that is not yet in
                        // the catalogue (CLI / programmatic entry). Register
                        // it so the desktop picker shows it next time.
                        if let Err(e) = store.add(&path, None) {
                            tracing::debug!(
                                error = %e,
                                project = %path.display(),
                                "projects.add (auto-register) failed"
                            );
                        }
                    }
                    Err(e) => tracing::debug!(error = %e, "projects.find_by_path failed"),
                }
            });
        }

        // Write workspace-scoped output paths to tool context handle. With a
        // project override active, tool artifacts land under `<project>/output/`
        // — same convention used for agent workspaces, just rooted at the
        // user-picked project.
        if let Some(tc_handle) = self.tool_registry.tool_context_handle() {
            match crate::tools::ToolContext::from_workspace(&effective_workspace) {
                Ok(ctx) => {
                    let mut tc = tc_handle.write().await;
                    *tc = ctx;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to create ToolContext from workspace {}: {}",
                        effective_workspace.display(),
                        e
                    );
                }
            }
        }

        // === Pre-compute values reusable across retry attempts ===

        let default_working_dir = Some(effective_workspace.to_string_lossy().to_string());

        // Resolve soul for prompt building (constant across retries).
        let _ = agent.agent_dir();

        // `extension_manager`, `hook_executor` (snapshotted `BeforeToolCall` /
        // `AfterToolCall` / compaction hooks) and `hook_session_id` are
        // resolved once by `run_agent_loop` and threaded in as parameters.

        // Build tool registry inputs (filtered by agent whitelist).
        let base_allowed_tools: Vec<crate::tool_metadata::UnifiedTool> = self
            .tools
            .iter()
            .filter(|t| !matches!(t.source, crate::tool_metadata::ToolSource::Plugin { .. }))
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
                    .filter(|t| matches!(t.source, crate::tool_metadata::ToolSource::Plugin { .. }))
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
            let skill_whitelist: std::collections::HashSet<&str> = raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
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
                hook_executor.as_deref(),
            )
            .await
        } else {
            build_loop_history(&agent, &request.session_key, &request.input).await
        };
        let compress_elapsed = before_compress.elapsed();
        if !compress_elapsed.is_zero() {
            *deadline.lock().await += compress_elapsed;
        }

        // SessionStart — the first turn of a brand-new session has no prior
        // history. Firing here (not inside `src/harness/`) keeps the dumb
        // loop free of lifecycle logic (R10). Observers only.
        if history.is_empty() {
            if let Some(executor) = hook_executor.as_ref() {
                let ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
                executor
                    .execute_observers(HookEvent::SessionStart, &ctx)
                    .await;
            }
        }

        // UserPromptSubmit — fires once per run, before the first provider
        // call. Interceptors may abort the run (block/deny) or inject
        // `context:` lines which we wrap into the user's prompt as
        // `<system-reminder>` blocks. This is Claude Code's seam for
        // project-scoped prompt augmentation (sprint context, env reminders,
        // policy nudges) without touching the agent loop's logic.
        let mut effective_user_input: String = request.input.clone();
        if let Some(executor) = hook_executor.as_ref() {
            let mut ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
            ctx = ctx.with_tool_input(request.input.clone());
            match executor
                .execute_interceptors(HookEvent::UserPromptSubmit, ctx)
                .await
            {
                Ok((_ctx, hr)) if hr.denied || hr.blocked => {
                    let reason = hr
                        .deny_reason
                        .or(hr.block_reason)
                        .unwrap_or_else(|| "user prompt blocked by hook".to_string());
                    warn!(
                        run_id = run_id,
                        reason = %reason,
                        "UserPromptSubmit hook aborted the run"
                    );
                    return Err(ExecutionError::Failed(format!(
                        "UserPromptSubmit hook aborted the run: {reason}"
                    )));
                }
                Ok((_ctx, hr)) => {
                    if !hr.additional_contexts.is_empty() {
                        let reminders = hr
                            .additional_contexts
                            .iter()
                            .map(|c| {
                                format!("<system-reminder>\n{}\n</system-reminder>", c.trim())
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        effective_user_input =
                            format!("{reminders}\n\n{}", effective_user_input);
                    }
                }
                Err(e) => warn!(run_id = run_id, error = %e, "UserPromptSubmit hook failed"),
            }
        }

        // Project-mode context: when the run is scoped to a user-picked
        // project folder, surface its `AGENTS.md` and `CLAUDE.md` (Claude
        // Code parity) to the model the same way UserPromptSubmit hooks do
        // — as `<system-reminder>` blocks prefixed to the user's prompt.
        // This is intentionally a one-shot inline read rather than a new
        // PromptLayer: project files vary per run, so they cannot live in
        // the cached stable prefix anyway, and the read cost is dwarfed by
        // the LLM call that follows.
        if request.workspace_override.is_some() {
            let blocks = collect_project_context_blocks(&effective_workspace);
            if !blocks.is_empty() {
                let joined = blocks
                    .into_iter()
                    .map(|b| format!("<system-reminder>\n{}\n</system-reminder>", b.trim()))
                    .collect::<Vec<_>>()
                    .join("\n");
                effective_user_input = format!("{joined}\n\n{}", effective_user_input);
            }
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
                    text: effective_user_input.clone(),
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

            // Resolve model with health-aware fallback.
            //
            // When the chat-window model picker stamped a per-turn override,
            // we short-circuit the fallback chain: `Qualified { provider,
            // model }` pins both (matches openclaw's `chatModelOverrides`
            // qualified branch); `Raw { model }` lets the registry pick the
            // provider via its model-name heuristic and still skips fallback
            // (user explicitly chose this model — silent failover would be
            // surprising). When `model_override` is `None`, we fall back to
            // the agent's configured default + its declared fallback chain.
            let resolved = match request.model_override.as_ref() {
                Some(override_) => {
                    use crate::providers::health::ResolvedModel;
                    let provider_name = match override_.provider() {
                        Some(p) => p.to_string(),
                        None => self
                            .provider_registry
                            .get(override_.model())
                            .map(|prov| prov.name().to_string())
                            .unwrap_or_else(|| {
                                self.provider_registry.default_provider().name().to_string()
                            }),
                    };
                    ResolvedModel {
                        provider_name,
                        model: override_.model().to_string(),
                        is_fallback: false,
                        original_model: override_.model().to_string(),
                    }
                }
                None => self
                    .provider_registry
                    .resolve_with_fallback(&agent.config().model, &agent.config().fallback_models)
                    .map_err(|e| ExecutionError::Failed(e.to_string()))?,
            };

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

            // Build FlowRequest. A resumed run carries no fresh input — the
            // session event log already holds the full trajectory. The
            // `ResumeCoordinator` sets `metadata["resume"] = "true"`; the
            // harness bridge then skips seeding and replays the log.
            let flow_input = if request.metadata.get("resume").map(String::as_str) == Some("true") {
                crate::orchestrator::FlowInput::Resume
            } else {
                super::helpers::history_to_flow_input(
                    history.clone(),
                    effective_user_input.clone(),
                )
            };

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
                // Project mode — the workspace override flows into
                // `HarnessRunner::run` and lands on the `RunStarted` marker
                // for resume parity.
                workspace_override: request.workspace_override.clone(),
                // D2: forward the per-run iteration cap override so the
                // harness bridge can apply it as the highest-priority layer
                // in `resolve_max_iterations`.
                max_iterations_override: request.max_iterations_override,
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

/// Build a `HookContext` for an agent/session lifecycle event. Carries the
/// session id plus `RUN_ID` / `AGENT_ID` env vars so command hooks have
/// correlation handles. Lifecycle events have no tool, so the tool fields
/// stay unset.
fn lifecycle_hook_context(session_id: &str, run_id: &str, agent: &AgentInstance) -> HookContext {
    HookContext::new(session_id)
        .with_env("RUN_ID", run_id)
        .with_env("AGENT_ID", agent.id())
}

/// Per-directory project context filenames. `AGENTS.md` is Aleph's native
/// operating manual, `CLAUDE.md` is recognised for Claude Code parity, and
/// the `.claude/CLAUDE.md` / `.aleph/CLAUDE.md` subdir variants let a repo
/// keep its assistant config out of the project root README slot.
const PROJECT_CONTEXT_FILES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    ".claude/CLAUDE.md",
    ".aleph/CLAUDE.md",
];

/// Per-file byte cap for project context injection. Project READMEs and
/// long contributor guides can be huge — bound the prompt impact so a
/// 200 KB CLAUDE.md cannot crowd out the rest of the system prompt.
const PROJECT_CONTEXT_MAX_BYTES: usize = 32 * 1024;

/// Aggregate cap across all walked files + rules combined. Without this a
/// deep monorepo with eight ancestors × four files × 32 KB could push
/// ~1 MB into a single user turn.
const PROJECT_CONTEXT_TOTAL_MAX_BYTES: usize = 128 * 1024;

/// Maximum number of ancestor directories to traverse before stopping.
/// Belt-and-suspenders against pathological filesystem layouts; the `.git`
/// boundary and `$HOME` stops normally trip first.
const PROJECT_CONTEXT_MAX_DEPTH: usize = 8;

/// Read project context files from the active workspace root, walking
/// upward like Claude Code's `claudemd.ts`: each ancestor's `AGENTS.md`,
/// `CLAUDE.md`, `.claude/CLAUDE.md` and `.aleph/CLAUDE.md` are loaded,
/// plus `<project_root>/.claude/rules/*.md` (rules are project-scoped, not
/// walked).
///
/// Order is ancestor-first → project-last so files closer to the project
/// root override the parent's guidance (last-wins for the LLM). Missing
/// or unreadable files are silently skipped.
///
/// Walk stops at the first of:
/// - A directory containing `.git/` (the repo root)
/// - The user's `$HOME` directory (don't walk above it)
/// - The filesystem root
/// - [`PROJECT_CONTEXT_MAX_DEPTH`] hops, whichever comes first
fn collect_project_context_blocks(workspace: &std::path::Path) -> Vec<String> {
    let chain = ancestor_chain(workspace);
    let mut body = String::new();
    let mut total_bytes: usize = 0;
    let header = format!(
        "Active project: `{}`. The following project files describe \
        local conventions, scope, and constraints — treat them as \
        durable context for this conversation. Files are listed from the \
        outermost ancestor (`<dir>/...`) down to the project root; later \
        entries override earlier ones on conflict.",
        workspace.display()
    );

    // Pass 1 — ancestor → project root. Label is the relative-to-workspace
    // name when the file is in the project root, the full absolute path
    // otherwise — keeps the system-reminder readable for the common case
    // while still attributing inherited files to their source dir.
    for dir in &chain {
        let is_project_root = dir == workspace;
        for name in PROJECT_CONTEXT_FILES {
            let candidate = dir.join(name);
            let Some(content) = read_capped_text(&candidate) else {
                continue;
            };
            let label = if is_project_root {
                name.to_string()
            } else {
                candidate.display().to_string()
            };
            append_block(&mut body, &mut total_bytes, &header, &label, &content);
        }
    }

    // Pass 2 — `.claude/rules/*.md` is project-scoped (not walked up).
    // Skip on read errors so a missing rules dir is a no-op.
    let rules_dir = workspace.join(".claude").join("rules");
    if let Ok(entries) = std::fs::read_dir(&rules_dir) {
        let mut rule_files: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|s| s.eq_ignore_ascii_case("md"))
                        .unwrap_or(false)
            })
            .collect();
        rule_files.sort();
        for path in rule_files {
            let Some(content) = read_capped_text(&path) else {
                continue;
            };
            let label = format!(
                ".claude/rules/{}",
                path.file_name().unwrap().to_string_lossy()
            );
            append_block(&mut body, &mut total_bytes, &header, &label, &content);
        }
    }

    if body.is_empty() {
        Vec::new()
    } else {
        vec![body]
    }
}

/// Walk from `start` upward, returning the chain in **ancestor → start**
/// order. Stops at `.git`, `$HOME`, filesystem root, or after
/// [`PROJECT_CONTEXT_MAX_DEPTH`] hops.
fn ancestor_chain(start: &std::path::Path) -> Vec<std::path::PathBuf> {
    let home = dirs::home_dir();
    let home_ref = home.as_deref();
    let mut up = Vec::new();
    let mut cur = start.to_path_buf();
    up.push(cur.clone());
    let mut steps = 0;
    while steps < PROJECT_CONTEXT_MAX_DEPTH {
        // Stop conditions checked AFTER recording the current dir so the
        // boundary dir itself contributes its CLAUDE.md.
        if cur.join(".git").exists() {
            break;
        }
        if home_ref.map(|h| h == cur.as_path()).unwrap_or(false) {
            break;
        }
        let Some(parent) = cur.parent() else { break };
        if parent == cur {
            break;
        }
        cur = parent.to_path_buf();
        up.push(cur.clone());
        steps += 1;
    }
    // Reverse to ancestor-first ordering.
    up.reverse();
    up
}

/// Read a file with UTF-8 truncation at [`PROJECT_CONTEXT_MAX_BYTES`].
/// Returns `None` when the file is missing, unreadable, empty, or
/// whitespace-only — all four are normal "no project context" cases.
fn read_capped_text(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    if content.len() <= PROJECT_CONTEXT_MAX_BYTES {
        return Some(content);
    }
    // Find a char boundary at-or-before the cap to avoid splitting a
    // codepoint mid-bytes. Falls back to 0 in the unreachable case where
    // no boundary exists (every char-boundary at 0 satisfies this).
    let mut end = PROJECT_CONTEXT_MAX_BYTES;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    let mut shortened = content[..end].to_string();
    shortened.push_str("\n\n... [project file truncated for prompt budget]");
    Some(shortened)
}

/// Append `(label, content)` to the in-progress system-reminder body,
/// respecting [`PROJECT_CONTEXT_TOTAL_MAX_BYTES`]. The header is added
/// lazily on the first appended block so an empty project produces an
/// empty block list.
fn append_block(
    body: &mut String,
    total_bytes: &mut usize,
    header: &str,
    label: &str,
    content: &str,
) {
    if *total_bytes >= PROJECT_CONTEXT_TOTAL_MAX_BYTES {
        return;
    }
    if body.is_empty() {
        body.push_str(header);
        body.push_str("\n\n");
        *total_bytes += header.len() + 2;
    }
    let prefix = format!("### {label}\n\n");
    body.push_str(&prefix);
    body.push_str(content.trim_end());
    body.push_str("\n\n");
    *total_bytes += prefix.len() + content.len() + 2;
}

#[cfg(test)]
mod project_context_tests {
    use super::*;
    use tempfile::tempdir;

    /// Mark `dir` as a `.git` boundary so [`ancestor_chain`] halts there.
    /// Tests build their workspaces inside a tempdir and would otherwise
    /// walk up to whichever directory holds the test runner — sometimes a
    /// user's real `~/.aleph/...` layout — and pick up files that pollute
    /// assertions. Calling this on the workspace root keeps the walk
    /// confined to the tempdir.
    fn anchor(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn returns_nothing_when_no_project_files() {
        let dir = tempdir().unwrap();
        anchor(dir.path());
        let blocks = collect_project_context_blocks(dir.path());
        assert!(blocks.is_empty());
    }

    #[test]
    fn reads_agents_md_when_present() {
        let dir = tempdir().unwrap();
        anchor(dir.path());
        std::fs::write(dir.path().join("AGENTS.md"), "# Project rules\nNo force push.\n").unwrap();
        let blocks = collect_project_context_blocks(dir.path());
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("Active project"));
        assert!(blocks[0].contains("### AGENTS.md"));
        assert!(blocks[0].contains("No force push"));
    }

    #[test]
    fn includes_both_agents_and_claude_md_when_both_present() {
        let dir = tempdir().unwrap();
        anchor(dir.path());
        std::fs::write(dir.path().join("AGENTS.md"), "# Aleph rules").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# CC rules").unwrap();
        let blocks = collect_project_context_blocks(dir.path());
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("### AGENTS.md"));
        assert!(blocks[0].contains("### CLAUDE.md"));
    }

    #[test]
    fn ignores_whitespace_only_files() {
        let dir = tempdir().unwrap();
        anchor(dir.path());
        std::fs::write(dir.path().join("AGENTS.md"), "   \n\n\t\n").unwrap();
        let blocks = collect_project_context_blocks(dir.path());
        assert!(blocks.is_empty());
    }

    #[test]
    fn truncates_oversized_files() {
        let dir = tempdir().unwrap();
        anchor(dir.path());
        let big = "x".repeat(PROJECT_CONTEXT_MAX_BYTES + 4096);
        std::fs::write(dir.path().join("AGENTS.md"), &big).unwrap();
        let blocks = collect_project_context_blocks(dir.path());
        assert!(blocks[0].contains("[project file truncated for prompt budget]"));
        assert!(blocks[0].len() < big.len() + 4096);
    }

    #[test]
    fn loads_claude_md_in_subdir() {
        let dir = tempdir().unwrap();
        anchor(dir.path());
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/CLAUDE.md"), "# CC sub").unwrap();
        let blocks = collect_project_context_blocks(dir.path());
        assert!(blocks[0].contains(".claude/CLAUDE.md"));
        assert!(blocks[0].contains("# CC sub"));
    }

    #[test]
    fn loads_aleph_claude_md_in_subdir() {
        let dir = tempdir().unwrap();
        anchor(dir.path());
        std::fs::create_dir_all(dir.path().join(".aleph")).unwrap();
        std::fs::write(dir.path().join(".aleph/CLAUDE.md"), "# Aleph sub").unwrap();
        let blocks = collect_project_context_blocks(dir.path());
        assert!(blocks[0].contains(".aleph/CLAUDE.md"));
        assert!(blocks[0].contains("# Aleph sub"));
    }

    /// Walk-up: parent CLAUDE.md is included, and parent appears BEFORE
    /// the project root's so the LLM reads parent first → project last
    /// (last-wins ordering).
    #[test]
    fn walks_up_to_ancestor_claude_md_until_git_boundary() {
        let root = tempdir().unwrap();
        anchor(root.path()); // `.git` lives on the outer dir (the repo root)
        std::fs::write(root.path().join("CLAUDE.md"), "# outer").unwrap();
        let inner = root.path().join("packages").join("svc");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("CLAUDE.md"), "# inner").unwrap();

        let blocks = collect_project_context_blocks(&inner);
        assert_eq!(blocks.len(), 1);
        let body = &blocks[0];
        let outer_pos = body
            .find("# outer")
            .expect("outer CLAUDE.md must be injected");
        let inner_pos = body
            .find("# inner")
            .expect("inner CLAUDE.md must be injected");
        assert!(
            outer_pos < inner_pos,
            "ancestor must appear before project root so last-wins ordering holds"
        );
    }

    /// Walk-up halts at `.git`: a CLAUDE.md sitting above the boundary is
    /// NOT injected, even if it physically exists on disk.
    #[test]
    fn walk_stops_at_git_boundary() {
        let outer = tempdir().unwrap();
        std::fs::write(outer.path().join("CLAUDE.md"), "# above boundary").unwrap();
        let project = outer.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        anchor(&project); // `.git` lives ON the project root → stops walk there
        std::fs::write(project.join("CLAUDE.md"), "# project").unwrap();

        let blocks = collect_project_context_blocks(&project);
        assert!(blocks[0].contains("# project"));
        assert!(
            !blocks[0].contains("# above boundary"),
            "files above the .git boundary must NOT leak into project context"
        );
    }

    #[test]
    fn loads_claude_rules_glob() {
        let dir = tempdir().unwrap();
        anchor(dir.path());
        let rules = dir.path().join(".claude").join("rules");
        std::fs::create_dir_all(&rules).unwrap();
        std::fs::write(rules.join("a.md"), "rule alpha").unwrap();
        std::fs::write(rules.join("b.md"), "rule beta").unwrap();
        std::fs::write(rules.join("ignored.txt"), "not a rule").unwrap();
        let blocks = collect_project_context_blocks(dir.path());
        assert!(blocks[0].contains("rule alpha"));
        assert!(blocks[0].contains("rule beta"));
        assert!(!blocks[0].contains("not a rule"));
        // a.md should appear before b.md (sort order).
        assert!(blocks[0].find("rule alpha").unwrap() < blocks[0].find("rule beta").unwrap());
    }

    /// Aggregate-size cap: a deep tree with many ancestors and big files
    /// cannot exceed [`PROJECT_CONTEXT_TOTAL_MAX_BYTES`] by more than a
    /// single last-block worth of overshoot.
    #[test]
    fn enforces_total_context_cap() {
        let outer = tempdir().unwrap();
        anchor(outer.path());
        // 7 ancestor dirs each with a 32 KB CLAUDE.md — total raw input
        // would be ~224 KB, well above the 128 KB cap.
        let mut cur = outer.path().to_path_buf();
        for i in 0..7 {
            cur = cur.join(format!("lvl{i}"));
            std::fs::create_dir_all(&cur).unwrap();
            std::fs::write(
                cur.join("CLAUDE.md"),
                "x".repeat(PROJECT_CONTEXT_MAX_BYTES),
            )
            .unwrap();
        }
        let blocks = collect_project_context_blocks(&cur);
        // Overshoot is bounded by one block (~32 KB) + a small header.
        let allowed = PROJECT_CONTEXT_TOTAL_MAX_BYTES + PROJECT_CONTEXT_MAX_BYTES + 1024;
        assert!(
            blocks[0].len() <= allowed,
            "context body {} exceeds allowed budget {}",
            blocks[0].len(),
            allowed
        );
    }
}
