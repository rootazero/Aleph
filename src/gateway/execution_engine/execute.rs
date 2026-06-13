use super::{ActiveRun, ExecutionEngine, ExecutionError, RunRequest, RunState};
use crate::gateway::agent_instance::{AgentInstance, AgentState, MessageRole};
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::resilience::TaskStatus;
use crate::sync_primitives::Arc;
use crate::verification::stop_hooks::{execute_stop_hooks_arc, StopHookContext};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
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

        // Reserve the agent's run slot FIRST. The per-agent Idle→Running gate in
        // `try_start_run` is the single source of truth for concurrency.
        // Registering the run in `active_runs` before reserving the slot (the
        // previous order) inserted a transient `Running` row that inflated the
        // concurrent-run count and could spuriously reject a sibling run.
        // `simple.rs` already uses this ordering.
        if !agent.try_start_run(&run_id).await {
            // Agent busy. The per-channel busy-input policy (stamped into
            // metadata by the inbound router; absent → Steer) decides what
            // happens to this message. The run was never registered, so there is
            // nothing to undo on either path.
            match super::BusyInputMode::from_metadata(&request.metadata) {
                super::BusyInputMode::Interrupt => {
                    // Cancel the running sibling on THIS session, then let the
                    // inbound router's FIFO busy queue restart this message as
                    // a fresh run once the slot frees — the new run reads the
                    // interrupted task's full context from the session log
                    // plus this instruction. Reuses `cancel` + the `AgentBusy`
                    // delivery path; no new dispatch machinery (R10). If no
                    // same-session sibling is running (e.g. cross-session
                    // busy), fall through to plain busy-queue waiting without
                    // cancelling anything.
                    let target = {
                        let runs = self.active_runs.read().await;
                        super::steering::find_steering_target_id(
                            &runs,
                            &run_id,
                            &request.session_key,
                        )
                    };
                    if let Some(target_id) = target {
                        let _ = self.cancel(&target_id).await;
                        // No interruption marker is persisted here: the
                        // harness bridge emits `RunFinished{Cancelled}` when
                        // the cancelled loop tears down, and the prompt
                        // builder replays that as a `<system-reminder>`
                        // interruption note (covers /stop, Panel chat.abort
                        // and this Interrupt mode alike) — single source,
                        // nothing stored twice.
                        info!(
                            session = %request.session_key.to_key_string(),
                            target_run = %target_id,
                            "busy-input interrupt: cancelled running sibling; message will restart as a fresh run via the busy queue",
                        );
                    }
                    return Err(ExecutionError::AgentBusy(agent.id().to_string()));
                }
                super::BusyInputMode::Queue => {
                    // Follow-up lane (openclaw `followup` / hermes `queue` /
                    // Pi `followUp` / OpenSquilla `followup` parity): leave the
                    // running task untouched — no mid-loop injection, no
                    // cancellation — and let the inbound router's FIFO busy
                    // queue deliver this message as a fresh run once the
                    // current one finishes.
                    return Err(ExecutionError::AgentBusy(agent.id().to_string()));
                }
                super::BusyInputMode::Steer => {
                    // Mid-loop steering: if the busy run is on THIS session,
                    // inject the message into the live event log so the running
                    // loop picks it up at its next turn boundary (codex parity).
                    let injected = super::steering::try_inject_steering(
                        self.config.mid_turn_steering,
                        &self.active_runs,
                        self.orchestrator.as_ref(),
                        &request,
                        &run_id,
                    )
                    .await;
                    if injected {
                        return Ok(());
                    }
                    return Err(ExecutionError::AgentBusy(agent.id().to_string()));
                }
            }
        }

        // Slot held — now enforce the concurrent-run limit and register the run.
        {
            let mut runs = self.active_runs.write().await;
            let agent_runs = runs
                .values()
                .filter(|r| {
                    r.request.session_key.agent_id() == request.session_key.agent_id()
                        && matches!(r.state, RunState::Running)
                })
                .count();

            if agent_runs >= self.config.max_concurrent_runs {
                drop(runs);
                // Release the slot we just reserved before bailing out.
                agent.set_state(AgentState::Idle).await;
                return Err(ExecutionError::TooManyRuns(format!(
                    "Agent {} has {} active runs (max: {})",
                    request.session_key.agent_id(),
                    agent_runs,
                    self.config.max_concurrent_runs
                )));
            }

            runs.insert(
                run_id.clone(),
                ActiveRun {
                    request: request.clone(),
                    state: RunState::Running,
                    started_at: chrono::Utc::now(),
                    completed_at: None,
                    steps_completed: 0,
                    current_tool: None,
                    cancel_tx: Some(cancel_tx),
                    seq_counter: crate::sync_primitives::AtomicU64::new(0),
                    chunk_counter: crate::sync_primitives::AtomicU32::new(0),
                },
            );
        }

        let trace_task_persisted = self
            .persist_run_task_started(&run_id, &request, &agent)
            .await;

        // Emit run accepted event
        let _ = emitter
            .emit(StreamEvent::RunAccepted {
                run_id: run_id.clone(),
                session_key: request.session_key.to_key_string(),
                accepted_at: chrono::Utc::now().to_rfc3339(),
            })
            .await;

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

        // Store user message in session (with attachment markers for history).
        // Shared with the mid-loop steering path so both render identically.
        //
        // Resume-style runs (crash resume, post-run steering rescue) carry no
        // fresh input — the session log already holds the full trajectory —
        // so storing their empty placeholder would only pollute channel
        // history with blank user turns.
        let is_resume = request.metadata.get("resume").map(String::as_str) == Some("true");
        if !is_resume {
            let session_text = super::steering::render_user_session_text(&request);
            agent
                .add_message(&request.session_key, MessageRole::User, &session_text)
                .await;
        }

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
            if let Some(mode_json) = self.try_resolve_slash_command(&request.input) {
                request
                    .metadata
                    .insert(SLASH_COMMAND_MODE_KEY.to_string(), mode_json);
            }
        }

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

        // Propagate session key to memory_search so scope=current_session works
        if let Some(sk_handle) = self.tool_registry.session_key_handle() {
            *sk_handle.write().await = request.session_key.to_key_string();
        }

        // Propagate the active agent's memory scope + smart-recall profile to
        // memory_search. Both handles were built for exactly this write (see
        // MemorySearchTool::default_workspace_handle / smart_recall_config_handle)
        // but previously had no writer: memory_search always searched the
        // DEFAULT_AGENT workspace regardless of which agent was running, and
        // the profile's [profiles.*.smart_recall] config never reached the
        // Two-Phase Smart Recall gate.
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
            *sr_handle.write().await = smart_recall;
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

        // Execute the run
        let active_runs = self.active_runs.clone();
        let timeout_secs = request
            .timeout_secs
            .or(agent.config().timeout_secs())
            .unwrap_or(self.config.default_timeout_secs);

        let deadline = Arc::new(tokio::sync::Mutex::new(
            tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs),
        ));

        let result: Result<String, ExecutionError> = tokio::select! {
            result = self.run_agent_loop(
                &run_id,
                &request,
                agent.clone(),
                emitter.clone(),
                deadline.clone(),
                trace_task_persisted.then(|| run_id.clone()),
                cancel_token.clone(),
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

        // Reset agent state
        agent.set_state(AgentState::Idle).await;

        let final_result = match result {
            Ok(response) => {
                let response = &response;
                if trace_task_persisted {
                    self.persist_run_task_status(&run_id, TaskStatus::Completed)
                        .await;
                }

                // Store assistant response, stamping the run_id so the
                // workspace panel can rehydrate this turn's persisted trace
                // on session reload/switch.
                agent
                    .add_message_with_run_id(
                        &request.session_key,
                        MessageRole::Assistant,
                        response,
                        Some(&run_id),
                    )
                    .await;

                // Notify UI that the session was updated (global bus, so
                // channel-originated runs reach the Panel too). RunComplete
                // itself is single-sourced from the orchestrator drain.
                self.publish_session_updated(
                    &request.session_key,
                    request.metadata.get("channel_id").map(String::as_str),
                );

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
                            use crate::providers::adapter::RequestPayload;
                            use crate::providers::message::UnifiedMessage;

                            let prompt = format!(
                                "Generate a concise topic title (5-10 characters, same language as the message) \
                                 for a conversation that starts with: {topic_message}"
                            );
                            let messages = vec![UnifiedMessage::user(&prompt)];
                            let payload = RequestPayload {
                                messages: &messages,
                                system_prompt: Some("You are a title generator. Output ONLY the title, nothing else."),
                                system_blocks: None,
                                tools: None,
                                think_level: None,
                                temperature: Some(0.3),
                                max_tokens: None,
                                tool_choice: None,
                                model: None,
                                metadata: None,
                            };

                            let topic_text = match topic_provider.process(payload).await {
                                Ok(resp) => {
                                    let text = resp.text_content().trim().to_string();
                                    if text.is_empty() {
                                        None
                                    } else {
                                        Some(text)
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "Auto-topic: LLM call failed, using fallback");
                                    None
                                }
                            };

                            // Fallback: use truncated first message as topic
                            let topic_text = topic_text.unwrap_or_else(|| {
                                let msg = topic_message.trim();
                                let truncated: String = msg.chars().take(20).collect();
                                if msg.chars().count() > 20 {
                                    format!("{truncated}…")
                                } else {
                                    truncated
                                }
                            });

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

                                // 闸门分支：模型在 Active 续跑下自报 Complete，
                                // 但客观闸门（stop_hooks 退出码）尚未确认。
                                // 在接受 complete 为终止前先跑闸门（Ralph
                                // Wiggum 营救）。结构化退出码，零 LLM 调用（R7）。
                                let gate_configured = cont_deps.gate.is_some()
                                    || goal.gate_command.is_some();
                                if crate::tasks::goal_pursuit::awaiting_gate(
                                    &goal,
                                    gate_configured,
                                ) {
                                    let gate = crate::verification::stop_hooks::effective_gate(
                                        cont_deps.gate.as_ref(),
                                        goal.gate_command.as_deref(),
                                    )
                                    .expect("gate_configured implies effective_gate is Some");
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
                                    let vetoed = result
                                        .halt_reason()
                                        .or_else(|| result.blocking_reason());
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
                                                    );
                                                }
                                            } else {
                                                info!(session = %session_key_str,
                                                    "goal pursuit: objective gate vetoed at iteration cap, goal blocked");
                                            }
                                        }
                                    }
                                } else if crate::tasks::goal_pursuit::should_continue(
                                    &goal, 0, now_ms,
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
                                        );
                                        info!(session = %session_key_str,
                                            continuations_used = bumped.continuations_used,
                                            "goal pursuit: enqueued autonomous continuation");
                                    }
                                } else if crate::tasks::goal_pursuit::exhausted_while_active(
                                    &goal, 0, now_ms,
                                ) {
                                    // Distinguish wall-clock exhaustion from the
                                    // iteration cap so the user sees the real
                                    // stop reason on their next turn.
                                    let note = if goal
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
                                        .with_note(Some(note), now_ms);
                                    if let Err(e) = store.put(&blocked) {
                                        warn!(error = %e, session = %session_key_str,
                                            "goal pursuit: failed to persist cap-reached block");
                                    } else {
                                        info!(session = %session_key_str,
                                            continuations_used = goal.continuations_used,
                                            "goal pursuit: iteration cap reached, goal blocked for user guidance");
                                    }
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
                let _ = emitter
                    .emit(StreamEvent::RunError {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        error: receipt.message,
                        error_code: Some(receipt.code.to_string()),
                    })
                    .await;
                // Preserve the typed variant so downstream callers (cron / heartbeat
                // executors) can dispatch on `ExecutionError::Timeout`,
                // `Cancelled`, etc. Collapsing to `Failed(string)` here made the
                // dedicated Timeout arms unreachable and misclassified timeouts as
                // permanent failures.
                Err(e)
            }
        };

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

/// 入队一次自主续跑 run（同一 session、同一 agent，给定 prompt）。
/// 被 `should_continue` 续跑分支与 gate-failure 续跑分支共用——消除重复的
/// `RunRequest` 构造与 `tokio::spawn` 样板。
fn spawn_continuation_run(
    registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    adapter: Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>,
    session_key: crate::routing::session_key::SessionKey,
    session_key_str: String,
    prompt: String,
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    delay_ms: Option<u64>,
) {
    let cont_request = super::RunRequest {
        run_id: uuid::Uuid::new_v4().to_string(),
        input: prompt,
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
        let origin: Option<(Arc<crate::gateway::channel_registry::ChannelRegistry>, String, String)> =
            match crate::gateway::event_emitter::origin_fanout::channel_registry() {
                Some(reg) => cont_agent
                    .origin_route(&session_key)
                    .await
                    .map(|(ch, conv)| (reg, ch, conv)),
                None => None,
            };
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
            // G3: a cancelled run means the user interrupted — leave the goal
            // Active so their next interaction resumes pursuit (same rationale
            // as the post-run steering rescue's Completed-only guard). Any other
            // failure ends the silent stall: block the goal and notify, so
            // unattended pursuit never dies as a stuck `Active` with no run.
            if matches!(e, ExecutionError::Cancelled) {
                info!(session = %session_key_str, "goal pursuit: continuation cancelled by user");
            } else {
                warn!(
                    error = %e,
                    session = %session_key_str,
                    "goal pursuit: continuation run failed; blocking goal for user guidance"
                );
                block_goal_on_failure(&session_key_str, &e, origin.as_ref()).await;
            }
        }
    });
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
    origin: Option<&(Arc<crate::gateway::channel_registry::ChannelRegistry>, String, String)>,
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
                }
            }
            // Already terminal (complete/blocked/paused) or no goal → nothing
            // to block. A store error is logged, not silently swallowed.
            Ok(_) => {}
            Err(e) => warn!(error = %e, session = %session_key_str,
                "goal pursuit: goal lookup failed during failure block"),
        }
    }
    if let Some((reg, ch, conv)) = origin {
        let msg = crate::gateway::channel::OutboundMessage::text(
            conv.clone(),
            format!("⚠️ Autonomous pursuit of your standing goal halted: {reason}"),
        );
        if let Err(e) = reg
            .send(&crate::gateway::channel::ChannelId::new(ch.clone()), msg)
            .await
        {
            warn!(channel = %ch, error = %e, "goal pursuit: failed to deliver halt notice");
        }
    }
}
