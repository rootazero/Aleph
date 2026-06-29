//! Inner think→act loop body (`run_agent_loop_inner`).
//!
//! Carved verbatim from the original `run_loop.rs`; behavior unchanged.
//! `run_agent_loop` (in the parent module) wraps this with the
//! `BeforeAgentStart` / `AgentEnd` lifecycle hooks.

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::sync_primitives::Arc;

use super::super::{ExecutionError, RunRequest};
use crate::extension::hooks::HookExecutor;
use crate::extension::HookEvent;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::super::engine::ExecutionEngine;

// Submodule helpers used by the loop body.
use super::super::callback::{CallbackStateFlushHandle, StreamCallbackState, TracePersistence};
use super::super::history::build_loop_history;
use super::super::tool_refresh::{active_plugin_tools_for_agent, ExtensionToolRefreshSource};

// Free helpers carved into the sibling project_context module.
use super::project_context::{
    collect_project_context_blocks, collect_project_skill_block, lifecycle_hook_context,
    workspace_directive,
};

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Inner body of the think→act loop. `run_agent_loop` wraps this with the
    /// `BeforeAgentStart` / `AgentEnd` lifecycle hooks and supplies the
    /// pre-resolved extension manager + hook executor snapshot.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_agent_loop_inner<E: EventEmitter + Send + Sync + 'static>(
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
        // Skip catalogue writes for team-worktree runs: their override points
        // at an ephemeral `$TMPDIR` checkout that would otherwise pollute the
        // Panel's recent-projects list with paths that vanish minutes later.
        if request.workspace_override.is_some()
            && !request.metadata.contains_key("team_worktree_path")
        {
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

        // Effective tool permission policy for this turn: global
        // `[policies.tool_permissions]` (engine-held boot snapshot) merged with
        // the agent's override, then with the originating channel's override
        // (stamped into metadata by the inbound router — absent for Panel /
        // CLI / cron turns). Most restrictive wins at every layer. `None` when
        // everything is all-default so the ScopedToolService hot path stays a
        // no-op — byte-identical to pre-wiring behavior for unconfigured
        // installs.
        let tool_permissions = {
            use crate::config::types::policies::ToolPermissionsConfig;
            let mut merged = ToolPermissionsConfig::merge(
                &self.global_tool_permissions,
                &agent.config().tool_permissions(),
            );
            if let Some(raw) = request
                .metadata
                .get(super::super::CHANNEL_TOOL_PERMISSIONS_KEY)
            {
                match serde_json::from_str::<ToolPermissionsConfig>(raw) {
                    Ok(channel_perms) => {
                        merged = ToolPermissionsConfig::merge(&merged, &channel_perms)
                    }
                    Err(e) => warn!(
                        run_id = run_id,
                        error = %e,
                        "Malformed channel tool_permissions metadata — channel layer skipped"
                    ),
                }
            }
            let is_all_default = merged.default == crate::extension::PermissionAction::Allow
                && merged.overrides.is_empty();
            (!is_all_default).then(|| {
                info!(
                    run_id = run_id,
                    default = ?merged.default,
                    overrides = merged.overrides.len(),
                    "Tool permission policy active for this turn"
                );
                merged
            })
        };
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
                    // `prevent_continuation` (Claude-Code `continue: false`):
                    // the hook handled this prompt out-of-band and the agent
                    // must not run. Stop gracefully and surface the hook's
                    // message/context as the run output (NOT an error).
                    if hr.prevent_continuation {
                        let stop_msg = hr
                            .messages
                            .first()
                            .cloned()
                            .or_else(|| hr.additional_contexts.first().cloned())
                            .unwrap_or_else(|| {
                                "Run halted by UserPromptSubmit hook (prevent_continuation)."
                                    .to_string()
                            });
                        warn!(
                            run_id = run_id,
                            "UserPromptSubmit hook requested prevent_continuation; stopping run"
                        );
                        return Ok(stop_msg);
                    }
                    if !hr.additional_contexts.is_empty() {
                        let reminders = hr
                            .additional_contexts
                            .iter()
                            .map(|c| format!("<system-reminder>\n{}\n</system-reminder>", c.trim()))
                            .collect::<Vec<_>>()
                            .join("\n");
                        effective_user_input = format!("{reminders}\n\n{effective_user_input}");
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
            let mut blocks = collect_project_context_blocks(&effective_workspace);
            // Round 3: advertise the project's own skills so the model knows
            // to `skill_read` them (the tool itself is project-aware now).
            if let Some(skills_block) = collect_project_skill_block(&effective_workspace) {
                blocks.push(skills_block);
            }
            if !blocks.is_empty() {
                let joined = blocks
                    .into_iter()
                    .map(|b| format!("<system-reminder>\n{}\n</system-reminder>", b.trim()))
                    .collect::<Vec<_>>()
                    .join("\n");
                effective_user_input = format!("{joined}\n\n{effective_user_input}");
            }
        }

        // Always surface the effective working directory to the model — for
        // both default `~/.aleph/workspaces/{agent_id}` runs and project-scoped
        // runs. The path-selection logic above already picks the right
        // directory; this is the missing half (R7/R9): without it the model
        // has no way to know where it is and, when asked to save a file,
        // invents an arbitrary absolute path under the user's home instead of
        // writing into its workspace. Prepended last so it sits outermost.
        effective_user_input = format!(
            "<system-reminder>\n{}\n</system-reminder>\n\n{}",
            workspace_directive(&effective_workspace),
            effective_user_input
        );

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
            run_id: run_id.to_string(),
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
            caller_role: request.metadata.get("caller_role").cloned(),
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
                        None => self.provider_registry.get(override_.model()).map_or_else(
                            || self.provider_registry.default_provider().name().to_string(),
                            |prov| prov.name().to_string(),
                        ),
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

            // Build per-request ToolService with SubagentTool + optional MCP refresh
            let mut loop_registry_inner = crate::tools::adapters::build_registry_from_tools(
                self.tool_registry.clone(),
                &allowed_tools,
                default_working_dir.clone(),
            );

            let mut allowed_names: std::collections::BTreeSet<String> =
                allowed_tools.iter().map(|t| t.name.clone()).collect();

            let unattended = request.metadata.get("unattended").map(String::as_str) == Some("true");

            // External MCP tools: snapshot the bridge-maintained registry
            // (boot installs it via `set_mcp_tool_registry`) and join each
            // entry into this request's LoopToolRegistry. This is the
            // consumer side of `mcp::spawn_tool_bridge` — the bridge keeps
            // the ToolHandlerRegistry in sync with every connected server's
            // tools/list; here those handlers become LLM-visible LoopTools.
            // Gating mirrors the builtin path above: per-agent allowlist,
            // plus the slash-skill whitelist when one restricted this run
            // (re-parsed from metadata — see the restriction block above).
            // Existing names are never overwritten (builtins win collisions).
            if let Some(mcp_registry) = super::super::tool_service_builder::mcp_tool_registry() {
                let skill_whitelist: Option<std::collections::HashSet<&str>> = request
                    .metadata
                    .get("slash_skill_allowed_tools")
                    .map(|raw| {
                        raw.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect::<std::collections::HashSet<&str>>()
                    })
                    .filter(|set| !set.is_empty());
                let mut joined = 0usize;
                for (name, handler) in mcp_registry.snapshot().iter() {
                    if !agent.is_tool_allowed(name) {
                        continue;
                    }
                    if let Some(ref wl) = skill_whitelist {
                        if !wl.contains(name.as_str()) {
                            continue;
                        }
                    }
                    if loop_registry_inner.get(name).is_some() {
                        continue;
                    }
                    loop_registry_inner.register(Box::new(
                        crate::tools::adapters::McpRegistryTool::from_registry_entry(
                            name,
                            Arc::clone(handler),
                        ),
                    ));
                    // Only widen a non-empty allow-set; an empty set already
                    // means allow-all in ScopedToolService and inserting
                    // names would flip it to restrictive.
                    if !allowed_names.is_empty() {
                        allowed_names.insert(name.clone());
                    }
                    joined += 1;
                }
                if joined > 0 {
                    info!(
                        run_id = run_id,
                        count = joined,
                        "MCP tools joined tool surface"
                    );
                }
            }

            let loop_registry = Arc::new(loop_registry_inner);

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
                super::super::markdown_skill_refresh::MarkdownSkillRefreshSource::new(),
            )
                as Arc<dyn crate::tools::refresh::ToolRefreshSource>);
            let tool_refresh: Option<Arc<dyn crate::tools::refresh::ToolRefreshSource>> = Some(
                Arc::new(crate::tools::refresh::CompositeRefreshSource::new(
                    refresh_sources,
                )) as Arc<dyn crate::tools::refresh::ToolRefreshSource>,
            );

            // Build parent view ToolService WITHOUT the subagent tool
            let parent_view_for_children: Arc<dyn crate::tools::service::ToolService> =
                super::super::build_request_tool_service(
                    loop_registry.clone(),
                    allowed_names.clone(),
                    None,
                    tool_refresh.clone(),
                    Some(turn_context.clone()),
                    hook_executor.clone(),
                    hook_session_id.clone(),
                    tool_permissions.clone(),
                    unattended,
                );

            // Trace sink — built before SubagentTool so it can be inherited by
            // runtime-spawned subagents (B3): their run events flow into the
            // same gateway sink as the main runner, and the background path's
            // ForwardingTraceSink populates check_status progress.
            let trace_sink: Arc<dyn crate::harness::TraceSink> =
                Arc::new(super::super::GatewayTraceSink::new(Arc::new(
                    CallbackStateFlushHandle::new(callback_state.clone()),
                )));

            // R5 progress side-channel: when enabled and this run is bound to a
            // user channel, wrap the gateway sink so scratchpad progress +
            // watchdog-boundary events mirror to that channel (Telegram /
            // Panel / …). Pure I/O bypass; falls back to the inner sink when
            // the channel registry isn't up yet or no channel is bound.
            let trace_sink: Arc<dyn crate::harness::TraceSink> =
                if self.config.scratchpad_progress_push && !turn_context.channel_id.is_empty() {
                    match self.channel_registry.get() {
                        Some(registry) => Arc::new(super::super::ScratchpadProgressSink::new(
                            trace_sink,
                            registry.clone(),
                            turn_context.channel_id.clone(),
                            turn_context.conversation_id.clone(),
                        )),
                        None => trace_sink,
                    }
                } else {
                    trace_sink
                };

            // Forward the harness trace stream to this run's WebSocket as
            // `agent_trace` notifications so the WebChat Panel can segment the
            // chat per Think→Act step and populate the workspace timeline.
            // Outermost wrap: it sees every event and forwards to the inner
            // (persistence + scratchpad) sink unchanged.
            let trace_sink: Arc<dyn crate::harness::TraceSink> =
                Arc::new(super::super::AgentTraceEmitSink::new(
                    trace_sink,
                    emitter.clone() as Arc<dyn crate::gateway::event_emitter::EventEmitter>,
                    run_id.to_string(),
                ));

            // Unattended security-tax (observability side): when no human is
            // watching, redact model-authored text before it reaches
            // persistence / the channel push / the WebSocket. Outermost wrap so
            // it sees every event first; attended runs are never wrapped.
            let trace_sink: Arc<dyn crate::harness::TraceSink> = if unattended {
                Arc::new(super::super::UnattendedRedactingSink::new(trace_sink))
            } else {
                trace_sink
            };

            // SubagentTool construction
            let subagent_tool = {
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
                // Engine-lifetime tracker: background sub-agents must stay
                // pollable (`check_status` / `list`) and cancellable on turns
                // AFTER the one that spawned them. A per-request tracker here
                // silently dropped every result once the spawning run returned.
                let background_tracker = self.background_tracker.clone();
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
                // Stage 5a (#9) — inherit the main harness's guardrail
                // registry so spawned subagents enforce the same
                // Input/Output/ToolCall checks. `None` (mocks / simple
                // engine / no [guardrails] config) leaves subagents
                // unguarded, matching the main harness.
                if let Some(g) = orchestrator.harness.guardrails() {
                    t = t.with_guardrails(g);
                }
                // VESR v1.1 (b) — thread the routing store so spawned subagents
                // capture their own routing experience under their agent_id.
                if let Some(rs) = orchestrator.harness.routing_store() {
                    t = t.with_routing_store(rs);
                }
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
                // Weld the run-global Strategy into every spawned subagent's
                // inline prompt (StraTA spec §7). A subagent is a downstream
                // execution node, so it must carry the same system-level
                // Strategy the parent run's prompt does. Mirror the main
                // harness's goal-then-loop precedence (`active_strategy`) and
                // the exact body `StrategyLayer` emits (`render_strategy_summary`,
                // the same string fed into `ResolvedContext.strategy`), so the
                // child's inline `<strategy>` weld is byte-identical to the
                // parent's cached layer. Fail-soft: no Strategy → no call → the
                // child prompt stays byte-identical to the pre-feature baseline.
                if let Some(s) = crate::orchestrator::harness_bridge::active_strategy(
                    &request.session_key.to_key_string(),
                )
                .await
                {
                    t = t.with_strategy(crate::strategy::render_strategy_summary(&s));
                }
                Arc::new(t)
            };

            let tool_service = super::super::build_request_tool_service(
                loop_registry,
                allowed_names,
                Some(subagent_tool),
                tool_refresh,
                Some(turn_context.clone()),
                hook_executor.clone(),
                hook_session_id.clone(),
                tool_permissions.clone(),
                unattended,
            );

            // Build FlowRequest. A resumed run carries no fresh input — the
            // session event log already holds the full trajectory. The
            // `ResumeCoordinator` sets `metadata["resume"] = "true"`; the
            // harness bridge then skips seeding and replays the log.
            let flow_input = if request.metadata.get("resume").map(String::as_str) == Some("true") {
                crate::orchestrator::FlowInput::Resume
            } else {
                super::super::helpers::history_to_flow_input(
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

            let dispatch_result = super::super::helpers::run_dispatch_and_drain_classified(
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
                Err(super::super::helpers::DispatchFailure::Cancelled) => {
                    if multimodal_messages.is_some() {
                        if let Some(mp) = self.media_processor.as_ref() {
                            mp.cleanup(&request.session_key.to_key_string());
                        }
                    }
                    return Err(ExecutionError::Cancelled);
                }
                Err(super::super::helpers::DispatchFailure::Transient {
                    provider: prov_name,
                    message,
                }) if attempt < MAX_FALLBACK_ATTEMPTS => {
                    self.provider_registry.report_outcome(
                        &resolved.provider_name,
                        Err(crate::providers::health::ProviderError::Transient(
                            crate::providers::health::TransientError::ConnectionFailed,
                        )),
                    );
                    // Reactive OAuth self-heal: a `token_expired` 401 means the
                    // Codex/ChatGPT access token lapsed mid-run. Refresh it and
                    // hot-swap the live provider so the retry below uses a fresh
                    // token instead of hammering the same stale one. Infra-level
                    // (limb refreshing its credential), not a loop strategy
                    // decision — the retry happens regardless.
                    if crate::gateway::codex_token_refresher::is_oauth_token_expired_error(&message)
                    {
                        if let Some(refresher) = crate::gateway::codex_token_refresher::global() {
                            let healed = refresher.refresh_and_swap().await;
                            info!(
                                run_id = run_id,
                                attempt = attempt,
                                healed = healed,
                                "OAuth token expired mid-run — attempted refresh before retry"
                            );
                        }
                    }
                    warn!(
                        run_id = run_id,
                        attempt = attempt,
                        failed_provider = %prov_name,
                        message = %message,
                        "Provider transient failure via Orchestrator::dispatch — retrying"
                    );
                    // Surface the retry to the user — without this, a full
                    // provider outage looks like minutes of silent "thinking"
                    // before the run finally errors out.
                    let reason: String = message.chars().take(200).collect();
                    emitter
                        .emit_run_retrying(
                            run_id,
                            &prov_name,
                            (attempt + 1) as u32,
                            MAX_FALLBACK_ATTEMPTS as u32,
                            &reason,
                        )
                        .await;
                    continue;
                }
                Err(super::super::helpers::DispatchFailure::Transient {
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
                Err(super::super::helpers::DispatchFailure::Fatal(msg)) => {
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
