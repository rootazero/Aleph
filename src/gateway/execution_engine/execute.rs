use super::gate::GateOutcome;
use super::{ExecutionEngine, ExecutionError, RunRequest, RunState};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::resilience::TaskStatus;
use crate::sync_primitives::Arc;
use crate::verification::stop_hooks::{execute_stop_hooks_arc, StopHookContext};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Pure gate for the naked agent-loop strategic planner. ORIGIN/PLUMBING FACTS
/// ONLY (R7 — never a message-content heuristic): fire only for a genuine human
/// interactive first message. Excludes cron / group-chat member / subagent /
/// ephemeral runs (non-interactive `SessionKey`), resume runs, empty input,
/// and slash-command invocations (a `/name ...` first message is protocol
/// syntax, not a naked objective — `/goal`·`/loop` run their OWN planner, and
/// planning over raw slash text would weld a garbage session Strategy; most
/// slash inputs used to be excluded implicitly by the fast path's metadata
/// flag, but `/loop`·`/goal` now deliberately fall through to the full loop).
/// Pure ⇒ host-testable without an LLM or a live gateway.
fn naked_loop_planner_should_fire(
    session_key: &crate::routing::session_key::SessionKey,
    is_first_message: bool,
    is_resume: bool,
    input: &str,
) -> bool {
    let trimmed = input.trim();
    is_first_message
        && session_key.is_interactive()
        && !is_resume
        && !trimmed.is_empty()
        && !trimmed.starts_with('/')
}

/// Cap the planner objective at a generous UTF-8 char boundary. Naked-loop input
/// is raw channel text (goal/loop objectives are already tool-bounded), so a
/// multi-KB paste must not bloat the planner prompt.
fn bounded_objective(input: &str) -> String {
    const MAX_OBJECTIVE_CHARS: usize = 4000;
    match input.char_indices().nth(MAX_OBJECTIVE_CHARS) {
        Some((byte_idx, _)) => input[..byte_idx].to_string(),
        None => input.to_string(),
    }
}

/// Round-2 B7: decide the rewritten input for a channel-path `/moa` one-shot
/// fallthrough. Returns `Some(prompt)` only when the input parses as a
/// `/moa <prompt>` one-shot AND the prompt is not itself a slash command —
/// exactly the condition under which the arming site
/// (`slash_command.rs`'s "moa" arm / this file's Panel-CLI intercept) arms
/// MoA. A slash remainder (e.g. `/moa /help`) was never armed, and unlike the
/// Panel/CLI path the channel path has no second slash-resolution pass after
/// the fast path — stripping would hand the LLM a bare `/help` as literal
/// turn text — so the input is left untouched (`None`).
fn moa_fallthrough_input(original: &str) -> Option<String> {
    let prompt = crate::providers::moa::parse_one_shot_command(original)?;
    (!prompt.trim_start().starts_with('/')).then(|| prompt.to_string())
}

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
    /// Fire the tool-free strategic planner ONCE for a genuine human first
    /// message on the naked agent loop, concurrently with the remaining
    /// first-message setup. Returns `(store_key, handle)` the caller awaits and
    /// stores BEFORE dispatching the harness run, so the Strategy is welded on
    /// turn 1. Returns `None` (a no-op) when the planner is disabled
    /// (`planner_provider` is `None` ⇒ `[strategy].enabled`/`plan_naked_loop`
    /// off), the origin gate rejects the run, the strategy subsystem is
    /// uninitialized, or a Strategy already exists for this session (fire-once).
    fn spawn_naked_loop_planner(
        &self,
        request: &RunRequest,
        is_first_message: bool,
    ) -> Option<(
        String,
        tokio::task::JoinHandle<Option<crate::strategy::Strategy>>,
    )> {
        let provider = self.planner_provider.clone()?;
        let is_resume = request.metadata.get("resume").map(String::as_str) == Some("true");
        if !naked_loop_planner_should_fire(
            &request.session_key,
            is_first_message,
            is_resume,
            &request.input,
        ) {
            return None;
        }
        let store = crate::strategy::global()?;
        let session_key_str = request.session_key.to_key_string();
        let key = crate::strategy::session_key(&session_key_str);
        // Fire-exactly-once: only when the slot is provably empty. `matches!`,
        // NOT `==` (anyhow::Result<Option<_>> is not Eq); a transient Err skips
        // so a read failure never risks a double-write (P7).
        if !matches!(store.get(&key), Ok(None)) {
            return None;
        }
        let objective = bounded_objective(&request.input);
        let handle = tokio::spawn(async move {
            let ctx = crate::strategy::planner::PlannerContext {
                tool_descriptions: Vec::new(),
                env_summary: crate::strategy::planner::env_summary(),
                lessons: Vec::new(),
            };
            crate::strategy::planner::plan_strategy(&provider, &objective, &ctx, None).await
        });
        Some((key, handle))
    }

    /// Execute a run request
    ///
    /// Returns a stream of events for the run.
    ///
    /// # Arguments
    ///
    /// * `request` - The run request containing input and metadata
    /// * `agent` - The agent instance to execute with
    /// * `emitter` - Event emitter for streaming events
    pub async fn execute<E: EventEmitter + Send + Sync + 'static>(
        &self,
        mut request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<(), ExecutionError> {
        let run_id = request.run_id.clone();

        // Create cancellation channel
        let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);

        // Create CancellationToken for fine-grained agent loop cancellation.
        // The bridge task converts the coarse cancel_rx signal into a token cancellation.
        let cancel_token = CancellationToken::new();

        // Admission gate: claim the session's run slot, resolve busy-input
        // policy (Steer / Interrupt / Queue) if it's already held, acquire a
        // run-lifetime concurrency permit, and register the `ActiveRun`.
        // Extracted to `gate.rs` (Task 2); Task 6 rewrote the predicate to be
        // per-session (`SessionRunRegistry`) instead of per-agent
        // (`AgentState`). `_run_slot` is bound here — in `execute()`'s own
        // body scope, not inside this match arm — so it lives across the
        // run's execution (including every early return below and a panic
        // unwind). It is dropped explicitly further down, at the same point
        // the legacy per-agent gate used to reset `AgentState::Idle`, which
        // is before the post-run continuation/steering-rescue logic that may
        // re-enter `execute()` on the SAME session (see the drop site for why
        // holding it until the literal end of this function would deadlock
        // that re-entry).
        let _run_slot = match self
            .admit_run(&request, &run_id, &agent, cancel_tx)
            .await?
        {
            GateOutcome::Admitted(slot) => slot,
            GateOutcome::HandledInline => return Ok(()),
        };

        let trace_task_persisted = self
            .persist_run_task_started(&run_id, &request, &agent)
            .await;

        // Emit run accepted event
        if let Err(e) = emitter
            .emit(StreamEvent::RunAccepted {
                run_id: run_id.clone(),
                session_key: request.session_key.to_key_string(),
                accepted_at: chrono::Utc::now().to_rfc3339(),
            })
            .await
        {
            tracing::warn!(
                run_id = %run_id,
                session_key = %request.session_key.to_key_string(),
                error = %e,
                "failed to emit RunAccepted stream event"
            );
        }

        // Log lifecycle event: agent started
        info!(
            event_type = "agent.lifecycle.started",
            agent_id = %agent.id(),
            run_id = %run_id,
            "Agent execution started"
        );

        // Ensure session exists in memory + SQLite before adding messages
        agent.ensure_session(&request.session_key).await;

        // Check if this is the first real user message (for auto-topic generation).
        // Slash commands routed via fast-path don't count — they bypass
        // add_message entirely, so history stays empty for the next real message.
        let history_empty = agent
            .get_history(&request.session_key, Some(1))
            .await
            .is_empty();
        let is_slash = request.metadata.contains_key(SLASH_COMMAND_MODE_KEY);
        let is_first_message = history_empty && !is_slash;

        if is_first_message {
            info!(
                session_key = %request.session_key.to_key_string(),
                "First message detected for session (will generate topic)"
            );
            // Record the originating channel once, on session creation, so
            // sessions.list / sessions.changed can surface conversation origin
            // (Panel can identify + continue channel-originated conversations).
            // "gui:chat" for Panel chat.send, the real channel id for inbound
            // channels (both land in metadata["channel_id"]). The store call is
            // idempotent and skips the empty/"unknown" sentinel.
            if let Some(channel) = request.metadata.get("channel_id") {
                let conversation = request.metadata.get("conversation_id").map(String::as_str);
                agent
                    .set_session_source_channel(&request.session_key, channel, conversation)
                    .await;
            }
        }

        // Resume-style runs (crash resume, post-run steering rescue) carry no
        // fresh input — the session log already holds the full trajectory —
        // so a resume-flag check is still needed for the memory write below.
        // The user message is now SSOT via SessionEvent::UserMessage (seed_session
        // path) — no direct add_message call here.
        let is_resume = request.metadata.get("resume").map(String::as_str) == Some("true");

        // Announce the session the moment it's created (first message), not just
        // when the run completes. Without this, a brand-new session is silent for
        // the entire first turn and never appears in clients that refresh their
        // session list on `SessionUpdated` (e.g. the Panel sidebar) until a manual
        // reload. The run-completion `SessionUpdated` below still fires for
        // topic/title/token updates. Published on the global bus (not the
        // per-run emitter) so channel-originated runs reach the Panel too.
        if is_first_message {
            self.publish_session_updated(
                &request.session_key,
                request.metadata.get("channel_id").map(String::as_str),
            );
        }

        // Inline slash command resolution for non-router paths (Panel, CLI)
        if request.input.trim().starts_with('/')
            && !request.metadata.contains_key(SLASH_COMMAND_MODE_KEY)
        {
            // `/moa <prompt>` one-shot (hermes semantics): arm MoA for THIS run
            // and let the prompt run as a normal turn. The argument is ALWAYS a
            // prompt, never a preset name. Bare `/moa` falls through to the LLM
            // (parse_one_shot_command returns None; `moa` stays fast-path-excluded
            // below so the LLM maps it onto the tool's structured schema). State
            // is written here, before `try_resolve_slash_command` and well before
            // the run is constructed further down this function.
            if let Some(prompt) = crate::providers::moa::parse_one_shot_command(&request.input) {
                // If the remainder is ITSELF a slash command (e.g. `/moa /help`),
                // do not arm MoA: after the rewrite below, `try_resolve_slash_command`
                // can resolve it as its own fast path that returns before run
                // construction, so `take_for_run` never consumes the pref and it
                // leaks into the user's next turn. Still strip the `/moa ` prefix
                // so the inner command resolves exactly as if typed directly.
                if !prompt.trim_start().starts_with('/') {
                    let key = request.session_key.to_key_string();
                    crate::providers::session_moa_handle::set_session_moa(&key, None, true);
                    // Selector slots are mutually exclusive; mirror
                    // moa_manage::activate() (set-then-clear ordering).
                    crate::providers::session_model_handle::clear_session_model(&key);
                }
                request.input = prompt.to_string();
            }

            if let Some(mode_json) = self.try_resolve_slash_command(&request.input) {
                request
                    .metadata
                    .insert(SLASH_COMMAND_MODE_KEY.to_string(), mode_json);
            }
        }

        // Naked agent-loop strategic planner (StraTA round 2): on a genuine
        // human first message that is NOT a slash command, plan a Strategy
        // concurrently with the remaining first-message setup. Placed AFTER
        // slash resolution so the SLASH_COMMAND_MODE_KEY plumbing flag (set
        // just above) lets us skip slash commands: /goal·/loop·/workflow run
        // their OWN planner, and fast-path commands return before the await
        // below, so spawning for them would burn a detached planner LLM call
        // whose result is silently discarded (dropping a tokio JoinHandle
        // detaches the task; it does not cancel it). Skipping on the plumbing
        // flag — not a message-content heuristic — keeps this R7-clean.
        // Awaited + stored just before harness dispatch so StrategyLayer welds
        // it on turn 1. A no-op otherwise.
        let naked_loop_planner = if request.metadata.contains_key(SLASH_COMMAND_MODE_KEY) {
            None
        } else {
            self.spawn_naked_loop_planner(&request, is_first_message)
        };

        // Propagate session context BEFORE fast path so agent management
        // tools (agent_create, agent_delete) can resolve the session correctly.
        if let Some(sc_handle) = self.tool_registry.session_context_handle() {
            let mut sc = sc_handle.write().await;
            sc.channel = request
                .metadata
                .get("channel_id")
                .cloned()
                .unwrap_or_default();
            sc.peer_id = request
                .metadata
                .get("sender_id")
                .cloned()
                .unwrap_or_default();
            sc.session_key_str = request.session_key.to_key_string();
            sc.conversation_id = request
                .metadata
                .get("conversation_id")
                .cloned()
                .unwrap_or_default();
        }

        // Propagate session key to the shared registry handle. This handle is
        // process-global and rewritten by every concurrent run, so it is NOT
        // authoritative during tool execution: session-binding tools (loop /
        // goal / scratchpad / strategy / memory_search) read the per-run
        // TURN_CONTEXT task-local first and only fall back to this handle on
        // non-scoped paths (direct invocations outside the dispatch chokepoint).
        if let Some(sk_handle) = self.tool_registry.session_key_handle() {
            *sk_handle.write().await = request.session_key.to_key_string();
        }

        // Propagate the active agent's memory scope + smart-recall profile to
        // memory_search (see MemorySearchTool::default_workspace_handle /
        // smart_recall_config_handle).
        //
        // The workspace handle is process-global and rewritten by every
        // concurrent run, so it is NOT authoritative during tool execution:
        // memory_search reads the per-run TURN_CONTEXT task-local first
        // (turn_context::current_agent_id) and only falls back to this handle
        // on non-scoped paths. The smart-recall map is keyed per agent, so a
        // concurrent run of another agent writes its own entry instead of
        // clobbering this run's profile.
        let env_agent_id = request.session_key.agent_id().to_string();
        if let Some(ws_handle) = self.tool_registry.workspace_handle() {
            *ws_handle.write().await = env_agent_id.clone();
        }
        if let Some(sr_handle) = self.tool_registry.smart_recall_config_handle() {
            let smart_recall = match self.workspace_manager {
                Some(ref wm) => {
                    crate::gateway::agent_env::ActiveAgentEnv::from_agent_id(wm, &env_agent_id)
                        .await
                        .profile
                        .smart_recall
                }
                None => None,
            };
            let mut slots = sr_handle.write().await;
            match smart_recall {
                Some(cfg) => {
                    slots.insert(env_agent_id.clone(), cfg);
                }
                None => {
                    slots.remove(&env_agent_id);
                }
            }
        }

        // Pre-extract skill context from the slash mode JSON so the agent
        // loop (run_loop.rs) can apply `allowed_tools` intersection without
        // re-parsing the envelope on every Think→Act iteration. Two keys are
        // emitted: `slash_skill_instructions` (raw markdown for prompt
        // overlay) and `slash_skill_allowed_tools` (comma-separated tool name
        // whitelist). The keys are absent unless the slash command resolves
        // to a Skill source with a non-empty allowed_tools list / non-empty
        // instructions.
        if let Some(mode_json) = request.metadata.get(SLASH_COMMAND_MODE_KEY).cloned() {
            if let Ok(mode) = serde_json::from_str::<serde_json::Value>(&mode_json) {
                if mode.get("type").and_then(|v| v.as_str()) == Some("skill") {
                    if let Some(instructions) = mode.get("instructions").and_then(|v| v.as_str()) {
                        if !instructions.is_empty() {
                            request.metadata.insert(
                                "slash_skill_instructions".to_string(),
                                instructions.to_string(),
                            );
                        }
                    }
                    if let Some(allowed) = mode.get("allowed_tools").and_then(|v| v.as_array()) {
                        let tools: Vec<String> = allowed
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect();
                        if !tools.is_empty() {
                            request
                                .metadata
                                .insert("slash_skill_allowed_tools".to_string(), tools.join(","));
                        }
                    }
                }
            }
        }

        // Slash command fast path (L0): bypass full agent loop
        if let Some(mode_json) = request.metadata.get(SLASH_COMMAND_MODE_KEY) {
            let fast_result = self
                .execute_slash_command_fast_path(
                    &run_id,
                    mode_json,
                    &request,
                    agent.clone(),
                    emitter.clone(),
                )
                .await;

            match fast_result {
                Ok(response) => {
                    return self
                        .finalize_fast_path_success(
                            &run_id,
                            &request,
                            &agent,
                            &emitter,
                            response,
                            trace_task_persisted,
                        )
                        .await;
                }
                Err(ExecutionError::Fallthrough { ref reason }) => {
                    // Skills/custom commands need LLM processing — fall through to agent loop
                    let mut runs = self.active_runs.write().await;
                    if let Some(run) = runs.get_mut(&run_id) {
                        run.state = RunState::Running;
                    }
                    // Round-2 B7: a `/moa <prompt>` arriving through a channel
                    // reaches this fallthrough with the RAW "/moa ..." text
                    // still in request.input (the channel-path intercept can't
                    // mutate its borrow). Strip the prefix here — mirroring
                    // the Panel/CLI intercept — so the LLM sees the prompt,
                    // not the command. The strip is gated to exactly the arm
                    // condition (parse succeeds AND the prompt is not itself
                    // slash-prefixed, see `moa_fallthrough_input`): a slash
                    // remainder like `/moa /help` was never armed, and the
                    // channel path has no later slash-resolution pass, so its
                    // input stays untouched instead of degrading to a bare
                    // `/help` as literal turn text.
                    if reason == "moa one-shot" {
                        if let Some(prompt) = moa_fallthrough_input(&request.input) {
                            request.input = prompt;
                        }
                    }
                    warn!(
                        run_id = %run_id,
                        reason = %reason,
                        "Command falling through to agent loop"
                    );
                    // Fall through to normal agent loop
                }
                Err(ref e) => {
                    // Direct tool errors: return error response, do NOT fall through
                    return self
                        .finalize_fast_path_error(
                            &run_id,
                            &request,
                            &agent,
                            &emitter,
                            &e.to_string(),
                            trace_task_persisted,
                        )
                        .await;
                }
            }
        }

        // Join the naked-loop planner (started above, overlapped with setup) and
        // store its Strategy before harness dispatch so the weld is present on
        // turn 1. Best-effort: any failure (planner error / put error / join
        // error) leaves the prompt byte-identical and the run proceeds (P7).
        if let Some((key, handle)) = naked_loop_planner {
            if let Ok(Some(strategy)) = handle.await {
                if let Some(store) = crate::strategy::global() {
                    let _ = store.put(&key, &strategy);
                }
            }
        }

        // Execute the run
        let active_runs = self.active_runs.clone();
        let timeout_secs = request
            .timeout_secs
            .or(agent.config().timeout_secs())
            .unwrap_or(self.config.default_timeout_secs);

        let deadline = Arc::new(tokio::sync::Mutex::new(
            tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs),
        ));

        // Shared slot the dispatch drain fills with the last turn's authoritative
        // context-window occupancy; read after the loop to stamp it onto the
        // persisted assistant message (so the Panel gauge survives a reload).
        let occupancy_out: Arc<std::sync::Mutex<Option<super::helpers::RunContextOccupancy>>> =
            Arc::new(std::sync::Mutex::new(None));

        let result: Result<String, ExecutionError> = tokio::select! {
            result = self.run_agent_loop(
                &run_id,
                &request,
                agent.clone(),
                emitter.clone(),
                deadline.clone(),
                trace_task_persisted.then(|| run_id.clone()),
                cancel_token.clone(),
                occupancy_out.clone(),
            ) => result,

            _ = cancel_rx.recv() => {
                // Bridge: coarse cancellation → fine-grained CancellationToken
                cancel_token.cancel();
                info!("Run {} cancelled", run_id);
                Err(ExecutionError::Cancelled)
            }

            _ = super::deadline::wait_for_deadline(deadline.clone()) => {
                cancel_token.cancel();
                warn!("Run {} timed out after {}s effective time", run_id, timeout_secs);
                Err(ExecutionError::Timeout)
            }
        };

        // Update run state based on result
        let final_state = match &result {
            Ok(_) => RunState::Completed,
            Err(ExecutionError::Cancelled) => RunState::Cancelled,
            Err(e) => RunState::Failed {
                error: e.to_string(),
            },
        };

        // Log lifecycle event: agent completed
        info!(
            event_type = "agent.lifecycle.completed",
            agent_id = %agent.id(),
            run_id = %run_id,
            success = matches!(final_state, RunState::Completed),
            "Agent execution completed"
        );

        // Close out the active-run record (state, completion timestamp).
        // `final_seq` is only needed by the error branch's `RunError` emit —
        // the terminal `RunComplete` is single-sourced from the orchestrator
        // drain inside `run_agent_loop` (see `helpers::run_dispatch_and_drain
        // _classified`), which carries the enriched summary; the all-zeros
        // duplicate previously emitted here is gone.
        let final_seq = {
            let mut runs = active_runs.write().await;
            if let Some(run) = runs.get_mut(&run_id) {
                run.state = final_state.clone();
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                run.next_seq()
            } else {
                0
            }
        };

        // Release the session claim + concurrency permit now — this run's own
        // execution is complete. Explicit (not left to drop at the end of
        // `execute()`): the post-run continuation/steering-rescue logic below
        // may re-enter `execute()` on this SAME session (a fresh run, new
        // run_id), which needs `try_claim` to succeed again. Mirrors where
        // the legacy per-agent `agent.set_state(AgentState::Idle)` gate
        // release used to sit.
        drop(_run_slot);

        let final_result = match result {
            Ok(response) => {
                let response = &response;
                if trace_task_persisted {
                    self.persist_run_task_status(&run_id, TaskStatus::Completed)
                        .await;
                }

                let occupancy = occupancy_out.lock().ok().and_then(|g| *g);
                // SSOT: the harness already emitted SessionEvent::AssistantMessage for
                // `response` (the projector writes that row). Emit run_id + occupancy so
                // the projector stamps them onto that row — preserving the Panel context
                // gauge + workspace trace across a session reload.
                if let Some(svc) = crate::session::service::global_session_service() {
                    let occ = match occupancy {
                        Some(o) => o,
                        None => super::helpers::RunContextOccupancy {
                            context_tokens: 0,
                            context_window: 0,
                            total_tokens: 0,
                        },
                    };
                    let _ = svc
                        .emit_event(
                            &request.session_key,
                            crate::session::events::SessionEvent::AssistantRunMeta {
                                turn_id: uuid::Uuid::new_v4(),
                                run_id: run_id.clone(),
                                context_tokens: occ.context_tokens,
                                context_window: occ.context_window,
                                total_tokens: occ.total_tokens,
                                at: crate::session::events::now_ms(),
                            },
                        )
                        .await;
                }

                // Notify UI that the session was updated (global bus, so
                // channel-originated runs reach the Panel too). RunComplete
                // itself is single-sourced from the orchestrator drain.
                self.publish_session_updated(
                    &request.session_key,
                    request.metadata.get("channel_id").map(String::as_str),
                );

                // Persist the user-chosen project folder onto the session so the
                // Panel can restore it after a reload (project workspaces G3).
                // Stamped on the first message, mirroring source-channel/topic
                // stamping; the project↔session binding is fixed at session
                // creation, so a later switch starts a fresh session and
                // re-stamps. Best-effort: a metadata write failure never blocks
                // the turn.
                if is_first_message {
                    if let (Some(sm), Some(root)) = (
                        self.session_manager.clone(),
                        request
                            .workspace_override
                            .as_ref()
                            .map(|p| p.display().to_string()),
                    ) {
                        let key = request.session_key.clone();
                        tokio::spawn(async move {
                            if let Err(e) = sm.set_project_root(&key, Some(&root)).await {
                                warn!(error = %e, "failed to persist session project_root");
                            }
                        });
                    }
                }

                // Auto-generate session topic on first real message
                if is_first_message {
                    if let (Some(sm), Some(eb)) =
                        (self.session_manager.clone(), self.event_bus.clone())
                    {
                        let topic_provider = self
                            .provider_registry
                            .get("haiku")
                            .unwrap_or_else(|| self.provider_registry.default_provider());
                        let topic_session_key = request.session_key.clone();
                        let topic_message = request.input.clone();
                        info!(
                            session_key = %topic_session_key.to_key_string(),
                            "Auto-topic: spawning generation for first message"
                        );
                        tokio::spawn(async move {
                            let topic_text = super::topic::generate_conversation_topic(
                                &topic_provider,
                                &topic_message,
                            )
                            .await;

                            if let Err(e) = sm.set_topic(&topic_session_key, &topic_text).await {
                                warn!(error = %e, "Auto-topic: failed to persist topic");
                            } else {
                                let event_json = serde_json::json!({
                                    "method": "stream.session_updated",
                                    "params": {
                                        "session_key": topic_session_key.to_key_string(),
                                        "topic": topic_text,
                                    }
                                });
                                eb.publish(event_json.to_string());
                                info!(
                                    session_key = %topic_session_key.to_key_string(),
                                    topic = %topic_text,
                                    "Auto-topic: session topic set"
                                );
                            }
                        });
                    } else {
                        info!(
                            "Auto-topic: skipped (session_manager={}, event_bus={})",
                            self.session_manager.is_some(),
                            self.event_bus.is_some()
                        );
                    }
                }

                // Async write to memory system (Layer 1). Resume-style runs
                // have no fresh user input — a blank user/assistant pair is
                // noise in raw memory, so skip the write.
                if !is_resume {
                    if let Some(ref mb) = self.memory_backend {
                        let mb = mb.clone();
                        let sk = request.session_key.to_key_string();
                        let agent_id = request.session_key.agent_id().to_string();
                        let ui = request.input.clone();
                        let ao = response.clone();
                        tokio::spawn(async move {
                            super::history::write_conversation_memory(mb, sk, agent_id, ui, ao)
                                .await;
                        });
                    }
                }
                // Record conversation turn for compression scheduling.
                // Signal-aware: corrections compress immediately; other turns
                // ride the turn-threshold cadence.
                if let Some(ref cs) = self.compression_service {
                    cs.record_turn_and_check_signal(&request.input);
                }

                // Async session compaction (hierarchical summarization)
                if let Some(ref sc) = self.session_compactor {
                    let sc = sc.clone();
                    let agent_clone = agent.clone();
                    let session_key_clone = request.session_key.clone();
                    tokio::spawn(async move {
                        if let Err(e) = sc
                            .post_turn_compress(&agent_clone, &session_key_clone)
                            .await
                        {
                            warn!(error = %e, "Session compaction failed");
                        }
                    });
                }

                // Autonomous-continuation hook (R7/R10-safe, opt-in).
                // Fires only when the session has a standing goal with
                // `PursuitMode::Active` that still needs more work. Increments
                // the counter BEFORE spawning so termination is guaranteed even
                // if the continuation run crashes before re-entering this hook.
                if let Some(cont_deps) = self.continuation_deps.get() {
                    let goal_store = crate::goal::global();
                    if let Some(store) = goal_store {
                        let session_key_str = request.session_key.to_key_string();
                        match store.get(&session_key_str) {
                            Ok(Some(goal)) => {
                                let now_ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map_or(0, |d| d.as_millis() as u64);

                                // Token-budget accounting (codex/openclaw parity).
                                // ONLY runs when the goal carries a token_budget —
                                // the common no-budget path reads no session state
                                // and stays behavior-identical. On the first hook
                                // that sees a budget, capture the real baseline
                                // (the session's cumulative total at this moment)
                                // and persist it; thereafter pass the live total so
                                // `should_continue`/`exhausted_while_active` enforce
                                // the budget. Previously these calls hardcoded
                                // `tokens_now = 0`, so `over_budget` could never
                                // fire — the budget was advertised but dead.
                                // Only capture/enforce the token baseline on an ACTIVE
                                // goal: `tokens_now` is consumed solely by the Active
                                // path (should_continue / exhausted_while_active). For a
                                // goal the model just marked Complete (awaiting gate),
                                // the gate branch ignores `tokens_now`, so seeding a
                                // baseline there is a wasted SQLite write plus a spurious
                                // `updated_at` bump on a terminal goal.
                                let (goal, tokens_now) = if goal.token_budget.is_some()
                                    && goal.is_active()
                                {
                                    match self.session_manager.as_ref() {
                                        Some(sm) => {
                                            match sm.get_total_tokens(&request.session_key).await {
                                                Ok(Some(total)) if goal.baseline_captured => {
                                                    (goal, total)
                                                }
                                                Ok(Some(total)) => {
                                                    let seeded =
                                                        goal.clone().with_baseline(total, now_ms);
                                                    if let Err(e) = store.put(&seeded) {
                                                        warn!(error = %e, session = %session_key_str,
                                                        "goal pursuit: failed to persist token baseline; budget unenforced this turn");
                                                        (goal, 0)
                                                    } else {
                                                        // Baseline just captured → 0 spent
                                                        // so far; never a false over-budget.
                                                        (seeded, total)
                                                    }
                                                }
                                                // No row yet / read error → skip budget
                                                // enforcement this turn (graceful: iteration
                                                // and deadline caps still apply).
                                                Ok(None) => (goal, 0),
                                                Err(e) => {
                                                    warn!(error = %e, session = %session_key_str,
                                                    "goal pursuit: session token read failed; budget unenforced this turn");
                                                    (goal, 0)
                                                }
                                            }
                                        }
                                        None => (goal, 0),
                                    }
                                } else {
                                    (goal, 0)
                                };

                                // 闸门分支：模型在 Active 续跑下自报 Complete，
                                // 但客观闸门（stop_hooks 退出码）尚未确认。
                                // 在接受 complete 为终止前先跑闸门（Ralph
                                // Wiggum 营救）。结构化退出码，零 LLM 调用（R7）。
                                let gate_configured =
                                    cont_deps.gate.is_some() || goal.gate_command.is_some();
                                if crate::tasks::goal_pursuit::awaiting_gate(&goal, gate_configured)
                                {
                                    let gate = crate::verification::stop_hooks::effective_gate(
                                        cont_deps.gate.as_ref(),
                                        goal.gate_command.as_deref(),
                                    )
                                    .ok_or_else(|| {
                                        ExecutionError::Failed(
                                            "configured gate resolved to none".into(),
                                        )
                                    })?;
                                    let hctx = StopHookContext {
                                        final_text: Some(goal.objective.clone()),
                                        iterations: goal.continuations_used as usize,
                                        tool_calls_made: 0,
                                        stop_reason: "goal_complete_claim".to_string(),
                                    };
                                    let result = execute_stop_hooks_arc(
                                        &gate,
                                        &hctx,
                                        &CancellationToken::new(),
                                    )
                                    .await;
                                    let vetoed =
                                        result.halt_reason().or_else(|| result.blocking_reason());
                                    match vetoed {
                                        None => {
                                            // 闸门通过 → 确认完成，循环终止。
                                            let confirmed =
                                                crate::tasks::goal_pursuit::confirm_complete(
                                                    &goal, now_ms,
                                                );
                                            if let Err(e) = store.put(&confirmed) {
                                                warn!(error = %e, session = %session_key_str,
                                                    "goal pursuit: failed to persist gate confirmation");
                                            } else {
                                                info!(session = %session_key_str,
                                                    "goal pursuit: objective gate passed, goal verified complete");
                                                // Gate-confirmed complete is an
                                                // authoritative end-point: clear
                                                // the welded Strategy so it does
                                                // not bleed into a later plain
                                                // turn in this reused session
                                                // (spec §6). Best-effort.
                                                clear_goal_welded_strategy(&session_key_str);
                                            }
                                        }
                                        Some(reason) => {
                                            // 闸门否决 → 退回 Active(或 Blocked)。
                                            let reopened =
                                                crate::tasks::goal_pursuit::reopen_after_gate_failure(
                                                    &goal, reason, now_ms,
                                                );
                                            let reopened_active = reopened.is_active();
                                            if let Err(e) = store.put(&reopened) {
                                                warn!(error = %e, session = %session_key_str,
                                                    "goal pursuit: failed to persist gate veto");
                                            } else if reopened_active {
                                                let bumped =
                                                    reopened.clone().spent_continuation(now_ms);
                                                if let Err(e) = store.put(&bumped) {
                                                    warn!(error = %e, session = %session_key_str,
                                                        "goal pursuit: failed to persist continuation counter after veto");
                                                } else {
                                                    let prompt =
                                                        crate::tasks::goal_pursuit::gate_failure_prompt(
                                                            &goal, reason,
                                                        );
                                                    info!(session = %session_key_str,
                                                        "goal pursuit: objective gate vetoed completion, re-running with feedback");
                                                    spawn_continuation_run(
                                                        cont_deps.registry.clone(),
                                                        cont_deps.adapter.clone(),
                                                        request.session_key.clone(),
                                                        session_key_str.clone(),
                                                        prompt,
                                                        cont_deps.event_bus.clone(),
                                                        None,
                                                        ContinuationKind::Goal,
                                                    );
                                                }
                                            } else {
                                                // Vetoed with no runway left → the
                                                // reopen path Blocked the goal. This
                                                // is an authoritative dormant end —
                                                // clear the welded plan (the failed
                                                // plan must not bleed into later
                                                // plain turns; a user resume re-reasons).
                                                clear_goal_welded_strategy(&session_key_str);
                                                info!(session = %session_key_str,
                                                    "goal pursuit: objective gate vetoed at iteration cap, goal blocked");
                                            }
                                        }
                                    }
                                } else if crate::tasks::goal_pursuit::should_continue(
                                    &goal, tokens_now, now_ms,
                                ) {
                                    let bumped = goal.clone().spent_continuation(now_ms);
                                    if let Err(e) = store.put(&bumped) {
                                        warn!(error = %e, session = %session_key_str,
                                            "goal pursuit: failed to persist continuation counter; skipping");
                                    } else {
                                        let prompt =
                                            crate::tasks::goal_pursuit::continuation_prompt(&goal);
                                        spawn_continuation_run(
                                            cont_deps.registry.clone(),
                                            cont_deps.adapter.clone(),
                                            request.session_key.clone(),
                                            session_key_str.clone(),
                                            prompt,
                                            cont_deps.event_bus.clone(),
                                            None,
                                            ContinuationKind::Goal,
                                        );
                                        info!(session = %session_key_str,
                                            continuations_used = bumped.continuations_used,
                                            "goal pursuit: enqueued autonomous continuation");
                                    }
                                } else if crate::tasks::goal_pursuit::exhausted_while_active(
                                    &goal, tokens_now, now_ms,
                                ) {
                                    // Distinguish token-budget / wall-clock
                                    // exhaustion from the iteration cap so the user
                                    // sees the real stop reason on their next turn.
                                    let note = if goal.over_budget(tokens_now) {
                                        crate::tasks::goal_pursuit::budget_reached_note(&goal)
                                    } else if goal
                                        .deadline_ms
                                        .is_some_and(|d| now_ms != 0 && now_ms > d)
                                    {
                                        crate::tasks::goal_pursuit::deadline_reached_note(&goal)
                                    } else {
                                        crate::tasks::goal_pursuit::cap_reached_note(&goal)
                                    };
                                    let blocked = goal
                                        .clone()
                                        .with_status(crate::goal::GoalStatus::Blocked, now_ms)
                                        .with_note(Some(note.clone()), now_ms);
                                    if let Err(e) = store.put(&blocked) {
                                        warn!(error = %e, session = %session_key_str,
                                            "goal pursuit: failed to persist cap-reached block");
                                    } else {
                                        // Authoritative dormant end — clear the
                                        // welded plan (mirror loop's cap-stop) so
                                        // it does not bleed into later plain turns.
                                        clear_goal_welded_strategy(&session_key_str);
                                        // R5: a Blocked goal is invisible in the
                                        // prompt (only Active surfaces), so the
                                        // most common autonomous ending — hitting
                                        // a cap — was silent. Ping the origin
                                        // channel like loop's cap-exhaustion stop;
                                        // Panel-only sessions rely on the stored note.
                                        let origin = match crate::gateway::event_emitter::origin_fanout::channel_registry() {
                                            Some(reg) => agent
                                                .origin_route(&request.session_key)
                                                .await
                                                .map(|(ch, conv)| (reg, ch, conv)),
                                            None => None,
                                        };
                                        notify_origin(origin.as_ref(), format!("⏹ {note}")).await;
                                        info!(session = %session_key_str,
                                            continuations_used = goal.continuations_used,
                                            "goal pursuit: cap reached, goal blocked for user guidance");
                                    }
                                } else if goal.status == crate::goal::GoalStatus::Complete
                                    && matches!(goal.pursuit, crate::goal::PursuitMode::Active { .. })
                                {
                                    // Active-pursuit goal the model self-reported
                                    // Complete with NO gate to arbitrate it (a gate
                                    // would have routed through `awaiting_gate`
                                    // above, which owns the weld clear on confirm).
                                    // This is the common gate-less autonomous
                                    // completion — an authoritative terminal end,
                                    // so clear the welded plan here (idempotent for
                                    // the already-gate-Passed case).
                                    clear_goal_welded_strategy(&session_key_str);
                                }
                            }
                            Ok(None) => {} // No goal for this session — common path, silent.
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    session = %request.session_key.to_key_string(),
                                    "goal pursuit: goal store lookup failed"
                                );
                            }
                        }
                    }

                    // Loop continuation hook: the clock-gated sibling of the
                    // goal hook above. The whole post-run decision — lazy
                    // token-baseline capture, in-flight-tick dedupe, cap
                    // enforcement, counter bump — is ONE atomic registry call
                    // (`try_claim_tick`), so a stop/update racing this hook can
                    // never be clobbered, and two completing runs (this hook
                    // fires after EVERY run in the session, user turns
                    // included) can never each enqueue a parallel tick chain.
                    if let Some(loop_reg) = crate::looping::global() {
                        let session_key_str = request.session_key.to_key_string();
                        if let Some(state) = loop_reg.get_active(&session_key_str) {
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_millis() as u64);

                            // Live cumulative token total, read (await) BEFORE
                            // the atomic claim; only needed when the loop
                            // carries a budget — the common no-budget path
                            // reads no session state. `None` → budget
                            // unenforced this tick (iteration/deadline caps
                            // still apply inside the claim).
                            let tokens_total = if state.token_budget.is_some() {
                                match self.session_manager.as_ref() {
                                    Some(sm) => {
                                        match sm.get_total_tokens(&request.session_key).await {
                                            Ok(total) => total,
                                            Err(e) => {
                                                warn!(error = %e, session = %session_key_str,
                                                    "loop: session token read failed; budget unenforced this tick");
                                                None
                                            }
                                        }
                                    }
                                    None => None,
                                }
                            } else {
                                None
                            };

                            match loop_reg.try_claim_tick(&session_key_str, tokens_total, now_ms) {
                                crate::looping::TickDecision::Fire {
                                    delay_ms,
                                    wake_ms,
                                    prompt,
                                } => {
                                    spawn_continuation_run(
                                        cont_deps.registry.clone(),
                                        cont_deps.adapter.clone(),
                                        request.session_key.clone(),
                                        session_key_str.clone(),
                                        prompt,
                                        cont_deps.event_bus.clone(),
                                        Some(delay_ms),
                                        ContinuationKind::Loop { wake_ms },
                                    );
                                    info!(session = %session_key_str, delay_ms,
                                        "loop: enqueued next tick");
                                }
                                crate::looping::TickDecision::Exhausted { note } => {
                                    // The claim already stored Stopped + the
                                    // stop reason for loop(action='status').
                                    // Mirror the tool-stop cleanup here too:
                                    // clear the loop-welded Strategy so the
                                    // stale plan does not bleed into every
                                    // later turn of this reused session (and a
                                    // future start can re-plan).
                                    if let Some(strat) = crate::strategy::global() {
                                        if let Err(e) = strat
                                            .delete(&crate::strategy::loop_key(&session_key_str))
                                        {
                                            info!(session = %session_key_str, error = %e,
                                                "loop: failed to delete welded strategy on cap stop (ignored)");
                                        }
                                    }
                                    // Cap stops were the one silent ending —
                                    // the failure path already notifies. Tell
                                    // the origin channel (R5); Panel-only
                                    // sessions rely on the stored stop_reason.
                                    let origin = match crate::gateway::event_emitter::origin_fanout::channel_registry() {
                                        Some(reg) => agent
                                            .origin_route(&request.session_key)
                                            .await
                                            .map(|(ch, conv)| (reg, ch, conv)),
                                        None => None,
                                    };
                                    notify_origin(origin.as_ref(), format!("⏹ {note}")).await;
                                    info!(session = %session_key_str, note = %note,
                                        "loop: safety cap reached, loop stopped");
                                }
                                crate::looping::TickDecision::Idle => {}
                            }
                        }
                    }
                }

                Ok(())
            }
            Err(e) => {
                let task_status = match e {
                    ExecutionError::Cancelled => TaskStatus::Interrupted,
                    _ => TaskStatus::Failed,
                };
                if trace_task_persisted {
                    self.persist_run_task_status(&run_id, task_status).await;
                }

                // Render a user-facing receipt instead of the flattened
                // internal error chain: a stable code plus a short message that
                // tells the user whether retrying is worthwhile (rate-limited /
                // unreachable). The typed `e` is still returned below for
                // internal callers; only the channel presentation changes.
                let receipt = super::failure_receipt::FailureReceipt::from_error(&e);
                if let Err(emit_err) = emitter
                    .emit(StreamEvent::RunError {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        error: receipt.message,
                        error_code: Some(receipt.code.to_string()),
                    })
                    .await
                {
                    tracing::warn!(
                        run_id = %run_id,
                        error_code = %receipt.code,
                        error = %emit_err,
                        "failed to emit RunError stream event"
                    );
                }
                // Preserve the typed variant so downstream callers (cron / heartbeat
                // executors) can dispatch on `ExecutionError::Timeout`,
                // `Cancelled`, etc. Collapsing to `Failed(string)` here made the
                // dedicated Timeout arms unreachable and misclassified timeouts as
                // permanent failures.
                Err(e)
            }
        };

        // Clear the session-level "running" marker now that the final assistant
        // message (or error receipt) has been persisted. This keeps the session
        // metadata from staying stuck in "running" after the turn ends.
        agent.set_session_idle(&request.session_key).await;

        // Remove from active runs after a short delay (for status queries)
        let runs_clone = active_runs.clone();
        let run_id_clone = run_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            runs_clone.write().await.remove(&run_id_clone);
        });

        // codex `maybe_start_turn_for_pending_work` parity: a steering message
        // that landed after the harness's final follow-up check — but before
        // this run's state flipped out of `Running` above — was acknowledged
        // into the session log yet has no live loop left to answer it. The
        // state flip makes further injections impossible, so one bounded
        // re-read here closes the race: if an unanswered burst remains,
        // re-drive the loop over the existing log via the resume flow,
        // reusing this run's emitter so the answer reaches the same client.
        // Deliberately limited to `Completed` runs — a cancelled run means the
        // user asked the agent to stop, so pending messages wait for the next
        // interaction instead of restarting the loop they just killed.
        if matches!(final_state, RunState::Completed) {
            if let Some(rescue) =
                super::steering::build_steering_rescue_request(self.orchestrator.as_ref(), &request)
                    .await
            {
                info!(
                    session = %request.session_key.to_key_string(),
                    rescue_run = %rescue.run_id,
                    "post-run steering rescue: unanswered steering burst detected; re-driving loop",
                );
                // The rescue's outcome must NOT replace the original run's
                // result: the original message was already answered, and
                // propagating e.g. a rescue `AgentBusy` (another run grabbed
                // the freed slot — it reads the same log and covers the
                // burst) would make the inbound router's busy/retry loop
                // re-execute the already-answered original message. Log and
                // swallow; the burst is never dropped — at worst it defers to
                // the next interaction. Box::pin breaks the infinitely-sized
                // recursive future.
                if let Err(e) = Box::pin(self.execute(rescue, agent, emitter)).await {
                    warn!(
                        session = %request.session_key.to_key_string(),
                        error = %e,
                        "post-run steering rescue run failed; burst defers to next interaction",
                    );
                }
            }
        }

        final_result
    }
}

/// Which subsystem scheduled an autonomous continuation. Decides how a *run
/// failure* is routed: a goal continuation blocks the goal for user guidance,
/// a loop tick stops the clock-driven loop. Both leave a user-*cancelled* run
/// untouched so the next interaction resumes. Without this, the shared failure
/// path blocked only goals — a failed loop tick left the loop a stuck `Active`
/// that never re-fired (silent stall).
#[derive(Clone, Copy, Debug)]
enum ContinuationKind {
    Goal,
    /// A clock-driven loop tick. Carries the wake stamped by
    /// `LoopRegistry::try_claim_tick`; at fire time the tick must
    /// `confirm_fire` with it — a mismatch means the loop was stopped or
    /// replaced during the delay and the tick must not execute.
    Loop { wake_ms: u64 },
}

/// 入队一次自主续跑 run（同一 session、同一 agent，给定 prompt）。
/// 被 goal 续跑（`should_continue` / gate-failure）与 loop tick 续跑共用——
/// 消除重复的 `RunRequest` 构造与 `tokio::spawn` 样板。`kind` 路由失败处理。
fn spawn_continuation_run(
    registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    adapter: Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>,
    session_key: crate::routing::session_key::SessionKey,
    session_key_str: String,
    prompt: String,
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    delay_ms: Option<u64>,
    kind: ContinuationKind,
) {
    let cont_request = super::RunRequest {
        run_id: uuid::Uuid::new_v4().to_string(),
        // Cloned (not moved) so the AgentBusy retry below can re-enqueue the
        // SAME tick prompt without recomputing it.
        input: prompt.clone(),
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata: {
            // Unattended security-tax: this autonomous run has no human on the
            // channel to approve anything. The per-run ScopedToolService reads
            // this marker and fails closed on confirm-gated tools.
            let mut m = std::collections::HashMap::new();
            m.insert("unattended".to_string(), "true".to_string());
            m
        },
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    };
    let cont_agent_id = session_key.agent_id().to_string();
    tokio::spawn(async move {
        // Loop cadence: wait the requested delay before this tick fires. Goal
        // continuations pass None (immediate); loop ticks pass Some(interval).
        if let Some(d) = delay_ms {
            tokio::time::sleep(std::time::Duration::from_millis(d)).await;
        }
        // Fire-time gate: the registry state may have changed during the
        // (minutes-long) sleep — loop(stop), a cap stop, or a fresh start all
        // supersede this tick. Without the atomic confirm, a stopped loop
        // still burned one full stale LLM turn (ghost tick).
        if let ContinuationKind::Loop { wake_ms } = kind {
            let confirmed = crate::looping::global()
                .is_some_and(|reg| reg.confirm_fire(&session_key_str, wake_ms));
            if !confirmed {
                info!(session = %session_key_str,
                    "loop: tick superseded during its delay (loop stopped or replaced); skipping");
                return;
            }
        }
        let Some(cont_agent) = registry.get(&cont_agent_id).await else {
            warn!(
                agent_id = %cont_agent_id,
                session = %session_key_str,
                "goal pursuit: agent not found, skipping continuation"
            );
            return;
        };
        // G1: resolve the session's bound origin channel once — used both to
        // fan the continuation's final reply out to it (Telegram/Slack) and,
        // on failure, to deliver the halt notice (G3). `None` for Panel-only
        // (`gui:chat`) sessions, which still get live event-bus streaming.
        let origin: Option<(
            Arc<crate::gateway::channel_registry::ChannelRegistry>,
            String,
            String,
        )> = match crate::gateway::event_emitter::origin_fanout::channel_registry() {
            Some(reg) => cont_agent
                .origin_route(&session_key)
                .await
                .map(|(ch, conv)| (reg, ch, conv)),
            None => None,
        };
        // Kept for the AgentBusy retry re-spawn (the original is consumed by
        // the emitter construction just below).
        let retry_bus = event_bus.clone();
        // G1: broadcast the continuation live (Panel + `aleph watch`) via the
        // gateway event bus when one is wired; fall back to collect-and-drop in
        // tests / non-gateway contexts so those paths stay behavior-identical.
        let base: Arc<dyn EventEmitter + Send + Sync> = match event_bus {
            Some(bus) => Arc::new(crate::gateway::event_emitter::GatewayEventEmitter::new(bus)),
            None => Arc::new(crate::gateway::event_emitter::CollectingEventEmitter::new()),
        };
        // Mirror handlers::agent / subagent_announce: fan the final reply out to
        // the origin channel when one is bound (delivery errors are swallowed by
        // the decorator — a failed delivery must never mis-mark goal progress).
        let emitter: Arc<dyn EventEmitter + Send + Sync> = match &origin {
            Some((reg, ch, conv)) => Arc::new(
                crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter::new(
                    base,
                    reg.clone(),
                    ch.clone(),
                    conv.clone(),
                ),
            ),
            None => base,
        };
        if let Err(e) = adapter.execute(cont_request, cont_agent, emitter).await {
            // A cancelled run means the user interrupted — leave the goal/loop
            // as-is so the next interaction resumes (same rationale as the
            // post-run steering rescue's Completed-only guard). Any other
            // failure ends the silent stall: block the goal / stop the loop and
            // notify, so unattended work never dies as a stuck `Active`.
            if matches!(e, ExecutionError::Cancelled) {
                info!(session = %session_key_str, kind = ?kind,
                    "continuation cancelled by user");
            } else if matches!(e, ExecutionError::AgentBusy(_)) {
                // A scheduling collision, not a pursuit failure: another run
                // of this agent held the slot when the tick fired — possibly
                // in a DIFFERENT session, whose completion re-enters the hook
                // under its own key and would never re-arm THIS loop. The old
                // behavior turned the collision into a terminal stop with a
                // confusing "halted by an error: Agent is busy" receipt; a
                // bare skip would instead strand the loop silently dormant.
                // So: re-arm the SAME claimed tick with a short retry delay
                // (atomic in the registry, no iteration bump) and re-enqueue.
                // Goals keep the visible blocking path below — they have no
                // pending/claim machinery to retry through, and a Blocked
                // goal is user-recoverable and notified.
                match kind {
                    ContinuationKind::Loop { .. } => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_millis() as u64);
                        let rearmed = crate::looping::global()
                            .and_then(|r| r.rearm_after_busy(&session_key_str, now));
                        match rearmed {
                            Some((delay_ms, wake_ms)) => {
                                info!(session = %session_key_str, delay_ms,
                                    "loop: agent busy at tick fire; re-armed the tick with a retry delay");
                                spawn_continuation_run(
                                    registry.clone(),
                                    adapter.clone(),
                                    session_key.clone(),
                                    session_key_str.clone(),
                                    prompt.clone(),
                                    retry_bus.clone(),
                                    Some(delay_ms),
                                    ContinuationKind::Loop { wake_ms },
                                );
                            }
                            None => info!(session = %session_key_str,
                                "loop: agent busy at tick fire; loop no longer active, re-claimed, or capped — tick dropped"),
                        }
                    }
                    ContinuationKind::Goal => {
                        // A scheduling collision, not a pursuit failure — the
                        // loop sibling above treats it as benign, and goals now
                        // do too. The busy holder is another run of this agent
                        // (commonly a user turn racing this session's own
                        // continuation): its completion re-enters the goal hook
                        // under the SAME session key and re-arms the pursuit. So
                        // leave the goal Active rather than permanently Blocking
                        // it — a user simply chatting while a goal pursues must
                        // not spuriously kill the work they asked for (loop's
                        // "视同 Cancelled 留 Active" parity, R5).
                        info!(session = %session_key_str,
                            "goal pursuit: continuation hit a busy agent; leaving goal active, next completed run re-arms it");
                    }
                }
            } else {
                match kind {
                    ContinuationKind::Goal => {
                        warn!(error = %e, session = %session_key_str,
                            "goal pursuit: continuation run failed; blocking goal for user guidance");
                        block_goal_on_failure(&session_key_str, &e, origin.as_ref()).await;
                    }
                    ContinuationKind::Loop { .. } => {
                        warn!(error = %e, session = %session_key_str,
                            "loop: tick run failed; stopping loop for user guidance");
                        stop_loop_on_failure(&session_key_str, &e, origin.as_ref()).await;
                    }
                }
            }
        }
    });
}

/// Clear the goal-welded Strategy for a session (best-effort). Called at every
/// authoritative goal termination — gate-confirmed complete, cap/deadline/
/// budget exhaustion, gate-veto-at-cap, and failure block — so the stale plan
/// neither bleeds into later plain turns of the reused session (the goal tier is
/// resolved FIRST in `resolve_active_strategy`) nor blocks a future same-
/// objective `goal set` from re-planning. The loop sibling clears `loop_key`
/// inline at its cap-stop / failure-stop; this is goal's single-source
/// equivalent so those sites do not each hand-roll the delete. The tool-side
/// terminations (`goal clear`, self-reported terminal `update`) clear the same
/// key in `builtin_tools/goal.rs`; only Passive-complete and Blocked are cleared
/// there — Active-pursuit completion is gate-arbitrated here so a gate veto can
/// still reopen the goal WITH its plan intact.
fn clear_goal_welded_strategy(session_key_str: &str) {
    if let Some(strat) = crate::strategy::global() {
        if let Err(e) = strat.delete(&crate::strategy::goal_key(session_key_str)) {
            info!(session = %session_key_str, error = %e,
                "goal pursuit: failed to clear welded strategy on termination (ignored)");
        }
    }
}

/// G3: when an autonomous continuation fails (non-cancellation), transition the
/// session's goal to `Blocked` with the error and best-effort notify the origin
/// channel. Without this, a transient failure leaves the goal a stuck `Active`
/// with no in-flight run — silent stall — and a `Blocked` goal is invisible in
/// the prompt (`active_standing_goal` only surfaces `Active`), so the channel
/// notice is the user's signal that unattended pursuit halted.
async fn block_goal_on_failure(
    session_key_str: &str,
    error: &ExecutionError,
    origin: Option<&(
        Arc<crate::gateway::channel_registry::ChannelRegistry>,
        String,
        String,
    )>,
) {
    let reason: String = format!("{error}").chars().take(300).collect();
    if let Some(store) = crate::goal::global() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        match store.get(session_key_str) {
            // Only block a goal still actively being pursued — never clobber a
            // goal the failed run had already marked complete/blocked.
            Ok(Some(goal)) if goal.is_active() => {
                let note = format!(
                    "Autonomous pursuit was halted by an error and blocked for your \
                     guidance: {reason}. Review progress, then clear or re-set the \
                     goal to continue."
                );
                let blocked = goal
                    .with_status(crate::goal::GoalStatus::Blocked, now_ms)
                    .with_note(Some(note), now_ms);
                if let Err(e) = store.put(&blocked) {
                    warn!(error = %e, session = %session_key_str,
                        "goal pursuit: failed to persist failure block");
                } else {
                    // Authoritative dormant end — clear the welded plan so the
                    // failed plan does not bleed into later plain turns (mirror
                    // the cap-stop / loop failure-stop cleanup).
                    clear_goal_welded_strategy(session_key_str);
                }
            }
            // Already terminal (complete/blocked/paused) or no goal → nothing
            // to block. A store error is logged, not silently swallowed.
            Ok(_) => {}
            Err(e) => warn!(error = %e, session = %session_key_str,
                "goal pursuit: goal lookup failed during failure block"),
        }
    }
    notify_origin(
        origin,
        format!("⚠️ Autonomous pursuit of your standing goal halted: {reason}"),
    )
    .await;
}

/// Loop sibling of [`block_goal_on_failure`]: when a clock-driven loop tick run
/// fails (non-cancellation), mark the loop `Stopped` and best-effort notify the
/// origin channel. Without this, a failed tick left the loop a stuck `Active`
/// in the registry that the continuation hook never re-fired (it only runs on a
/// run's success path) — a silent stall where `loop(action='status')` lies.
async fn stop_loop_on_failure(
    session_key_str: &str,
    error: &ExecutionError,
    origin: Option<&(
        Arc<crate::gateway::channel_registry::ChannelRegistry>,
        String,
        String,
    )>,
) {
    let reason: String = format!("{error}").chars().take(300).collect();
    if let Some(reg) = crate::looping::global() {
        // Only stop a loop still Active — never clobber one the user already
        // stopped between the tick firing and its failure landing. Store the
        // failure reason so loop(action='status') explains the halt even for
        // Panel-only sessions that have no origin channel to receive the notice.
        if let Some(state) = reg.get_active(session_key_str) {
            reg.put(
                state
                    .with_status(crate::looping::LoopStatus::Stopped)
                    .with_stop_reason(Some(format!("Halted by an error: {reason}"))),
            );
            // Mirror the tool-stop cleanup (loop_manage stop): clear the
            // loop-welded Strategy so the stale plan neither bleeds into
            // later turns nor blocks a future start from re-planning.
            if let Some(strat) = crate::strategy::global() {
                if let Err(e) = strat.delete(&crate::strategy::loop_key(session_key_str)) {
                    info!(session = %session_key_str, error = %e,
                        "loop: failed to delete welded strategy on failure stop (ignored)");
                }
            }
        }
    }
    notify_origin(
        origin,
        format!("⚠️ Your timer loop halted on an error and was stopped: {reason}"),
    )
    .await;
}

/// Best-effort deliver a continuation stop/halt notice to the session's bound
/// origin channel (R5). Shared by both the loop endings (failure stop +
/// cap-exhaustion stop) and the goal endings (failure block + cap/deadline/
/// budget exhaustion block) so every unattended ending is equally visible;
/// Panel-only sessions have no origin and rely on the stored stop reason
/// (loop) / blocked note (goal).
async fn notify_origin(
    origin: Option<&(
        Arc<crate::gateway::channel_registry::ChannelRegistry>,
        String,
        String,
    )>,
    text: String,
) {
    if let Some((reg, ch, conv)) = origin {
        let msg = crate::gateway::channel::OutboundMessage::text(conv.clone(), text);
        if let Err(e) = reg
            .send(&crate::gateway::channel::ChannelId::new(ch.clone()), msg)
            .await
        {
            warn!(channel = %ch, error = %e, "continuation: failed to deliver origin notice");
        }
    }
}

#[cfg(test)]
mod naked_loop_planner_tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    #[test]
    fn gate_fires_for_human_first_message() {
        let k = SessionKey::main("a");
        assert!(naked_loop_planner_should_fire(
            &k,
            true,
            false,
            "research X and email Bob"
        ));
    }

    #[test]
    fn gate_excludes_slash_command_input() {
        // `/loop`·`/goal` fall through the fast path on purpose (their first
        // tick/pursuit is scheduled by the completion hook) — the naked-loop
        // planner must not plan over raw slash syntax; they run their own
        // planners, and double-planning would weld a stale session Strategy.
        let k = SessionKey::main("a");
        assert!(!naked_loop_planner_should_fire(
            &k,
            true,
            false,
            "/loop watch CI every 5m"
        ));
        assert!(!naked_loop_planner_should_fire(&k, true, false, "  /goal x"));
    }

    #[test]
    fn gate_excludes_non_first_message() {
        let k = SessionKey::main("a");
        assert!(!naked_loop_planner_should_fire(&k, false, false, "hi"));
    }

    #[test]
    fn gate_excludes_automated_origins() {
        // cron + group-chat member runs use Task keys; subagent/ephemeral too.
        let cron = SessionKey::task("a", "cron", "job-1");
        let team = SessionKey::task("a", "team_chat", "team-1");
        let eph = SessionKey::ephemeral("a");
        let sub = SessionKey::subagent(SessionKey::main("a"), "s");
        assert!(!naked_loop_planner_should_fire(
            &cron,
            true,
            false,
            "do a thing"
        ));
        assert!(!naked_loop_planner_should_fire(
            &team,
            true,
            false,
            "do a thing"
        ));
        assert!(!naked_loop_planner_should_fire(
            &eph,
            true,
            false,
            "do a thing"
        ));
        assert!(!naked_loop_planner_should_fire(
            &sub,
            true,
            false,
            "do a thing"
        ));
    }

    #[test]
    fn gate_excludes_resume_and_empty() {
        let k = SessionKey::main("a");
        assert!(!naked_loop_planner_should_fire(&k, true, true, "x")); // resume
        assert!(!naked_loop_planner_should_fire(&k, true, false, "   ")); // whitespace
    }

    #[test]
    fn bounded_objective_caps_utf8_safely() {
        let long = "字".repeat(5000); // multi-byte chars
        let out = bounded_objective(&long);
        assert_eq!(out.chars().count(), 4000);
        assert!(out.is_char_boundary(out.len()));
    }
}

#[cfg(test)]
mod moa_fallthrough_input_tests {
    use super::moa_fallthrough_input;

    #[test]
    fn moa_fallthrough_input_semantics() {
        // Plain one-shot: strip the `/moa ` wrapper, keep the whole remainder
        // as the prompt (hermes-pinned: the arg is ALWAYS a prompt).
        assert_eq!(
            moa_fallthrough_input("/moa write a poem"),
            Some("write a poem".to_string())
        );
        assert_eq!(
            moa_fallthrough_input("/moa preset hello"),
            Some("preset hello".to_string())
        );
        // Nested slash remainder was never armed (round-1 guard, `185119c8e`)
        // and the channel path has no later slash resolution — leave the
        // input untouched.
        assert_eq!(moa_fallthrough_input("/moa /help"), None);
        // Bare `/moa` (no prompt) falls through to the LLM unchanged.
        assert_eq!(moa_fallthrough_input("/moa"), None);
        assert_eq!(moa_fallthrough_input("/moa   "), None);
        // Non-moa input is never rewritten.
        assert_eq!(moa_fallthrough_input("hello"), None);
        assert_eq!(moa_fallthrough_input("/moab x"), None);
    }
}
