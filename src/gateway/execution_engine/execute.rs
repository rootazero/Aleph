use super::gate::GateOutcome;
use super::{ExecutionEngine, ExecutionError, RunRequest, RunState};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::resilience::TaskStatus;
use crate::sync_primitives::Arc;
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
        let _run_slot = match self.admit_run(&request, &run_id, &agent, cancel_tx).await? {
            GateOutcome::Admitted(slot) => slot,
            GateOutcome::HandledInline => return Ok(()),
        };

        let trace_task_persisted = self
            .persist_run_task_started(&run_id, &request, &agent)
            .await;

        // Log lifecycle event: agent started
        info!(
            event_type = "agent.lifecycle.started",
            agent_id = %agent.id(),
            run_id = %run_id,
            "Agent execution started"
        );

        // Ensure session exists in memory + SQLite before adding messages.
        // Scoped: the CREATE branch stamps `owner_user_id`/`scope_id` from the
        // ambient scope, which no producer still has by here (they all spawn).
        // See `run_loop::ensure_session_under_request_scope`.
        super::run_loop::ensure_session_under_request_scope(&agent, &request).await;

        // Emit run accepted event — AFTER the row above exists, and this order
        // is load-bearing rather than tidy. `RunAccepted` classifies as
        // `SessionIdentity::BySessionKey`, so the delivery filter resolves it
        // by reading that row; emitted before the CREATE it lost the race and
        // was denied (correctly, and the denial is deliberately not cached —
        // `event_visibility.rs::an_absent_session_row_is_transient_not_a_cached_denial`).
        // But this is also the ONE frame carrying `{run_id, session_key}`: it
        // seeds `EventVisibilityIndex`'s run→session index, and the Panel binds
        // run→conversation on it alone (`chat/events.rs::resolve_target` drops
        // any other frame whose run id it cannot look up). Losing it therefore
        // did not cost one frame — it silently voided the ENTIRE first turn of
        // every new conversation on every client, operator included: chunks and
        // traces arrived and were dropped unrendered, and the reply only
        // appeared after a reload re-read the transcript (2026-08-09
        // real-machine QA). Re-emitting later cannot repair it either; the
        // Panel starts an assistant bubble on this frame, so a duplicate leaves
        // an empty one behind. Same reasoning as the republish below — one row,
        // two things that could not resolve without it.
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

        // The claim broadcast inside `admit_run` above went out BEFORE that row
        // existed, so the per-connection projection
        // (`event_visibility::EventVisibilityIndex::project_for`) could not
        // resolve this session key and dropped it — every Panel recorded an
        // empty running set at that seq. Re-publish now that the key resolves.
        // Without this the sidebar's running dot stays dark for the ENTIRE
        // first turn of every new conversation: for a single in-flight run the
        // only later frame is the release at run end, which excludes the key
        // too. See `SessionRunRegistry::republish_running_set` for why it must
        // carry a higher seq.
        self.session_run_registry.republish_running_set();

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
                &request.run_id,
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
                    // Round-3 F3: mirror the operator gate on the `moa` tool
                    // (method_authz). A non-operator (chat-tier channel) may
                    // not arm advisory via `/moa`; the prompt still runs as a
                    // normal turn (prefix stripped below).
                    if crate::tools::turn_context::role_is_operator(
                        request.metadata.get("caller_role").map(String::as_str),
                    ) {
                        // Round-3 F1: single arm source (one-shot). `None`
                        // preset = the config default; clears the model slot.
                        if let Err(msg) =
                            crate::providers::moa::activation::arm_one_shot(&key, None)
                        {
                            crate::builtin_tools::notify_tool_result("moa", &msg, false);
                        }
                    } else {
                        crate::builtin_tools::notify_tool_result(
                            "moa",
                            "MoA advisory requires operator; running normally.",
                            false,
                        );
                    }
                }
                request.input = prompt.to_string();
            }

            // Safety net for producers that never pass through a handler —
            // cron jobs, heartbeat, team dispatch, goal/loop continuations —
            // and so cannot stamp before the busy lane. `chat.send` and
            // `agent.run` stamp earlier (they must: see `stamp_slash_mode`),
            // and this call is a no-op for them.
            self.stamp_slash_mode(&request.input, &mut request.metadata)
                .await;
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

                let occupancy = occupancy_out.lock().ok().and_then(|g| g.clone());
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
                            input_tokens: 0,
                            output_tokens: 0,
                            cost_usd: None,
                            model: None,
                            model_provider: None,
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
                                input_tokens: occ.input_tokens,
                                output_tokens: occ.output_tokens,
                                cost_usd: occ.cost_usd,
                                model: occ.model,
                                model_provider: occ.model_provider,
                                at: crate::session::events::now_ms(),
                            },
                        )
                        .await;
                }

                // Notify UI that the session was updated (global bus, so
                // channel-originated runs reach the Panel too). RunComplete
                // itself is single-sourced from the orchestrator drain.
                self.announce_turn_end(&request);

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
                // Record conversation turn for compression scheduling. Content-blind:
                // the turn-threshold cadence only. "Was this a correction worth
                // remembering?" is the model's call, via flag_user_correction (R7).
                if let Some(ref cs) = self.compression_service {
                    cs.record_turn_and_maybe_compress();
                }

                // Async session compaction (hierarchical summarization).
                // Capture this turn's project root AND scope attribution
                // BEFORE the spawn: neither the `projects::run_context` nor
                // the `crate::scope` task-local crosses a `tokio::spawn`
                // boundary (and the run-loop's own scopes are already closed
                // here), so resolving the storage agent id inside the spawned
                // task would always fall back to the base/org id while the
                // in-turn readers resolve the project- or personal-scoped id
                // — writes and reads would silently split. Mirrors the
                // workspace capture in the goal_continuation hook below.
                if let Some(ref sc) = self.session_compactor {
                    let sc = sc.clone();
                    let agent_clone = agent.clone();
                    let session_key_clone = request.session_key.clone();
                    let project_root = request.workspace_override.clone();
                    let scope_attr = crate::scope::current_scope();
                    tokio::spawn(crate::scope::with_scope(scope_attr, async move {
                        if let Err(e) = sc
                            .post_turn_compress(
                                &agent_clone,
                                &session_key_clone,
                                project_root.as_deref(),
                            )
                            .await
                        {
                            warn!(error = %e, "Session compaction failed");
                        }
                    }));
                }

                // Autonomous-continuation hook (R7/R10-safe, opt-in).
                // Fires only when the session has a standing goal with
                // `PursuitMode::Active` that still needs more work. The whole
                // post-run decision — lazy token-baseline capture, in-flight
                // dedupe, cap enforcement, iteration bump — is ONE atomic store
                // call (`GoalStore::try_claim_continuation`) inside
                // `goal_continuation`: the clock-less sibling of the loop's
                // `try_claim_tick` just below.
                if let Some(cont_deps) = self.continuation_deps.get() {
                    // Both continuation chains inherit this turn's policy inputs;
                    // chaining is transitive, since continuation N's own post_run
                    // reads them back out of N's metadata.
                    let policy_meta = carry_policy_metadata(&request.metadata);
                    // …and this turn's project root. It is NOT recoverable later:
                    // neither `goal` nor `looping` persists it, and
                    // `projects::run_context` is already closed by the time this
                    // hook runs. Without it an unattended continuation silently
                    // drops the project's CLAUDE.md / AGENTS.md and project
                    // skills, and its workspace_directive tells the model its cwd
                    // is the agent workspace.
                    let workspace = request.workspace_override.as_deref();
                    super::goal_continuation::post_run(
                        cont_deps,
                        &self.session_manager,
                        &request.session_key,
                        &agent,
                        &policy_meta,
                        workspace,
                    )
                    .await;

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
                                        policy_meta.clone(),
                                        request.workspace_override.clone(),
                                        cont_deps.event_bus.clone(),
                                        self.session_manager.clone(),
                                        Some(delay_ms),
                                        ContinuationKind::Loop { wake_ms },
                                    );
                                    info!(session = %session_key_str, delay_ms,
                                        "loop: enqueued next tick");
                                }
                                crate::looping::TickDecision::Exhausted { note } => {
                                    // The claim already stored Stopped + the
                                    // stop reason for loop(action='status').
                                    // Mirror the tool-stop cleanup here too so
                                    // the stale plan does not bleed into every
                                    // later turn of this reused session (and a
                                    // future start can re-plan).
                                    clear_loop_welded_strategy(&session_key_str, "cap stop");
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
                // `ExecutionError::user_receipt` is the single source of truth,
                // shared with the bin-crate RPC handlers (agent.run / chat.send).
                let (error_code, error_message) = e.user_receipt(
                    crate::gateway::i18n::Locale::from_run_metadata(&request.metadata),
                );
                if let Err(emit_err) = emitter
                    .emit(StreamEvent::RunError {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        error: error_message,
                        error_code: Some(error_code.to_string()),
                        // Post-admission: `RunAccepted` already seeded the index.
                        session_key: None,
                    })
                    .await
                {
                    tracing::warn!(
                        run_id = %run_id,
                        error_code = %error_code,
                        error = %emit_err,
                        "failed to emit RunError stream event"
                    );
                }
                // The same terminal announcement the success arm makes. A run
                // that failed or was cancelled still MOVED the transcript: the
                // harness appended the user message before dispatch, and any
                // partial assistant text and the error receipt are persisted
                // too. Announcing only on success is what left every other
                // surface — a second Panel tab, a room peer, the sidebar row —
                // showing a session that visibly never changed.
                self.announce_turn_end(&request);
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
pub(super) enum ContinuationKind {
    /// An autonomous goal continuation. Carries the wake stamped by
    /// `GoalStore::try_claim_continuation`; at fire time it must `confirm_fire`
    /// with it — a mismatch means the goal was cleared, completed or re-claimed
    /// while this continuation waited out an `AgentBusy` retry, and it must not
    /// execute (the ghost run this kills would burn a full stale LLM turn).
    Goal { wake_ms: u64 },
    /// A clock-driven loop tick. Carries the wake stamped by
    /// `LoopRegistry::try_claim_tick`; at fire time the tick must
    /// `confirm_fire` with it — a mismatch means the loop was stopped or
    /// replaced during the delay and the tick must not execute.
    Loop { wake_ms: u64 },
}

/// The session's bound origin channel: `(registry, channel_id, conversation_id)`.
/// Named so the continuation helpers do not each spell out the 3-tuple.
pub(super) type OriginRoute = (
    Arc<crate::gateway::channel_registry::ChannelRegistry>,
    String,
    String,
);

/// The policy-bearing subset of a run's metadata — the keys an autonomous
/// continuation MUST inherit from the conversation that spawned it.
///
/// `caller_role` drives the channel exec-tier clamp (`turn_permissions`) and the
/// config-tool operator gate (`tools/scoped/dispatch.rs`, via `TurnContext`);
/// [`CHANNEL_TOOL_PERMISSIONS_KEY`] is the originating channel's tool-permission
/// layer. Both are RESTRICTIVE inputs, and a continuation used to build its
/// metadata from scratch — so a Chat-tier Telegram conversation clamped to `Auto`
/// on its interactive turns spawned a goal continuation that ran at the
/// unclamped global tier with nobody watching. Invariant: a background run is
/// never MORE privileged than the conversation that spawned it — and, per P1
/// data isolation, never LESS situated either: [`crate::scope::OWNER_META_KEY`]
/// / [`crate::scope::SCOPE_META_KEY`] (the owner/scope attribution stamped at
/// the originating request) must inherit unchanged, so a continuation's
/// `memory.project_scoped` / retrieval / compaction reads never fall back to
/// the unscoped or wrong-owner namespace.
///
/// Everything else is per-turn (locale / platform / busy-input mode / slash
/// mode) and is deliberately dropped. `channel_id` / `conversation_id` stay out
/// too: they are what make an approval routable, and an unattended run must keep
/// failing closed on approval-gated tools.
pub(super) fn carry_policy_metadata(
    src: &std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    [
        "caller_role",
        super::CHANNEL_TOOL_PERMISSIONS_KEY,
        crate::scope::OWNER_META_KEY,
        crate::scope::SCOPE_META_KEY,
    ]
    .iter()
    .filter_map(|k| src.get(*k).map(|v| ((*k).to_string(), v.clone())))
    .collect()
}

/// Metadata of an autonomous continuation run: the inherited policy layer plus
/// the [`UNATTENDED_KEY`] marker and a `Queue` busy-input mode.
///
/// Both markers are written LAST and are therefore unconditional — an inherited
/// key can never demote a continuation to "attended" (which would let it park
/// on an approval card that nobody is there to answer) nor re-arm it as
/// `Steer`/`Interrupt`.
///
/// `Queue` is load-bearing: without it a continuation colliding with a live
/// same-session run fell into the default `Steer` mode and its machine prompt
/// (tick directive / goal AUDIT contract) was INJECTED into the user's attended
/// turn as if the user had typed it — while the designed collision path
/// (`AgentBusy` → `rearm_loop_after_busy` / `rearm_goal_after_busy`, which
/// retries the SAME claimed step without burning an iteration) sat unreachable.
/// `Queue` makes the gate return `AgentBusy` immediately, which
/// [`spawn_continuation_run`]'s failure routing turns into exactly that rearm.
///
/// [`UNATTENDED_KEY`]: super::UNATTENDED_KEY
fn continuation_metadata(
    policy_meta: std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    let mut m = policy_meta;
    m.insert(super::UNATTENDED_KEY.to_string(), "true".to_string());
    m.insert(super::BUSY_INPUT_MODE_KEY.to_string(), "queue".to_string());
    m
}

/// Enqueue one autonomous continuation run (same session, same agent, given prompt).
/// Shared by goal continuations (`should_continue` / gate-failure) and loop-tick
/// continuations — eliminates the duplicate `RunRequest` construction and
/// `tokio::spawn` boilerplate. `kind` routes failure handling.
///
/// `policy_meta` is [`carry_policy_metadata`] of the originating request;
/// `workspace_override` is its project root (`None` = the agent's own
/// workspace). Both are inherited, not rebuilt: a continuation must be neither
/// more privileged NOR less situated than the conversation that spawned it —
/// dropping the root moved the run out of the project (no project CLAUDE.md /
/// AGENTS.md, no project skills) and told the model so in its
/// workspace_directive.
pub(super) fn spawn_continuation_run(
    registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    adapter: Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>,
    session_key: crate::routing::session_key::SessionKey,
    session_key_str: String,
    prompt: String,
    policy_meta: std::collections::HashMap<String, String>,
    workspace_override: Option<std::path::PathBuf>,
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    // A clone of the engine's shared session-store handle — the SAME instance
    // every `AgentInstance::session_store` clones from (see `start`'s single
    // `session_store` variable). Threaded through so the agent-miss branch
    // below can resolve an origin channel WITHOUT a live agent, for the one
    // ending (Goal `OutOfBounds`) that must notify but has nothing else to
    // notify with once the agent is gone. `ContinuationKind::Loop` ignores it
    // — it has no analogous origin-resolution need on this path.
    session_manager: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
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
        metadata: continuation_metadata(policy_meta.clone()),
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: workspace_override.clone(),
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
        // Fire-time gate: the stored state may have changed during the delay —
        // loop(stop) / a cap stop / a fresh start supersede a tick; goal(clear),
        // a completion, or a stale-grace re-claim supersede a continuation.
        // Without the atomic confirm, a superseded run still burned one full
        // stale LLM turn (the ghost run).
        //
        // `goal_out_of_bounds_note` defers the OutOfBounds notice past the
        // agent lookup below: the common-path origin resolution goes through
        // `cont_agent`, which does not exist yet at this point, and
        // duplicating that lookup here would diverge from the single
        // agent-miss handling underneath (which has its own store-based
        // fallback for exactly the case where `cont_agent` never shows up).
        let mut goal_out_of_bounds_note: Option<String> = None;
        match kind {
            ContinuationKind::Loop { wake_ms } => {
                let confirmed = crate::looping::global()
                    .is_some_and(|reg| reg.confirm_fire(&session_key_str, wake_ms));
                if !confirmed {
                    info!(session = %session_key_str,
                        "loop: tick superseded during its delay (loop stopped or replaced); skipping");
                    return;
                }
            }
            ContinuationKind::Goal { wake_ms } => {
                let decision = crate::goal::global().map(|store| {
                    store.confirm_fire(
                        &session_key_str,
                        wake_ms,
                        super::goal_continuation::now_ms(),
                    )
                });
                match decision {
                    Some(Ok(crate::goal::FireDecision::Proceed)) => {}
                    Some(Ok(crate::goal::FireDecision::OutOfBounds { note })) => {
                        goal_out_of_bounds_note = Some(note);
                    }
                    _ => {
                        info!(session = %session_key_str,
                            "goal pursuit: continuation superseded (goal cleared, completed or re-claimed); skipping");
                        return;
                    }
                }
            }
        }
        let Some(cont_agent) = registry.get(&cont_agent_id).await else {
            // The agent was deleted between claim and fire. A bare skip would
            // leave the goal/loop Active-forever: the pending marker was just
            // cleared by confirm_fire, claims only happen post-run, and no run
            // can ever happen on this session again — so nothing re-claims,
            // `goal(action='list')` keeps showing a live pursuit from every
            // other session (dishonest), and the welded strategy row leaks.
            // Terminate honestly per kind instead. The stored stop reason /
            // blocked note is the surviving signal (R5) either way.
            //
            // Compound race: the wake also landed out of bounds. `confirm_fire`
            // already persisted the deadline note on the (now Blocked) goal, but
            // that note would otherwise vanish from THIS run entirely — the
            // generic warn below only names "agent no longer exists", and the
            // Goal-kind block below is a no-op once the goal is already Blocked
            // (`block_if_active` requires Active). Origin resolution normally
            // goes through `cont_agent`, which does not exist in this branch —
            // but the origin lives in the session STORE, not the agent, and
            // `session_manager` is a clone of that same shared store, so
            // `origin_of_via_store` still resolves and pushes it. Only when
            // that handle or the session's origin metadata itself is
            // unavailable does this fall back to a log line.
            if let Some(note) = &goal_out_of_bounds_note {
                // Through the shared ladder, which keeps its own named floor
                // when nothing is reachable. `registry` is handed to it even
                // though the lookup just missed: the ladder's shape is the
                // single source, and one extra miss on a terminal path is
                // cheaper than a fourth divergent copy of it.
                if super::goal_continuation::notify_goal_stop(
                    &registry,
                    session_manager.as_ref(),
                    &session_key_str,
                    note,
                )
                .await
                {
                    info!(session = %session_key_str, note = %note,
                        "goal pursuit: wake landed past the wall-clock bound; goal blocked \
                         (agent also gone — notified via the session-store origin)");
                }
            }
            warn!(
                agent_id = %cont_agent_id,
                session = %session_key_str,
                kind = ?kind,
                "continuation: agent no longer exists — terminating instead of skipping"
            );
            match kind {
                ContinuationKind::Loop { .. } => {
                    if let Some(reg) = crate::looping::global() {
                        if matches!(
                            reg.transition(
                                &session_key_str,
                                crate::looping::LoopStatus::Stopped,
                                Some(format!("Halted: agent '{cont_agent_id}' no longer exists")),
                            ),
                            crate::looping::TransitionOutcome::Applied { .. }
                        ) {
                            // Mirror the tool-stop cleanup: clear the welded
                            // strategy so the stale plan cannot bleed into a
                            // future session under a recreated agent.
                            clear_loop_welded_strategy(&session_key_str, "agent-miss stop");
                        }
                    }
                }
                ContinuationKind::Goal { .. } => {
                    if let Some(store) = crate::goal::global() {
                        let note = format!(
                            "Autonomous pursuit halted: agent '{cont_agent_id}' no longer \
                             exists. Re-set the goal in a session of an existing agent to \
                             continue."
                        );
                        match store.block_if_active(
                            &session_key_str,
                            &note,
                            super::goal_continuation::now_ms(),
                        ) {
                            Ok(true) => {
                                super::goal_continuation::clear_goal_welded_strategy(
                                    &session_key_str,
                                );
                            }
                            Ok(false) => {}
                            Err(e) => warn!(error = %e, session = %session_key_str,
                                "goal pursuit: failed to block goal after agent deletion"),
                        }
                    }
                }
            }
            return;
        };
        if let Some(note) = goal_out_of_bounds_note {
            // The bound elapsed while this continuation waited. The store
            // already Blocked the goal; push the same notice the `Exhausted`
            // claim decision would (R5 — an autonomous ending must never be
            // silent), and drop the welded plan like every other terminal end.
            super::goal_continuation::clear_goal_welded_strategy(&session_key_str);
            info!(session = %session_key_str, note = %note,
                "goal pursuit: wake landed past the wall-clock bound; goal blocked");
            let origin = super::goal_continuation::origin_of(&cont_agent, &session_key).await;
            notify_origin(origin.as_ref(), format!("⏹ {note}")).await;
            return;
        }
        // G1: resolve the session's bound origin channel once — used both to
        // fan the continuation's final reply out to it (Telegram/Slack) and,
        // on failure, to deliver the halt notice (G3). `None` for Panel-only
        // (`gui:chat`) sessions, which still get live event-bus streaming.
        let origin: Option<OriginRoute> =
            match crate::gateway::event_emitter::origin_fanout::channel_registry() {
                Some(reg) => cont_agent
                    .origin_route(&session_key)
                    .await
                    .map(|(ch, conv)| (reg, ch, conv)),
                None => None,
            };
        // Kept for the AgentBusy retry re-spawn (the original is consumed by
        // the emitter construction just below). `policy_meta` must be carried
        // too — without it the FIRST busy-retry silently drops the inherited
        // clamp and channel permission layer. Same for the project root: a
        // busy-retried tick that rebuilds it as `None` lands outside the project
        // just as surely as the first spawn did.
        let retry_bus = event_bus.clone();
        let retry_policy_meta = policy_meta;
        let retry_workspace = workspace_override;
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
            // Set by whichever arm decided this continuation should fire again
            // after a wait — a busy-collision re-arm, or a transient-provider
            // park. Both go through the ONE `spawn_continuation_run` call at
            // the end of this block, so a second re-arm reason can never grow a
            // second, subtly different scheduler call.
            let mut respawn: Option<(u64, ContinuationKind)> = None;
            if matches!(e, ExecutionError::Cancelled) {
                info!(session = %session_key_str, kind = ?kind,
                    "continuation cancelled by user");
            } else if matches!(e, ExecutionError::AgentBusy(_)) {
                // A scheduling collision, not a pursuit failure: another run of
                // this session held the slot when this one fired (commonly a
                // queued user message that won the race for the just-freed
                // slot). The iteration was already spent at claim time, so
                // BOTH kinds re-arm the SAME claimed run with a short retry
                // delay (atomic in the store/registry, no second bump) and
                // re-enqueue. Dropping it instead — what goals used to do —
                // burned an autonomous step for zero work, and stalled the
                // pursuit outright whenever the colliding run then failed (a
                // failed run never reaches the continuation hook, so nothing
                // re-armed the goal).
                let (delay_ms, next_kind) = match kind {
                    ContinuationKind::Loop { .. } => {
                        match rearm_loop_after_busy(&session_key_str, origin.as_ref()).await {
                            Some((delay_ms, wake_ms)) => {
                                (Some(delay_ms), ContinuationKind::Loop { wake_ms })
                            }
                            None => (None, kind),
                        }
                    }
                    ContinuationKind::Goal { .. } => {
                        match super::goal_continuation::rearm_goal_after_busy(
                            &session_key_str,
                            origin.as_ref(),
                        )
                        .await
                        {
                            Some((delay_ms, wake_ms)) => {
                                (Some(delay_ms), ContinuationKind::Goal { wake_ms })
                            }
                            None => (None, kind),
                        }
                    }
                };
                if let Some(delay_ms) = delay_ms {
                    respawn = Some((delay_ms, next_kind));
                }
            } else {
                match kind {
                    ContinuationKind::Goal { .. } => {
                        warn!(error = %e, session = %session_key_str,
                            "goal pursuit: continuation run failed; blocking goal for user guidance");
                        super::goal_continuation::block_goal_on_failure(
                            &session_key_str,
                            &e,
                            origin.as_ref(),
                        )
                        .await;
                    }
                    ContinuationKind::Loop { .. } => {
                        match park_loop_on_transient_failure(&session_key_str, &e, origin.as_ref())
                            .await
                        {
                            TransientPark::Rearmed { delay_ms, wake_ms } => {
                                respawn = Some((delay_ms, ContinuationKind::Loop { wake_ms }));
                            }
                            // The park path already ended (or found nothing to
                            // end) and said so — stopping again here would push
                            // a second, contradictory notice.
                            TransientPark::Ended => {}
                            TransientPark::NotTransient => {
                                warn!(error = %e, session = %session_key_str,
                                    "loop: tick run failed; stopping loop for user guidance");
                                stop_loop_on_failure(&session_key_str, &e, origin.as_ref()).await;
                            }
                        }
                    }
                }
            }
            if let Some((delay_ms, next_kind)) = respawn {
                spawn_continuation_run(
                    registry.clone(),
                    adapter.clone(),
                    session_key.clone(),
                    session_key_str.clone(),
                    prompt.clone(),
                    retry_policy_meta.clone(),
                    retry_workspace.clone(),
                    retry_bus.clone(),
                    session_manager.clone(),
                    Some(delay_ms),
                    next_kind,
                );
            }
        }
    });
}

/// Loop sibling of [`super::goal_continuation::rearm_goal_after_busy`]: a woken
/// tick lost the session run-slot (`AgentBusy`). Re-arm the SAME tick with a
/// short retry delay; or, when a cap tripped during the collision, clear the
/// loop-welded strategy and notify the origin — mirroring [`stop_loop_on_failure`]
/// — so a cap-trip during a busy collision never leaves the loop a silently
/// dormant `Active` that `loop(action='status')` then misreports. Returns
/// `(delay_ms, wake_ms)` to re-spawn, else `None`.
pub(super) async fn rearm_loop_after_busy(
    session_key_str: &str,
    origin: Option<&OriginRoute>,
) -> Option<(u64, u64)> {
    let reg = crate::looping::global()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    match reg.rearm_after_busy(session_key_str, now) {
        crate::looping::RearmDecision::Retry { delay_ms, wake_ms } => {
            info!(session = %session_key_str, delay_ms,
                "loop: agent busy at tick fire; re-armed the tick with a retry delay");
            Some((delay_ms, wake_ms))
        }
        crate::looping::RearmDecision::Exhausted { note } => {
            // rearm_after_busy already marked the loop Stopped with `note`.
            // Mirror stop_loop_on_failure's cleanup: clear the loop-welded
            // Strategy so the stale plan neither bleeds into later turns nor
            // blocks a future re-plan, then tell the user (R5).
            clear_loop_welded_strategy(session_key_str, "cap-trip stop");
            notify_origin(origin, format!("⏹ {note}")).await;
            info!(session = %session_key_str, note = %note,
                "loop: cap tripped during a busy collision; loop stopped");
            None
        }
        crate::looping::RearmDecision::Drop => {
            info!(session = %session_key_str,
                "loop: agent busy at tick fire; loop no longer active, re-claimed, or superseded — tick dropped");
            None
        }
    }
}

/// What [`park_loop_on_transient_failure`] did with a failed tick, so the
/// caller neither double-notifies nor stops a loop the park path already ended.
enum TransientPark {
    /// Same tick re-armed; re-spawn it after `delay_ms` confirming `wake_ms`.
    Rearmed { delay_ms: u64, wake_ms: u64 },
    /// Handled and finished here: a cap tripped while the provider was down (the
    /// registry stopped the loop with its reason and the user was told), or
    /// there was no claimable loop left to park. Either way the caller must not
    /// stop it again.
    Ended,
    /// Not a transient provider failure — the caller owns the verdict.
    NotTransient,
}

/// Default park when a transient tick failure carries no `Retry-After`. Same
/// value the goal side uses (`goal_continuation::TRANSIENT_PARK_FALLBACK_MS`):
/// this is one question — "how long is it sane to wait out a provider the whole
/// fallback chain just failed on" — and the two continuation kinds must not
/// answer it differently.
const LOOP_TRANSIENT_PARK_FALLBACK_MS: u64 = 300_000;

/// Ceiling on a single transient park, even when the provider's own
/// `Retry-After` asks for more. See the goal sibling for the derivation
/// (`providers::failover::MAX_COOLDOWN`); must stay `>=`
/// [`LOOP_TRANSIENT_PARK_FALLBACK_MS`].
const LOOP_TRANSIENT_PARK_MAX_MS: u64 = 600_000;

/// A tick run failed with something that is not a cancellation and not a busy
/// collision. Route it by what KIND of failure it was, exactly as the goal side
/// does: [`ExecutionError::receipt_kind`] is already the single source three
/// user surfaces read, and its own doc records that a rate-limit or network
/// signature at THIS layer means every provider and model in the chain was
/// tried — so the failure is genuinely transient and "retry soon" is the right
/// answer, not `Stopped`.
///
/// Stopping was a verdict on a 429: a watch loop the user set for the week died
/// permanently on one minute of provider trouble, and `loop(action='status')`
/// reported "Halted by an error" as if the work itself had failed.
///
/// The park re-uses [`crate::looping::LoopRegistry::rearm_after_interruption`] —
/// the same atomic re-arm the busy collision takes, with the provider's own
/// backoff instead of the fixed 30 s — so a parked tick is bounded by the two
/// caps that already guard a re-armed tick (`exhausted` and, projected onto the
/// retry wake, `fires_out_of_bounds`) rather than by a new set of rules.
///
/// **New exposure this opens.** Two paths run for the first time here:
/// * `RearmDecision::Exhausted` is now reachable from a FAILURE, not only from
///   a busy collision. A loop whose deadline elapsed while its provider was
///   down now ends as "reached its time limit" — the true cause — instead of
///   "halted by an error".
/// * A multi-hour outage can park the SAME tick many times in a row (each park
///   costs no iteration: the tick was counted at claim). `fires_out_of_bounds`
///   is therefore load-bearing for the first time — it is what keeps that chain
///   from re-arming a tick that would wake past `timeout_minutes`. A loop with
///   NO deadline and no iteration cap left will retry for as long as the
///   provider stays down, which is the intended reading of an unbounded loop.
async fn park_loop_on_transient_failure(
    session_key_str: &str,
    error: &ExecutionError,
    origin: Option<&OriginRoute>,
) -> TransientPark {
    let kind = error.receipt_kind();
    if !matches!(
        kind,
        crate::gateway::i18n::ReceiptKind::RateLimited
            | crate::gateway::i18n::ReceiptKind::Unreachable
    ) {
        return TransientPark::NotTransient;
    }
    let Some(reg) = crate::looping::global() else {
        return TransientPark::NotTransient;
    };
    // The hint rides the flattened provider error chain, the same string the
    // classifier above read. Bounding it is not a second answer to the goal
    // side's question — it is literally the same function.
    let hint = crate::providers::llm_retry::extract_retry_after_str(&format!("{error}"));
    let delay_ms = crate::goal::pursuit::bound_transient_park_delay_ms(
        hint,
        LOOP_TRANSIENT_PARK_FALLBACK_MS,
        LOOP_TRANSIENT_PARK_MAX_MS,
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    match reg.rearm_after_interruption(session_key_str, now, delay_ms) {
        crate::looping::RearmDecision::Retry { delay_ms, wake_ms } => {
            // Deliberately NOT clearing the welded strategy: a provider outage
            // did not invalidate the plan.
            info!(session = %session_key_str, code = kind.code(), delay_ms,
                "loop: transient provider failure at tick; parked for retry");
            notify_origin(
                origin,
                format!(
                    "⏸ Timer loop paused on a transient provider failure ({}); \
                     retrying in ~{}.",
                    kind.code(),
                    crate::looping::types::fmt_duration_ms(delay_ms)
                ),
            )
            .await;
            TransientPark::Rearmed { delay_ms, wake_ms }
        }
        crate::looping::RearmDecision::Exhausted { note } => {
            // The registry already marked the loop Stopped with `note`; mirror
            // the other authoritative endings' cleanup and tell the user (R5).
            clear_loop_welded_strategy(session_key_str, "transient-park cap-trip stop");
            notify_origin(origin, format!("⏹ {note}")).await;
            info!(session = %session_key_str, note = %note,
                "loop: cap tripped while parking a transient failure; loop stopped");
            TransientPark::Ended
        }
        crate::looping::RearmDecision::Drop => {
            // No claimable loop left — stopped by the user, gone, or already
            // re-claimed. Stopping it again would clobber nothing but the
            // notice would be a false alarm, so stay silent (the goal side's
            // `Ok(false)` arm reads the same way).
            info!(session = %session_key_str,
                "loop: transient tick failure but the loop is no longer claimable — dropped");
            TransientPark::Ended
        }
    }
}

/// Loop sibling of [`block_goal_on_failure`]: when a clock-driven loop tick run
/// fails (non-cancellation, non-busy, and not one of the transient provider
/// failures [`park_loop_on_transient_failure`] parks), mark the loop `Stopped`
/// and best-effort notify the origin channel. Without this, a failed tick left
/// the loop a stuck `Active` in the registry that the continuation hook never
/// re-fired (it only runs on a run's success path) — a silent stall where
/// `loop(action='status')` lies.
async fn stop_loop_on_failure(
    session_key_str: &str,
    error: &ExecutionError,
    origin: Option<&OriginRoute>,
) {
    let reason: String = format!("{error}").chars().take(300).collect();
    if let Some(reg) = crate::looping::global() {
        // `transition` refuses a move out of the terminal `Stopped`, so a loop
        // the user already stopped between the tick firing and its failure
        // landing is never clobbered. The stored reason explains the halt to
        // loop(action='status') even for Panel-only sessions with no origin
        // channel to receive the notice.
        if matches!(
            reg.transition(
                session_key_str,
                crate::looping::LoopStatus::Stopped,
                Some(format!("Halted by an error: {reason}")),
            ),
            crate::looping::TransitionOutcome::Applied { .. }
        ) {
            // Mirror the tool-stop cleanup (loop_manage stop): clear the
            // loop-welded Strategy so the stale plan neither bleeds into
            // later turns nor blocks a future start from re-planning.
            clear_loop_welded_strategy(session_key_str, "failure stop");
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
pub(super) async fn notify_origin(origin: Option<&OriginRoute>, text: String) {
    if let Some((reg, ch, conv)) = origin {
        // The THIRD unattended-output leg. Two are wrapped in `run_loop::inner`
        // — `RedactingEmitter` (the event/channel leg) and
        // `UnattendedRedactingSink` (the trace leg) — and they agree byte for
        // byte. This one bypassed both: it calls `ChannelRegistry::send`
        // directly, and its callers push run-derived error text
        // (`goal_continuation` formats the raw provider error, truncates it to
        // 300 chars, and sends "⚠️ Autonomous pursuit of your standing goal
        // halted: {reason}" to Telegram or Slack).
        //
        // That this string class carries secrets is not a new judgement: the
        // emitter leg already masks `RunError.error` / `RunRetrying.reason`,
        // with the comment "Provider failures do echo request material, so this
        // is not identifier-only." Same class of string, same destination, one
        // leg masked and two not — §5.1's recurrence shape, second occurrence.
        //
        // Masked HERE rather than at the five call sites: every caller of this
        // function is a continuation notice, i.e. unattended by construction,
        // so there is no attended path to charge for it.
        //
        // The class is closed too, as of `[[security.mask_patterns]]`: the
        // vendor list is still a fixed floor, but an operator's own
        // non-vendor-shaped credential can now be named in config and is
        // installed onto the TYPE, so this leg picks it up without this call
        // site being touched.
        let masker = crate::exec::masker::SecretMasker::new();
        let msg = crate::gateway::channel::OutboundMessage::text(conv.clone(), masker.mask(&text));
        if let Err(e) = reg
            .send(&crate::gateway::channel::ChannelId::new(ch.clone()), msg)
            .await
        {
            warn!(channel = %ch, error = %e, "continuation: failed to deliver origin notice");
        }
    }
}

/// Clear the loop-welded Strategy for a session (best-effort). Gateway-side
/// single source for every authoritative loop ending here — cap-exhaustion
/// stop, cap-trip during a busy rearm, and failure stop — so the stale plan
/// neither bleeds into later plain turns of this reused session nor blocks a
/// future `start` from re-planning. The tool-side endings (`loop stop`, and
/// `start` claiming a fresh plan slot) clear the same key in
/// `builtin_tools/loop_manage.rs`. Mirrors
/// `goal_continuation::clear_goal_welded_strategy` (the continuation siblings
/// keep parity).
pub(super) fn clear_loop_welded_strategy(session: &str, context: &str) {
    if let Some(strat) = crate::strategy::global() {
        if let Err(e) = strat.delete(&crate::strategy::loop_key(session)) {
            info!(session = %session, error = %e, context,
                "loop: failed to delete welded strategy (ignored)");
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
        assert!(!naked_loop_planner_should_fire(
            &k,
            true,
            false,
            "  /goal x"
        ));
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
    fn continuation_metadata_is_unattended_and_queues_on_collision() {
        // LOOP4-STEER-1 contract: an autonomous continuation must never steer
        // (inject its machine prompt into a live attended run) nor interrupt —
        // a collision must surface as AgentBusy so the rearm path retries the
        // same claimed step. Both markers must also override inherited values.
        let mut inherited = std::collections::HashMap::new();
        inherited.insert(
            crate::gateway::execution_engine::BUSY_INPUT_MODE_KEY.to_string(),
            "interrupt".to_string(),
        );
        let m = continuation_metadata(inherited);
        assert_eq!(
            crate::gateway::execution_engine::BusyInputMode::from_metadata(&m),
            crate::gateway::execution_engine::BusyInputMode::Queue,
        );
        assert_eq!(
            m.get(crate::gateway::execution_engine::UNATTENDED_KEY)
                .map(String::as_str),
            Some("true"),
        );
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

#[cfg(test)]
mod carry_policy_metadata_tests {
    use super::{carry_policy_metadata, continuation_metadata};
    use crate::gateway::execution_engine::{CHANNEL_TOOL_PERMISSIONS_KEY, UNATTENDED_KEY};
    use std::collections::HashMap;

    fn meta(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// The two RESTRICTIVE inputs cross into the continuation; the per-turn
    /// keys do not. A Telegram chat-tier conversation used to spawn goal
    /// continuations that ran unclamped because `caller_role` was dropped here.
    #[test]
    fn only_the_policy_bearing_keys_are_carried() {
        let carried = carry_policy_metadata(&meta(&[
            ("caller_role", "guest"),
            (CHANNEL_TOOL_PERMISSIONS_KEY, r#"{"default":"deny"}"#),
            ("locale", "zh-CN"),
            ("platform", "telegram"),
            ("busy_input_mode", "queue"),
            ("channel_id", "telegram"),
            ("conversation_id", "42"),
        ]));
        assert_eq!(carried.len(), 2);
        assert_eq!(
            carried.get("caller_role").map(String::as_str),
            Some("guest")
        );
        assert_eq!(
            carried
                .get(CHANNEL_TOOL_PERMISSIONS_KEY)
                .map(String::as_str),
            Some(r#"{"default":"deny"}"#)
        );
        // An unattended run must keep failing closed on approval-gated tools:
        // carrying the origin route would make its approvals look deliverable.
        assert!(!carried.contains_key("channel_id"));
        assert!(!carried.contains_key("conversation_id"));
    }

    /// Panel / CLI turns carry no role and no channel layer — nothing to inherit.
    #[test]
    fn a_panel_turn_carries_nothing() {
        assert!(carry_policy_metadata(&meta(&[("platform", "webchat")])).is_empty());
    }

    /// P1 data isolation: a continuation must inherit the owner/scope
    /// attribution stamped on the originating request, exactly like
    /// `caller_role` — otherwise a background run's memory reads fall back
    /// to the unscoped namespace.
    #[test]
    fn continuation_inherits_owner_and_scope_keys() {
        use crate::scope::ScopeAttribution;

        let mut src = HashMap::new();
        crate::scope::stamp_metadata(&mut src, &ScopeAttribution::personal("u-alice"));
        src.insert("caller_role".into(), "member".into());

        let out = carry_policy_metadata(&src);

        assert_eq!(
            out.get(crate::scope::OWNER_META_KEY).map(String::as_str),
            Some("u-alice")
        );
        assert_eq!(
            out.get(crate::scope::SCOPE_META_KEY).map(String::as_str),
            Some("personal:u-alice")
        );
    }

    /// The composed invariant, end to end: a Chat-tier Telegram turn's clamp and
    /// channel permission layer reach the continuation, AND the continuation is
    /// still unattended. This is the bug — a continuation used to build its
    /// metadata from scratch, so a conversation clamped to `Auto` on every
    /// interactive turn spawned a background run at the unclamped global tier.
    #[test]
    fn a_continuation_inherits_the_clamp_and_stays_unattended() {
        let source = meta(&[
            ("caller_role", "guest"),
            (CHANNEL_TOOL_PERMISSIONS_KEY, r#"{"default":"deny"}"#),
            ("platform", "telegram"),
        ]);
        let cont = continuation_metadata(carry_policy_metadata(&source));

        assert_eq!(cont.get("caller_role").map(String::as_str), Some("guest"));
        assert_eq!(
            cont.get(CHANNEL_TOOL_PERMISSIONS_KEY).map(String::as_str),
            Some(r#"{"default":"deny"}"#)
        );
        assert_eq!(cont.get(UNATTENDED_KEY).map(String::as_str), Some("true"));
        // `continuation_metadata` also writes the load-bearing Queue busy-input
        // marker (LOOP4-STEER-1), so the map carries 4 keys: the two inherited
        // policy keys + the two continuation markers.
        assert_eq!(
            cont.get(crate::gateway::execution_engine::BUSY_INPUT_MODE_KEY)
                .map(String::as_str),
            Some("queue"),
        );
        assert_eq!(cont.len(), 4);
    }

    /// The marker is written LAST and is unconditional: a source map that somehow
    /// carries `unattended=false` cannot demote a continuation to attended and
    /// park it on an approval card nobody is there to answer.
    #[test]
    fn the_unattended_marker_cannot_be_overridden_by_an_inherited_key() {
        // `carry_policy_metadata` does not forward the marker at all …
        let carried = carry_policy_metadata(&meta(&[
            (UNATTENDED_KEY, "false"),
            ("caller_role", "operator"),
        ]));
        assert!(!carried.contains_key(UNATTENDED_KEY));

        // … and even a policy map that does carry it loses to the insert-last.
        let cont = continuation_metadata(meta(&[(UNATTENDED_KEY, "false")]));
        assert_eq!(cont.get(UNATTENDED_KEY).map(String::as_str), Some("true"));
    }
}

#[cfg(test)]
mod transient_loop_park_tests {
    use super::*;
    use crate::looping::types::{Cadence, LoopState, LoopStatus};

    /// The registry is a process global shared by the whole test binary, so
    /// every test here keys its own session and never enumerates.
    fn registry() -> crate::sync_primitives::Arc<crate::looping::LoopRegistry> {
        crate::looping::global().unwrap_or_else(|| {
            crate::looping::init_global(crate::sync_primitives::Arc::new(
                crate::looping::LoopRegistry::default(),
            ));
            crate::looping::global().expect("loop registry installed")
        })
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }

    /// A claimed tick that has already fired: `confirm_fire` clears the pending
    /// marker before the run executes, which is the state every tick failure
    /// lands in.
    fn fired_tick(session: &str) -> LoopState {
        LoopState::new(
            session,
            "watch CI",
            Cadence::Fixed {
                interval_ms: 300_000,
            },
            now_ms(),
        )
        .spent_iteration()
        .with_pending_tick(None)
    }

    fn rate_limited() -> ExecutionError {
        ExecutionError::Failed(
            "flow: harness: llm error: Rate limit error: Anthropic API rate limited (429); \
             retry after 60 seconds"
                .to_string(),
        )
    }

    /// Before this arm existed ANY non-cancellation error was a verdict: one
    /// 429 marked a week-long watch loop `Stopped` forever and pushed "halted
    /// on an error". The classifier that says otherwise was already in the
    /// crate — `ExecutionError::receipt_kind`.
    #[tokio::test]
    async fn a_rate_limited_tick_parks_the_loop_instead_of_stopping_it() {
        let reg = registry();
        let session = "transient-park/rate-limited";
        reg.put(fired_tick(session));

        let (delay_ms, wake_ms) =
            match park_loop_on_transient_failure(session, &rate_limited(), None).await {
                TransientPark::Rearmed { delay_ms, wake_ms } => (delay_ms, wake_ms),
                _ => panic!("a 429 is transient — the tick must be re-armed, not judged"),
            };
        assert_eq!(
            delay_ms, 60_000,
            "the provider's own Retry-After decides the wait"
        );

        let state = reg.get(session).expect("the loop must still be registered");
        assert_eq!(
            state.status,
            LoopStatus::Active,
            "a transient provider failure pauses a loop, it does not end it"
        );
        assert_eq!(
            state.pending_tick_wake_ms,
            Some(wake_ms),
            "the re-armed tick must be pending, or nothing wakes it"
        );
        assert!(
            state.stop_reason.is_none(),
            "nothing stopped, so nothing may claim a stop reason"
        );
        assert_eq!(
            state.iterations_used, 1,
            "a park costs no iteration — the tick was already counted at claim"
        );
    }

    /// The other half: a genuine failure must still stop the loop. Only the two
    /// transient buckets get a park.
    #[tokio::test]
    async fn a_non_transient_failure_is_left_to_the_caller_to_stop() {
        let reg = registry();
        let session = "transient-park/hard-failure";
        reg.put(fired_tick(session));

        let outcome = park_loop_on_transient_failure(
            session,
            &ExecutionError::Failed("tool 'shell' is not registered for this agent".to_string()),
            None,
        )
        .await;
        assert!(
            matches!(outcome, TransientPark::NotTransient),
            "an unclassified failure is a verdict the caller owns"
        );
        let state = reg.get(session).expect("still registered");
        assert_eq!(
            state.status,
            LoopStatus::Active,
            "the park path must not touch a loop it declined to handle"
        );
        assert_eq!(state.pending_tick_wake_ms, None, "and must not re-arm it");
    }

    /// The new exposure surface: `Exhausted` now enters from the FAILURE arm.
    /// A loop whose wall-clock bound elapsed while the provider was down ends
    /// with the true cause ("time limit"), and the caller must not then stop it
    /// a second time with "halted by an error".
    #[tokio::test]
    async fn a_park_past_the_wall_clock_bound_ends_with_the_deadline_reason() {
        let reg = registry();
        let session = "transient-park/deadline";
        let now = now_ms();
        reg.put(fired_tick(session).with_deadline_ms(Some(now + 1_000)));

        let outcome = park_loop_on_transient_failure(session, &rate_limited(), None).await;
        assert!(
            matches!(outcome, TransientPark::Ended),
            "the registry already stopped it; a second stop would contradict the notice"
        );
        let state = reg.get(session).expect("still registered");
        assert_eq!(state.status, LoopStatus::Stopped);
        let reason = state.stop_reason.unwrap_or_default();
        assert!(
            reason.contains("time limit"),
            "the binding bound is the deadline, not the error: {reason}"
        );
    }

    /// A loop the user stopped while the failing tick was in flight must not be
    /// resurrected by the park — and must not draw a "paused, retrying" notice
    /// for something that is over.
    #[tokio::test]
    async fn a_stopped_loop_is_never_re_armed_by_a_park() {
        let reg = registry();
        let session = "transient-park/user-stopped";
        reg.put(fired_tick(session).with_status(LoopStatus::Stopped));

        let outcome = park_loop_on_transient_failure(session, &rate_limited(), None).await;
        assert!(matches!(outcome, TransientPark::Ended));
        let state = reg.get(session).expect("still registered");
        assert_eq!(state.status, LoopStatus::Stopped);
        assert_eq!(state.pending_tick_wake_ms, None);
    }
}
