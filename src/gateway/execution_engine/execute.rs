use super::{ActiveRun, ExecutionEngine, ExecutionError, RunRequest, RunState};
use crate::gateway::agent_instance::{AgentInstance, AgentState, MessageRole};
use crate::gateway::event_emitter::{EventEmitter, RunSummary, StreamEvent};
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::resilience::TaskStatus;
use crate::sync_primitives::Arc;
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
            // Agent busy. Before rejecting, try mid-loop steering: if the busy
            // run is on THIS session, inject the message into the live event
            // log so the running loop picks it up at its next turn boundary
            // (codex parity). The run was never registered, so there is nothing
            // to undo here.
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
        }

        // Store user message in session (with attachment markers for history).
        // Shared with the mid-loop steering path so both render identically.
        let session_text = super::steering::render_user_session_text(&request);
        agent
            .add_message(&request.session_key, MessageRole::User, &session_text)
            .await;

        // Announce the session the moment it's created (first message), not just
        // when the run completes. Without this, a brand-new session is silent for
        // the entire first turn and never appears in clients that refresh their
        // session list on `SessionUpdated` (e.g. the Panel sidebar) until a manual
        // reload. The run-completion `SessionUpdated` below still fires for
        // topic/title/token updates.
        if is_first_message {
            let _ = emitter
                .emit(StreamEvent::SessionUpdated {
                    session_key: request.session_key.to_key_string(),
                })
                .await;
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

        // Get run info for summary
        let (started_at, steps_completed, final_seq) = {
            let mut runs = active_runs.write().await;
            if let Some(run) = runs.get_mut(&run_id) {
                run.state = final_state.clone();
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                (run.started_at, run.steps_completed, run.next_seq())
            } else {
                (chrono::Utc::now(), 0, 0)
            }
        };

        // Reset agent state
        agent.set_state(AgentState::Idle).await;

        // Emit completion event
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;

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

                let _ = emitter
                    .emit(StreamEvent::RunComplete {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        summary: RunSummary {
                            total_tokens: 0,
                            tool_calls: 0,
                            loops: steps_completed,
                            final_response: Some(response.clone()),
                            ..Default::default()
                        },
                        total_duration_ms: duration_ms,
                    })
                    .await;

                // Notify UI that the session was updated
                let _ = emitter
                    .emit(StreamEvent::SessionUpdated {
                        session_key: request.session_key.to_key_string(),
                    })
                    .await;

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
                                 for a conversation that starts with: {}",
                                topic_message
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
                                    format!("{}…", truncated)
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

                // Async write to memory system (Layer 1)
                if let Some(ref mb) = self.memory_backend {
                    let mb = mb.clone();
                    let sk = request.session_key.to_key_string();
                    let agent_id = request.session_key.agent_id().to_string();
                    let ui = request.input.clone();
                    let ao = response.clone();
                    tokio::spawn(async move {
                        super::history::write_conversation_memory(mb, sk, agent_id, ui, ao).await;
                    });
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
                                if crate::tasks::goal_pursuit::should_continue(&goal, 0) {
                                    let now_ms = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_millis() as u64)
                                        .unwrap_or(0);
                                    let bumped = goal.clone().spent_continuation(now_ms);
                                    if let Err(e) = store.put(&bumped) {
                                        warn!(
                                            error = %e,
                                            session = %session_key_str,
                                            "goal pursuit: failed to persist continuation counter; skipping"
                                        );
                                    } else {
                                        let prompt =
                                            crate::tasks::goal_pursuit::continuation_prompt(&goal);
                                        let cont_request = super::RunRequest {
                                            run_id: uuid::Uuid::new_v4().to_string(),
                                            input: prompt,
                                            session_key: request.session_key.clone(),
                                            timeout_secs: None,
                                            metadata: std::collections::HashMap::new(),
                                            attachments: Vec::new(),
                                            pending_media: crate::sync_primitives::Arc::new(
                                                tokio::sync::Mutex::new(Vec::new()),
                                            ),
                                            sandbox_override: None,
                                            workspace_override: None,
                                            max_iterations_override: None,
                                            model_override: None,
                                        };
                                        let cont_agent_id =
                                            request.session_key.agent_id().to_string();
                                        let cont_registry = cont_deps.0.clone();
                                        let cont_adapter = cont_deps.1.clone();
                                        let cont_session = session_key_str.clone();
                                        tokio::spawn(async move {
                                            let resolved_agent =
                                                cont_registry.get(&cont_agent_id).await;
                                            let Some(cont_agent) = resolved_agent else {
                                                warn!(
                                                    agent_id = %cont_agent_id,
                                                    session = %cont_session,
                                                    "goal pursuit: agent not found, skipping continuation"
                                                );
                                                return;
                                            };
                                            use crate::gateway::event_emitter::{
                                                CollectingEventEmitter, EventEmitter,
                                            };
                                            let emitter: crate::sync_primitives::Arc<
                                                dyn EventEmitter + Send + Sync,
                                            > = crate::sync_primitives::Arc::new(
                                                CollectingEventEmitter::new(),
                                            );
                                            if let Err(e) = cont_adapter
                                                .execute(cont_request, cont_agent, emitter)
                                                .await
                                            {
                                                warn!(
                                                    error = %e,
                                                    session = %cont_session,
                                                    "goal pursuit: continuation run failed"
                                                );
                                            }
                                        });
                                        info!(
                                            session = %session_key_str,
                                            continuations_used = bumped.continuations_used,
                                            "goal pursuit: enqueued autonomous continuation"
                                        );
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

        final_result
    }
}
