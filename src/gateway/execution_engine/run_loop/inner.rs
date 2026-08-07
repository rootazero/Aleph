//! Inner think→act loop body (`run_agent_loop_inner`).
//!
//! Carved verbatim from the original `run_loop.rs`; behavior unchanged.
//! `run_agent_loop` (in the parent module) wraps this with the
//! `BeforeAgentStart` / `AgentEnd` lifecycle hooks.

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::sync_primitives::Arc;

use super::super::{ExecutionError, RunRequest};
use crate::extension::hooks::{budget_hook_contexts, join_messages, HookExecutor};
use crate::extension::HookEvent;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::super::engine::ExecutionEngine;

// Submodule helpers used by the loop body.
use super::super::callback::{CallbackStateFlushHandle, StreamCallbackState, TracePersistence};
use super::super::history::build_loop_history;
use super::super::tool_refresh::active_plugin_tools_for_agent;

// Free helpers carved into the sibling project_context module.
use super::project_context::{
    collect_project_skill_block, lifecycle_hook_context, workspace_directive,
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
        occupancy_out: Arc<std::sync::Mutex<Option<super::super::helpers::RunContextOccupancy>>>,
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
        //
        // A project-room run takes the short branch below instead: since P2
        // Task 7 the room's own `workspace_path` IS the override, so the
        // catalogue row already exists and is addressed by id. Letting it fall
        // into the path lookup would ask an owner-keyed index about the
        // SPEAKER (`ambient_owner()` is whoever typed, not the room's owner),
        // miss, and auto-register a duplicate personal row for the shared
        // folder in every member's recents.
        //
        // Unbound rooms take this branch too, which is new and deliberate: a
        // room with no folder still belongs in the sidebar's recency order,
        // and by id there is nothing to look up by path anyway.
        let room_id = crate::scope::scope_from_metadata(&request.metadata).and_then(|attr| {
            match attr.scope {
                crate::scope::ScopeId::Project(id) => Some(id),
                _ => None,
            }
        });
        if let Some(project_id) = room_id {
            tokio::task::spawn_blocking(move || {
                let store = crate::projects::ProjectStore::shared();
                if let Err(e) = store.touch(&project_id) {
                    tracing::debug!(
                        error = %e,
                        project = %project_id,
                        "projects.touch failed; room recency may be stale"
                    );
                }
            });
        } else if request.workspace_override.is_some()
            && !request.metadata.contains_key("team_worktree_path")
        {
            let path = effective_workspace.clone();
            // Capture the owner HERE, before the blocking boundary: both
            // ambient-attribution mechanisms are task-locals and are dead
            // inside `spawn_blocking`, so letting the store resolve it there
            // would silently file this user's folder into the legacy owner's
            // recents (and hand them back the owner's row).
            let owner = crate::scope::ambient_owner();
            tokio::task::spawn_blocking(move || {
                let store = crate::projects::ProjectStore::shared();
                match store.find_by_path_for(&path, owner.as_deref()) {
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
                        if let Err(e) = store.add_for(&path, None, owner.as_deref()) {
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

        // Deliberately NO per-run write to the shared `ToolContextHandle`. That
        // mutation was the last cross-run contamination vector: two concurrent
        // runs with different workspaces clobbered each other's handle, and an
        // FsScope-absent reader then saw whichever run wrote last. The per-run
        // `FsScope` published in `run_agent_loop` (see `run_loop/mod.rs`) already
        // carries THIS run's workspace artifact dir — derived from the same
        // `override > agent workspace` fallback as `effective_workspace` — and is
        // *preferred* over the handle at the resolution chokepoint
        // (`expand_input_path`). The write therefore only ever affected the
        // FsScope-absent fallback, where the deterministic construction-time
        // default is safer than a last-writer race. The handle stays as that
        // read-only fallback; runs no longer mutate it. (`effective_workspace`
        // still feeds `default_working_dir` below.)

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

        // This turn's execution tier + explicit tool permission policy. Resolved
        // by the shared `turn_permissions` module: the slash-command fast path
        // consults the same resolution before it is allowed to dispatch, so the
        // two surfaces cannot enforce different rules.
        let (exec_tier, tool_permissions) = self.resolve_turn_permissions(request, &agent).await;
        // This turn's reasoning depth (composer pill / RPC `thinking` param /
        // the model's own `self_config` call, else whatever the session was
        // last stamped with). `None` = send no thinking directive, which is the
        // behaviour every release before this one shipped.
        let think_level = self.resolve_turn_think_level(request).await;
        // This turn's usage mode (composer mode pill / RPC `mode` param / the
        // model's own `session_set_mode` call, else the session's stamped mode,
        // else `[policies] mode`). Partitions the tool PRESENTATION surface
        // only — schema-resident core set + deferred families — never
        // permissions; `Work` is the identity partition.
        let session_mode = self.resolve_turn_mode(request).await;
        // Mode-effective schema-resident core set, fed to both
        // `build_request_tool_service` call sites and the SchemaLookupTool
        // registration gate below. Work returns the configured set unchanged.
        let mut mode_core_tools = session_mode.effective_core_tools(&self.config.core_tools);
        let _max_loops = agent.config().max_loops as usize;
        let token_budget = agent.config().max_tokens.unwrap_or(500_000);

        // Start this session's route witness clean. A run that ends in an error
        // never reaches the taker in `run_dispatch_and_drain_classified`, and an
        // inherited record would make the *next* run in the same session
        // announce a migration that belonged to the previous one.
        crate::providers::route_witness::clear(&request.session_key.to_key_string());

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
        // loop free of lifecycle logic (R10). Observers run fire-and-forget;
        // interceptor-kind hooks are harvested for context (Claude Code /
        // codex parity: a SessionStart hook's stdout is injected into the
        // session context). SessionStart is deliberately non-blocking —
        // block/deny from a hook here is ignored, matching Claude Code.
        let mut session_start_blocks: Vec<String> = Vec::new();
        if history.is_empty() {
            if let Some(executor) = hook_executor.as_ref() {
                let ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
                executor
                    .execute_observers(HookEvent::SessionStart, &ctx)
                    .await;
                match executor
                    .execute_interceptors(HookEvent::SessionStart, ctx)
                    .await
                {
                    Ok((_ctx, hr)) => {
                        // Claude-Code convention: `context:` lines / JSON
                        // additionalContext AND plain stdout lines both count
                        // as injected context on this event. Everything this
                        // seam injects rides in the session context for the
                        // REST OF THE SESSION, so it goes through the shared
                        // hook-context budget — over-budget blocks spill to
                        // disk and are replaced by a recoverable preview.
                        let mut blocks = hr.additional_contexts;
                        blocks.extend(join_messages(&hr.messages));
                        session_start_blocks
                            .extend(budget_hook_contexts(&hook_session_id, blocks).await);
                    }
                    Err(e) => {
                        warn!(run_id = run_id, error = %e, "SessionStart hook failed")
                    }
                }
            }
        }

        // UserPromptSubmit — fires once per run, before the first provider
        // call. Interceptors may abort the run (block/deny) or inject
        // `context:` lines which we wrap into the user's prompt as
        // `<system-reminder>` blocks. This is Claude Code's seam for
        // project-scoped prompt augmentation (sprint context, env reminders,
        // policy nudges) without touching the agent loop's logic.
        // The user's message is persisted verbatim as the session's user turn;
        // the per-turn `<system-reminder>` augmentations below are collected
        // separately into `transient_blocks` and delivered to the model as the
        // non-persisted transient trailing recall message (via
        // `FlowRequest.transient_context` → `HarnessDeps::recall_context`). This
        // keeps the stored user message — and the session title derived from its
        // first-user-message content — equal to the raw input.
        let effective_user_input: String = request.input.clone();
        let mut transient_blocks: Vec<String> = Vec::new();
        for c in &session_start_blocks {
            transient_blocks.push(format!(
                "<system-reminder>\n{}\n</system-reminder>",
                c.trim()
            ));
        }
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
                        let stop_msg = hr.stop_message(
                            "Run halted by UserPromptSubmit hook (prevent_continuation).",
                        );
                        warn!(
                            run_id = run_id,
                            "UserPromptSubmit hook requested prevent_continuation; stopping run"
                        );
                        return Ok(stop_msg);
                    }
                    // Claude-Code convention for UserPromptSubmit: PLAIN
                    // stdout is injected as context, not just `context:`
                    // lines / JSON additionalContext. Without the `messages`
                    // hop a CC-ecosystem hook that simply prints
                    // "Current sprint: 42" silently did nothing here. Both
                    // kinds then share one budget (`budget_hook_contexts`)
                    // rather than the old split where only plain stdout was
                    // capped and `context:` lines were unbounded.
                    let mut blocks = hr.additional_contexts;
                    blocks.extend(join_messages(&hr.messages));
                    for c in budget_hook_contexts(&hook_session_id, blocks).await {
                        transient_blocks.push(format!(
                            "<system-reminder>\n{}\n</system-reminder>",
                            c.trim()
                        ));
                    }
                }
                Err(e) => warn!(run_id = run_id, error = %e, "UserPromptSubmit hook failed"),
            }
        }

        // Project-mode context: advertise the project's own skills so the model
        // knows to `skill_read` them (the tool itself is project-aware).
        //
        // The project's `AGENTS.md` / `CLAUDE.md` / rules are NOT pushed here.
        // They used to be — and the orchestrator ALSO loads the exact same set
        // through the exact same discovery function
        // (`prompt_build.rs` → `project_instructions::load_project_instructions`,
        // gated on the same `workspace_override.is_some()`), so every turn of a
        // project session shipped the whole file set TWICE: once sanitized and
        // budgeted inside `ExtraFilesLayer`, once verbatim here. On a repo with a
        // 30 KB `CLAUDE.md` that is ~15k wasted tokens per turn, and the copy that
        // skipped the sanitizer was the one framed as durable context. The
        // orchestrator presenter is the surviving owner: it sanitizes, it counts
        // against the prompt budget, and it shows up in `aleph-server prompt-size`.
        if request.workspace_override.is_some() {
            if let Some(skills_block) = collect_project_skill_block(&effective_workspace) {
                transient_blocks.push(format!(
                    "<system-reminder>\n{}\n</system-reminder>",
                    skills_block.trim()
                ));
            }
        }

        // Always remind the model to write into its working directory — for both
        // default `~/.aleph/workspaces/{agent_id}` runs and project-scoped runs.
        // Without it the model, asked to save a file, invents an arbitrary
        // absolute path under the user's home instead of writing into its
        // workspace. The *path itself* is not repeated here: the system prompt's
        // `## Runtime Environment` line states it once as `cwd=` (fed by the same
        // `effective_workspace`), so this carries only the behavioural half.
        // Delivered as a transient trailing reminder (never baked into the
        // persisted user message) so the stored message — and the session title
        // derived from it — stays equal to the raw input.
        transient_blocks.push(format!(
            "<system-reminder>\n{}\n</system-reminder>",
            workspace_directive()
        ));

        // Join the per-turn reminders into the ephemeral trailing context the
        // harness bridge appends to the LLM prompt (merged into
        // `HarnessDeps::recall_context`) and never persists. `None` when no
        // reminder applies keeps the pre-feature prompt byte-identical.
        let transient_context =
            (!transient_blocks.is_empty()).then(|| transient_blocks.join("\n\n"));

        // Pre-process attachments into the content blocks that ride on this
        // turn's user message (constant across retries). Serialized
        // `ContentBlock`s are persisted on the seeded `UserMessage`'s
        // `MessageContent.blocks` (via `FlowInput::Multimodal` below) and
        // rebuilt into the provider message by `harness::agent::prompt` — that
        // is the only path by which an image actually reaches the model.
        // Empty when the turn carries no attachments; also the media-cache
        // cleanup gate on every terminal branch of the retry loop below.
        let media_blocks: Vec<serde_json::Value> = if let (false, Some(media_processor)) = (
            request.attachments.is_empty(),
            self.media_processor.as_ref(),
        ) {
            // Whether the model serving this turn actually accepts inline
            // images decides HOW an attachment rides: a native
            // `ContentBlock::Image` for a vision model, a `VisionPipeline` text
            // description for a text-only one. Model id precedence mirrors the
            // retry loop's resolution below (per-turn picker override ▸ agent
            // default); when that id is absent from the capability catalogue we
            // ask the live provider chain which model it would actually serve
            // (`serving_model_hint`) before giving up. The lookup is a static
            // table read, never a judgement about the message (R7).
            // Same precedence the binder applies (per-turn pick ▸ session
            // `select_model` pick ▸ agent default). Asking a *different* model
            // whether it accepts inline images than the one that will serve the
            // turn is how a vision-capable pick still gets a text description of
            // the attachment — or worse, how a text-only one gets a raw image
            // block it cannot read.
            let session_pick = crate::providers::session_model_handle::get_session_model(
                &request.session_key.to_key_string(),
            );
            let configured_model = request.model_override.as_ref().map_or_else(
                || {
                    session_pick
                        .as_ref()
                        .map_or(agent.config().model.as_str(), |p| p.model.as_str())
                },
                |o| o.model(),
            );
            let serving_hint: Option<String> =
                if crate::providers::capabilities_for(configured_model).is_some() {
                    None
                } else {
                    self.provider_registry
                        .get(configured_model)
                        .and_then(|p| p.serving_model_hint().map(std::borrow::Cow::into_owned))
                };
            let supports_vision = model_supports_vision(configured_model, serving_hint.as_deref());

            // Archive the attachments before they are turned into content
            // blocks. The media cache writes under the OS temp dir and every
            // terminal branch of the retry loop below `remove_dir_all`s it, so
            // the cache is not a record of what the session received — the
            // artifact store, rooted in the Aleph data directory, is.
            archive_inbound_attachments(
                &request.attachments,
                &request.session_key.to_key_string(),
                run_id,
                self.event_bus.as_deref(),
            )
            .await;

            let blocks = media_processor
                .process(
                    &request.attachments,
                    supports_vision,
                    &request.session_key.to_key_string(),
                    run_id,
                )
                .await;

            let has_images = blocks
                .iter()
                .any(|b| matches!(b, crate::providers::message::ContentBlock::Image { .. }));
            let has_transcripts = blocks.iter().any(|b| {
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
                content_blocks = blocks.len(),
                has_images = has_images,
                has_transcripts = has_transcripts,
                supports_vision = supports_vision,
                model = %configured_model,
                "Multimodal content blocks built"
            );

            blocks
                .iter()
                .filter_map(|b| serde_json::to_value(b).ok())
                .collect()
        } else {
            Vec::new()
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

        // Whether this run is unattended (autonomous continuation / headless
        // producer). One read, three consumers: the turn context below (so
        // delegation tools can propagate it), `ScopedToolService`'s fail-closed
        // confirm gate, and the redacting trace sink.
        let unattended = request
            .metadata
            .get(crate::gateway::execution_engine::UNATTENDED_KEY)
            .map(String::as_str)
            == Some("true");

        // Unattended security tax, emitter leg. `UnattendedRedactingSink` below
        // covers the TraceSink leg only — persistence, the scratchpad channel
        // push, the `agent_trace` WS mirror. Everything a run emits as
        // `ResponseChunk` / `ToolStart` / `ToolEnd` / `RunComplete` leaves down
        // THIS leg instead, and `OriginFanoutEmitter` hands
        // `summary.final_response` straight to the bound channel with only
        // `sanitize_final_response` (a `<think>` stripper, not a masker) in the
        // way. So the same final text used to be masked on the trace leg and
        // delivered in the clear to Telegram in the same run.
        //
        // Shadowing here, before every downstream use, is the point: wrapping
        // at one of the two hand-off sites would leave the other unmasked, and
        // the incoming `emitter` is already `OriginFanoutEmitter`-wrapped, so
        // this lands OUTSIDE the fanout — masked bytes reach the channel too.
        // Attended runs keep the original emitter and pay nothing.
        let emitter: Arc<dyn EventEmitter> = if unattended {
            Arc::new(crate::gateway::event_emitter::RedactingEmitter::new(
                emitter,
            ))
        } else {
            emitter
        };

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
            channel_tool_permissions: request
                .metadata
                .get(crate::gateway::execution_engine::CHANNEL_TOOL_PERMISSIONS_KEY)
                .cloned(),
            unattended,
        };

        loop {
            attempt += 1;

            // This turn's explicit model pick, in the shape the harness binder
            // consumes. `Qualified { provider, model }` pins both; `Raw
            // { model }` pins only the model and lets the provider chain stand
            // (the failover chain's own primary is a better answer than a
            // name-prefix guess, and it keeps breaker/route-mode gating).
            //
            // This is the field that makes the pick REAL: everything below
            // (and the `ModelResolved` event) only ever described it.
            //
            // A blank model is dropped rather than pinned: the field is now
            // load-bearing, and stamping `""` onto the chain would turn a
            // malformed client payload into a provider error. (The `[voice]`
            // path already guards this in `ModelOverride::from_voice`.)
            let turn_model = request
                .model_override
                .as_ref()
                .filter(|o| !o.model().trim().is_empty())
                .map(
                    |o| crate::providers::session_model_handle::SessionModelPref {
                        provider: o.provider().map(ToString::to_string),
                        model: o.model().trim().to_string(),
                    },
                );

            // The model this turn ASKS FOR, in the same precedence the harness
            // binder applies: per-turn pick ▸ session `select_model` pick ▸ the
            // agent's configured default. Nothing has been dialed yet, so this
            // is a statement of intent and nothing more — `is_fallback` is
            // therefore always false here.
            //
            // It used to be a *prediction*: a health table on
            // `MultiProviderRegistry` picked a candidate from its own failure
            // record and stamped `is_fallback` from it. That table decided no
            // dial (the walk in `FailoverProvider` does, from its own breaker,
            // route mode, load ordering and rate windows), and the failures it
            // recorded were filed against the predicted provider rather than the
            // one that actually failed — so a real migration lit nothing while a
            // table that had merely seen errors could announce a provider that
            // was never tried. The table is gone. The honest half of the notice
            // now arrives from the walk itself, as a correction emitted just
            // before the terminal frame (`helpers::emit_route_correction`).
            let (requested_provider, requested_model) = match request.model_override.as_ref() {
                Some(override_) => {
                    let provider_name = match override_.provider() {
                        Some(p) => p.to_string(),
                        None => self.provider_registry.get(override_.model()).map_or_else(
                            || self.provider_registry.default_provider().name().to_string(),
                            |prov| prov.name().to_string(),
                        ),
                    };
                    (provider_name, override_.model().to_string())
                }
                // A `select_model` pick recorded for this session outranks the
                // agent's configured model at the binder, so the banner has to
                // say so too — otherwise the run the user is watching is
                // announced under the model it replaced. Same global, same
                // canonical key the binder reads.
                None => match crate::providers::session_model_handle::get_session_model(
                    &request.session_key.to_key_string(),
                ) {
                    Some(pref) => (
                        pref.provider.unwrap_or_else(|| {
                            self.provider_registry.default_provider().name().to_string()
                        }),
                        pref.model,
                    ),
                    None => {
                        let model = agent.config().model.clone();
                        let provider = self.provider_registry.get(&model).map_or_else(
                            || self.provider_registry.default_provider().name().to_string(),
                            |prov| prov.name().to_string(),
                        );
                        (provider, model)
                    }
                },
            };

            // Emit ModelResolved so the Panel can label the run while it streams.
            let _ = emitter
                .emit(StreamEvent::ModelResolved {
                    run_id: run_id.to_string(),
                    model_info: crate::providers::health::ModelInfo {
                        model: requested_model,
                        provider: requested_provider,
                        is_fallback: false,
                        original_model: None,
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

            // Collector for MCP tool names joined this request — used below to
            // build the deferred exposure tier when `defer_mcp_tools` is on.
            let mut mcp_tool_names: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();

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
                    mcp_tool_names.insert(name.clone());
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

            // Markdown CLI skills join at the same seam, for the same reason:
            // the registry is rebuilt per request, so an install that landed
            // mid-session is simply present on the next turn. They used to
            // arrive through a `ToolRefreshSource` whose result
            // `ScopedToolService::list()` discarded, which meant a skill
            // installed at runtime was never callable at all.
            {
                let joined = super::super::markdown_skill_tools::join_markdown_skills(
                    &mut loop_registry_inner,
                    |name| agent.is_tool_allowed(name),
                    &mut allowed_names,
                )
                .await;
                if joined > 0 {
                    info!(
                        run_id = run_id,
                        count = joined,
                        "markdown CLI skills joined tool surface"
                    );
                }
            }

            // Register the on-demand schema loader ONLY when progressive
            // disclosure is active. When disabled (core empty / ["*"]), nothing
            // is collapsed, so get_tool_schema has nothing to serve and must NOT
            // appear — keeping the tool surface byte-identical to pre-feature
            // (escape hatch) and preserving prompt-cache continuity.
            if crate::tools::scoped::ProgressiveDisclosureRewriter::is_enabled(&mode_core_tools) {
                // Snapshot every tool's ORIGINAL full schema before progressive
                // disclosure collapses them, then register the on-demand loader.
                let schema_snapshot: std::collections::HashMap<String, serde_json::Value> =
                    loop_registry_inner
                        .tool_definitions()
                        .into_iter()
                        .map(|d| (d.name, d.parameters))
                        .collect();
                loop_registry_inner.register(Box::new(
                    crate::tools::schema_lookup::SchemaLookupTool::new(std::sync::Arc::new(
                        schema_snapshot,
                    )),
                ));
                // Ensure the loader survives a non-empty allow-filter (empty =
                // allow-all; inserting into an empty set would flip it restrictive).
                if !allowed_names.is_empty() {
                    allowed_names
                        .insert(crate::tools::schema_lookup::SchemaLookupTool::NAME.to_string());
                }
            }

            // Deferred exposure tier (opt-in): when `[tools] defer_mcp_tools`
            // is on, MCP tools are dropped from the model's initial list and
            // surfaced only via `tool_search`. Build the search corpus from the
            // pre-collapse registry snapshot (name + description + full schema)
            // and register the meta-tool. Empty when off ⇒ byte-identical.
            // ONE shared handle: the ScopedToolService filters on it and
            // `tool_search` shrinks it when the model discovers a tool. Two
            // independent sets here would make deferred tools permanently
            // uncallable, which is exactly the bug this replaced.
            // Mode-deferred families join the same tier: the session mode's
            // static name partition (chat defers desktop/browser/media/teams/
            // automation, code defers desktop/media, work defers nothing) is
            // computed against the registry's REAL names, so a family that is
            // not registered costs nothing. One shared handle with
            // `tool_search` keeps every deferred tool promotable (R7).
            let mut deferred_names = if self.config.defer_mcp_tools {
                mcp_tool_names
            } else {
                std::collections::BTreeSet::new()
            };
            deferred_names.extend(
                loop_registry_inner
                    .tool_definitions()
                    .into_iter()
                    .map(|d| d.name)
                    .filter(|name| session_mode.defers_tool(name)),
            );
            let deferred = if deferred_names.is_empty() {
                crate::tools::scoped::DeferredTools::empty()
            } else {
                crate::tools::scoped::DeferredTools::new(deferred_names)
            };
            if !deferred.is_empty() {
                let docs: Vec<crate::tools::tool_search::ToolDoc> = loop_registry_inner
                    .tool_definitions()
                    .into_iter()
                    .map(|d| crate::tools::tool_search::ToolDoc {
                        name: d.name,
                        description: d.description,
                        schema: d.parameters,
                    })
                    .collect();
                loop_registry_inner.register(Box::new(
                    crate::tools::tool_search::ToolSearchTool::new(docs, deferred.clone()),
                ));
                if !allowed_names.is_empty() {
                    allowed_names
                        .insert(crate::tools::tool_search::ToolSearchTool::NAME.to_string());
                }
                // tool_search joins the collapsed-but-unsnapshotted class
                // (registered AFTER the get_tool_schema snapshot above, like
                // subagent/get_tool_schema — see `default_core_tools`'s
                // invariant): keep it schema-resident so the rewriter never
                // points the model at a snapshot lookup that must miss. Only
                // when disclosure is active — pushing into an empty core set
                // would flip the operator's escape hatch back on.
                if crate::tools::scoped::ProgressiveDisclosureRewriter::is_enabled(&mode_core_tools)
                {
                    let ts = crate::tools::tool_search::ToolSearchTool::NAME;
                    if !mode_core_tools.iter().any(|c| c == ts) {
                        mode_core_tools.push(ts.to_string());
                    }
                }
            }

            let loop_registry = Arc::new(loop_registry_inner);

            // Build parent view ToolService WITHOUT the subagent tool
            let parent_view_for_children: Arc<dyn crate::tools::service::ToolService> =
                super::super::build_request_tool_service(
                    loop_registry.clone(),
                    allowed_names.clone(),
                    None,
                    Some(turn_context.clone()),
                    hook_executor.clone(),
                    hook_session_id.clone(),
                    tool_permissions.clone(),
                    exec_tier,
                    unattended,
                    &mode_core_tools,
                    self.config.truncate_tool_descriptions,
                    deferred.clone(),
                    self.tool_health.clone(),
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
            // Forwards to the inner (persistence + scratchpad) sink unchanged;
            // on unattended runs the redacting sink below wraps OUTSIDE this
            // one, so every event is masked before reaching any of them.
            let trace_sink: Arc<dyn crate::harness::TraceSink> =
                Arc::new(super::super::AgentTraceEmitSink::new(
                    trace_sink,
                    Arc::clone(&emitter),
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
                let mut t = SubagentTool::new(
                    sub_provider,
                    run_chain,
                    agent_registry,
                    background_tracker,
                    sub_session,
                    parent_view_for_children,
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
                // Stage A (P1) — inherit the main harness's `[stability]`
                // runaway protection so a stuck/looping/hung subagent aborts on
                // the same stall watchdog, consecutive-failure cap, and per-turn
                // timeout the parent run enforces. The whole plumbing
                // (SubagentTool → AgentRuntime → SpawnerBase → HarnessDeps)
                // existed and was documented "inherited by subagents", but this
                // construction site never populated it — subagents ran unbounded.
                // `None` (no [stability] config / mocks / simple engine) leaves
                // subagents unbounded, matching the main harness.
                if let Some(sc) = orchestrator.harness.stall_config() {
                    t = t.with_stall_config(sc);
                }
                if let Some(cap) = orchestrator.harness.consecutive_failure_cap() {
                    t = t.with_consecutive_failure_cap(cap);
                }
                if let Some(tt) = orchestrator.harness.turn_timeout() {
                    t = t.with_turn_timeout(tt);
                }
                // B15 — inherit the main harness's boot-time `[execution]
                // max_iterations` so a subagent whose role declares no cap is
                // capped too. The main path is never uncapped; the spawn path
                // passed `AgentDef.max_iterations` straight through, and `None`
                // there means *unbounded* — the built-in "default" role (the
                // landing spot when the model omits `agent_type`) declares no
                // cap, so it ran until the spawn timeout killed it and discarded
                // the work. `None` here (mocks / simple engine) still leaves the
                // child capped at the spawner's fallback.
                if let Some(mi) = orchestrator.harness.default_max_iterations() {
                    t = t.with_default_max_iterations(mi);
                }
                // Inherit the runner's `[tool_service] parallel_tool_concurrency`
                // so a subagent's Act-phase cap (including 0/1 = disabled)
                // matches the operator's configured value. The whole chain
                // (SubagentTool → AgentRuntime → SpawnerBase → HarnessDeps)
                // previously bottomed out in a hardcoded config default, so
                // disabling parallel dispatch never reached subagents.
                if let Some(cap) = orchestrator.harness.parallel_tool_concurrency() {
                    t = t.with_parallel_tool_concurrency(cap);
                }
                // Inherit the runner's `[context_budget]` config so a spawned
                // child builds its OWN budget + compactor + preflight pipeline.
                // The spawner pinned all three to `None`, so a subagent had no
                // context management whatsoever: `build_prompt` replays the
                // whole child log every turn, nothing compacted it, and when the
                // provider finally said `prompt_too_long` the reactive drain
                // found no compactor, marked the rescue exhausted, and killed
                // the run. `None` (mocks / simple engine / `[context_budget]`
                // disabled) keeps the child unmanaged, matching the main run.
                if let Some(cfg) = orchestrator.harness.context_budget_config() {
                    t = t.with_context_budget_config(cfg);
                }
                // ...and the flash-tier summarizer that config's compactor is
                // meant to run on. Inheriting the budget without this gave the
                // child a compactor that billed the main reasoning model for
                // every summarization the root agent had already routed to a
                // cheap sibling — silent, and multiplied by the fan-out width.
                if let Some(cheap) = orchestrator.harness.cheap_summary_provider() {
                    t = t.with_cheap_summary_provider(cheap);
                }
                // P3 Stage I — hand subagents the shared plugin-registry handle
                // so a role declaring `mcp_servers:` frontmatter can provision
                // its per-agent MCP scope (reference validation + referenced-tool
                // snapshot at spawn). The whole chain (SubagentTool →
                // AgentRuntime → SpawnerBase → McpScope::provision) existed but
                // this construction site never populated it, so such a subagent
                // fail-loud'd at spawn ("plugin_registry is None but
                // agent_def.mcp_servers is non-empty"). `None` (mocks / simple
                // engine with no extension manager) leaves MCP scope disabled,
                // which only fail-louds if a role actually declares mcp_servers.
                if let Some(ext) = extension_manager.as_ref() {
                    t = t.with_plugin_registry(ext.plugin_registry_handle());
                }
                // Fix 4 — thread the capture-filter registry so a spawned
                // subagent's completion fires `on_delegation` memory-extension
                // hooks (spawn.rs `dispatch_on_delegation`) and the child's
                // memory writes route through the capture filter. The whole
                // chain (SubagentTool → AgentRuntime → SpawnerBase) existed and
                // `dispatch_on_delegation` was fully built, but this site never
                // populated `capture_registry`, so `deleg_registry` was always
                // None. `None` (tests / simple engine / no memory extensions)
                // leaves it dormant — the registry no-ops when empty.
                if let Some(cr) = self.capture_registry.as_ref() {
                    t = t.with_capture_registry(cr.clone());
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
                // The child's tool surface is built from the parent's mode
                // partition (`parent_view_for_children` above), so tell the
                // child which mode that was. Work is the identity partition —
                // skip the weld to keep those child prompts byte-identical.
                if session_mode != crate::config::types::policies::SessionMode::Work {
                    t = t.with_session_mode(session_mode);
                }
                Arc::new(t)
            };

            let tool_service = super::super::build_request_tool_service(
                loop_registry,
                allowed_names,
                Some(subagent_tool),
                Some(turn_context.clone()),
                hook_executor.clone(),
                hook_session_id.clone(),
                tool_permissions.clone(),
                exec_tier,
                unattended,
                &mode_core_tools,
                self.config.truncate_tool_descriptions,
                deferred.clone(),
                self.tool_health.clone(),
            );

            // Legacy backfill: a session with `messages` rows but no
            // `session_events` (pre-event-log era) would have its history
            // re-emitted by `seed_session` and re-materialized by the projector
            // → duplicate `messages` rows. Populate the event log directly here
            // (raw store append, bypassing the projector) so `seed_session`'s
            // History guard sees a non-empty log and skips re-seeding. Idempotent
            // and best-effort. The cheap `load_head_seq` gate keeps ongoing
            // (events already present) runs off the message store's read path;
            // only a first turn / legacy continuation reads `get_history`.
            if let (Some(event_store), Some(svc)) = (
                crate::session::store::global_session_event_store(),
                crate::session::service::global_session_service(),
            ) {
                let events_empty = event_store
                    .load_head_seq(&request.session_key)
                    .await
                    .map(|h| h == 0)
                    .unwrap_or(false);
                if events_empty {
                    if let Ok(legacy_msgs) = agent
                        .session_store()
                        .get_history(&request.session_key, None)
                        .await
                    {
                        let _ = crate::orchestrator::harness_bridge::backfill::backfill_events_from_messages(
                            &request.session_key,
                            svc.as_ref(),
                            event_store.as_ref(),
                            &legacy_msgs,
                        )
                        .await;
                    }
                }
            }

            // Build FlowRequest. A resumed run carries no fresh input — the
            // session event log already holds the full trajectory. The
            // `ResumeCoordinator` sets `metadata["resume"] = "true"`; the
            // harness bridge then skips seeding and replays the log.
            let flow_input = if request.metadata.get("resume").map(String::as_str) == Some("true") {
                crate::orchestrator::FlowInput::Resume
            } else if media_blocks.is_empty() {
                super::super::helpers::history_to_flow_input(
                    history.clone(),
                    effective_user_input.clone(),
                )
            } else {
                // Attachment turn: seed ONLY the new user message, carrying the
                // media blocks. Prior turns are already in the event log —
                // either persisted by earlier turns, or backfilled from the
                // legacy `messages` rows just above — so, unlike
                // `FlowInput::History`, this variant needs no empty-log replay
                // guard and cannot duplicate history on turn 2+.
                crate::orchestrator::FlowInput::Multimodal(vec![
                    crate::session::events::MessageContent {
                        text: effective_user_input.clone(),
                        blocks: media_blocks.clone(),
                        thinking: None,
                        thinking_signature: None,
                    },
                ])
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
                // P1 data isolation: forward the owner/scope attribution
                // already stamped on this request's metadata verbatim — the
                // two strings, not a re-derived `ScopeAttribution`, since
                // `FlowRequest` carries them as strings (see its doc).
                owner_user_id: request.metadata.get(crate::scope::OWNER_META_KEY).cloned(),
                scope_id: request.metadata.get(crate::scope::SCOPE_META_KEY).cloned(),
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
                // Ephemeral per-turn reminders (working directory, project
                // CLAUDE.md/AGENTS.md, UserPromptSubmit hook additions).
                // Delivered to the model as the transient trailing recall
                // message and never persisted — see the assembly above.
                // Cloned because this FlowRequest is rebuilt each retry
                // iteration of the enclosing `loop` (mirrors
                // `effective_user_input.clone()`).
                transient_context: transient_context.clone(),
                // Reasoning depth for this turn — resolved above, forwarded to
                // the harness bridge, and finally stamped onto every
                // `RequestPayload` in `think.rs`.
                think_level,
                // This turn's operating envelope. Every field is the SAME value
                // already fed to the machinery it describes, so the prompt can
                // never advertise a regime the runtime does not apply:
                //   - `exec_tier` is the tier handed to the `ScopedToolService`
                //     gate above (codex `<approval_policy>` parity, R9);
                //   - `session_mode` is the mode that partitioned the tool
                //     surface above (R9: behavioural half in the prompt, code
                //     only partitions);
                //   - `cwd` is `effective_workspace`, the same path given to the
                //     tool adapters as `default_working_dir` — i.e. where a shell
                //     call actually runs, and the anchor for `repo=` / `git=`.
                envelope: crate::thinker::TurnEnvelope {
                    exec_tier: Some(exec_tier),
                    session_mode: Some(session_mode),
                    // rust-doctor-disable-next-line excessive-clone
                    cwd: Some(effective_workspace.clone()),
                    //   - `serving_model` is NOT resolvable here: which model
                    //     actually answers is decided after the provider chain
                    //     is built. `runner_impl` fills it in from the same
                    //     `gauge_model` the context gauge and cost estimate
                    //     use, so all three agree.
                    serving_model: None,
                    // Primary user-facing sessions carry no parent binding;
                    // sub-agent / team / background dispatchers set this
                    // when they know the parent session (R7/R10: pure
                    // pass-through from their context — no judgment near
                    // the wire). `None` keeps `## Operating Envelope`
                    // byte-identical for the 99% case.
                    parent: None,
                    // Same: primary dispatch has no `run_id`; set by
                    // sub-agent dispatchers when their background tracker
                    // has minted one. `None` keeps the prompt byte-identical.
                    run_id: None,
                    // `response_language` is bound at the gateway layer
                    // (chat.send / voice / cron) from the request pill /
                    // session default; primary dispatch inherits whatever
                    // the gateway already resolved.
                    response_language: None,
                },
                // This turn's explicit model pick. Cloned because the request is
                // rebuilt on each retry iteration of the enclosing loop.
                // rust-doctor-disable-next-line excessive-clone
                model_directive: turn_model.clone(),
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
                Arc::clone(&emitter);

            let dispatch_result = super::super::helpers::run_dispatch_and_drain_classified(
                orchestrator,
                req,
                emitter_dyn,
                run_id,
                cancel_token.clone(),
                locale,
                &occupancy_out,
            )
            .await;

            match dispatch_result {
                Ok(response) => {
                    if !media_blocks.is_empty() {
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
                    if !media_blocks.is_empty() {
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
                    // The provider that actually failed is `prov_name`, carried
                    // on the failure itself and logged below. It used to *also*
                    // be recorded against a health table — but against
                    // `resolved.provider_name`, the pre-request prediction,
                    // rather than `prov_name`. That table is gone; the circuit
                    // breaker that does gate dialing lives in `FailoverProvider`
                    // and is fed by the walk, which is the only layer that knows
                    // which endpoint the failure came from.
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
                    if !media_blocks.is_empty() {
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
                    if !media_blocks.is_empty() {
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

/// Does the model serving this turn accept inline image blocks?
///
/// `configured` is the model id the turn resolves to; `serving_hint` is the
/// live provider chain's [`AiProvider::serving_model_hint`](crate::providers::AiProvider::serving_model_hint),
/// which covers the ids the catalogue cannot answer for on its own (agent model
/// left unset, MoA / failover dynamic routing). The first id the capability
/// catalogue knows wins — a pure static table read, blind to message content.
///
/// **Unknown capabilities fail OPEN (image is sent).** The two failures are not
/// symmetric: sending an image to an unlisted text-only model produces a loud,
/// attributable provider error (or an ignored block) the user can act on,
/// whereas degrading an image for a model that could have seen it is silent —
/// and when no `VisionPipeline` is configured the degradation is not even a
/// description but a bare name/type summary, i.e. total loss of the picture
/// itself. Unknown ids are overwhelmingly custom endpoints / proxy
/// aliases of modern (vision-capable) models, so the closed default would
/// blind them all. This matches the deliberate fail-open stance of
/// [`providers::capability_gate`](crate::providers::capability_gate): the
/// static table is best-effort, never authoritative.
pub(super) fn model_supports_vision(configured: &str, serving_hint: Option<&str>) -> bool {
    [Some(configured), serving_hint]
        .into_iter()
        .flatten()
        .find_map(crate::providers::capabilities_for)
        .is_none_or(|caps| caps.supports_vision)
}

/// Copy every inbound attachment into the durable artifact store, then ping
/// subscribers that the session's artifact list moved.
///
/// Entirely best-effort: a read-only data directory, an oversized payload or an
/// unreachable URL costs that one attachment, never the user's turn. The
/// attachment still reaches the model through `MediaProcessor` either way.
async fn archive_inbound_attachments(
    attachments: &[crate::gateway::channel::Attachment],
    session_key: &str,
    run_id: &str,
    event_bus: Option<&crate::gateway::event_bus::GatewayEventBus>,
) {
    let root = match crate::artifacts::ArtifactStore::default_root() {
        Ok(root) => root,
        Err(e) => {
            warn!(error = %e, "cannot resolve the artifact root; inbound attachments not archived");
            return;
        }
    };
    let store = crate::artifacts::ArtifactStore::new(root);
    let cache = crate::media::cache::MediaCache::new();
    let mut stored = 0usize;

    for attachment in attachments {
        // Inline bytes are the common case (Panel and Slack uploads) and need
        // no I/O at all. A `path` or `url` attachment goes through the media
        // cache so the SSRF policy and the 50 MB ceiling apply here exactly as
        // they do on the injection path. That costs a second fetch for a remote
        // URL, which is the price of not letting the only copy of the bytes
        // vanish with the temp directory at the end of the run.
        let bytes = match attachment.data.clone() {
            Some(bytes) => bytes,
            None => match cache.resolve(attachment, session_key).await {
                Ok(cached) => match tokio::fs::read(&cached.local_path).await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!(
                            attachment_id = %attachment.id,
                            error = %e,
                            "cannot read a resolved attachment for archiving"
                        );
                        continue;
                    }
                },
                Err(e) => {
                    warn!(
                        attachment_id = %attachment.id,
                        error = %e,
                        "cannot resolve an attachment for archiving"
                    );
                    continue;
                }
            },
        };

        let filename = attachment
            .filename
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&attachment.id);
        match store
            .put(
                session_key,
                Some(run_id),
                crate::artifacts::ArtifactOrigin::Inbound,
                filename,
                &attachment.mime_type,
                &bytes,
            )
            .await
        {
            Ok(record) => {
                stored += 1;
                tracing::debug!(
                    artifact_id = %record.id,
                    filename = %record.filename,
                    size = record.size,
                    "archived an inbound attachment"
                );
            }
            Err(e) => warn!(
                attachment_id = %attachment.id,
                error = %e,
                "failed to archive an inbound attachment"
            ),
        }
    }

    if stored > 0 {
        publish_artifact_invalidation(event_bus, session_key);
    }
}

/// Broadcast the content-free `session.artifact` ping on the injected bus.
///
/// The frame itself is built in
/// [`crate::gateway::event_emitter::artifact_ping`] — the single source for its
/// shape, shared with the outbound harvest at the tool chokepoint. Only the bus
/// resolution differs between the two producers, and that difference is real:
/// here one was injected, there it has to be looked up.
fn publish_artifact_invalidation(
    event_bus: Option<&crate::gateway::event_bus::GatewayEventBus>,
    session_key: &str,
) {
    let Some(bus) = event_bus else { return };
    crate::gateway::event_emitter::artifact_ping::publish_artifact_ping_on(bus, session_key);
}
