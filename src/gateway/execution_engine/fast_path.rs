use super::{ExecutionEngine, ExecutionError, RunRequest, RunState};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, RunSummary, StreamEvent};
use crate::resilience::TaskStatus;
use tracing::warn;

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
    /// Finalize a successful slash command fast-path execution.
    pub(super) async fn finalize_fast_path_success<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: &AgentInstance,
        emitter: &crate::sync_primitives::Arc<E>,
        response: String,
        task_persisted: bool,
    ) -> Result<(), ExecutionError> {
        let (started_at, steps_completed, final_seq) = {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(run_id) {
                run.state = RunState::Completed;
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                (run.started_at, run.steps_completed, run.next_seq())
            } else {
                (chrono::Utc::now(), 0, 0)
            }
        };

        // Session claim + concurrency permit release: handled by `_run_slot`'s
        // `Drop` when `execute()` returns right after this call (Task 6 — the
        // per-agent `AgentState::Idle` reset that used to sit here is gone).
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;
        if task_persisted {
            self.persist_run_task_status(run_id, TaskStatus::Completed)
                .await;
        }

        // Fast path bypasses seed_session (no SessionEvent::UserMessage from harness)
        // and also bypasses the main-path direct add_message — emit both via events
        // so the projector writes them to messages in the correct order.
        if let Some(svc) = crate::session::service::global_session_service() {
            let turn_id = uuid::Uuid::new_v4();
            let _ = svc
                .emit_event(
                    &request.session_key,
                    crate::session::events::SessionEvent::UserMessage {
                        turn_id,
                        content: crate::session::events::MessageContent {
                            text: request.input.clone(),
                            blocks: Vec::new(),
                            thinking: None,
                            thinking_signature: None,
                        },
                        at: crate::session::events::now_ms(),
                        synthetic: false,
                        author_user_id: crate::scope::room_author_from_metadata(&request.metadata),
                    },
                )
                .await;
            let _ = svc
                .emit_event(
                    &request.session_key,
                    crate::session::events::SessionEvent::AssistantMessage {
                        turn_id,
                        content: crate::session::events::MessageContent {
                            text: response.clone(),
                            blocks: Vec::new(),
                            thinking: None,
                            thinking_signature: None,
                        },
                        // Slash-command reply — no LLM call, nothing billed.
                        usage: None,
                        at: crate::session::events::now_ms(),
                    },
                )
                .await;
        }
        let _ = emitter
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq: final_seq,
                summary: RunSummary {
                    // 0 is correct: the L0 slash-command fast path bypasses
                    // the agent loop and makes no LLM call (commands that
                    // need an LLM fall through to the loop instead).
                    total_tokens: 0,
                    tool_calls: 1,
                    loops: steps_completed,
                    final_response: Some(response),
                    ..Default::default()
                },
                total_duration_ms: duration_ms,
            })
            .await;
        self.publish_session_updated(
            &request.session_key,
            request.metadata.get("channel_id").map(String::as_str),
            &request.run_id,
        );

        // Clear the session-level "running" marker now that the final message
        // has been persisted.
        agent.set_session_idle(&request.session_key).await;

        // Remove from active runs after a short delay (same as normal path)
        let runs_clone = self.active_runs.clone();
        let run_id_owned = run_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            runs_clone.write().await.remove(&run_id_owned);
        });

        Ok(())
    }

    /// Finalize a failed slash command fast-path execution (non-fallthrough error).
    pub(super) async fn finalize_fast_path_error<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: &AgentInstance,
        emitter: &crate::sync_primitives::Arc<E>,
        error_msg: &str,
        task_persisted: bool,
    ) -> Result<(), ExecutionError> {
        let (started_at, final_seq) = {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(run_id) {
                run.state = RunState::Failed {
                    error: error_msg.to_string(),
                };
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                (run.started_at, run.next_seq())
            } else {
                (chrono::Utc::now(), 0)
            }
        };

        // Session claim + concurrency permit release: handled by `_run_slot`'s
        // `Drop` when `execute()` returns right after this call (Task 6 — the
        // per-agent `AgentState::Idle` reset that used to sit here is gone).
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;
        if task_persisted {
            self.persist_run_task_status(run_id, TaskStatus::Failed)
                .await;
        }
        let error_response = format!("❌ {error_msg}");

        // Fast path bypasses seed_session and main-path add_message — emit both
        // user + assistant events so the projector writes them to messages.
        if let Some(svc) = crate::session::service::global_session_service() {
            let turn_id = uuid::Uuid::new_v4();
            let _ = svc
                .emit_event(
                    &request.session_key,
                    crate::session::events::SessionEvent::UserMessage {
                        turn_id,
                        content: crate::session::events::MessageContent {
                            text: request.input.clone(),
                            blocks: Vec::new(),
                            thinking: None,
                            thinking_signature: None,
                        },
                        at: crate::session::events::now_ms(),
                        synthetic: false,
                        author_user_id: crate::scope::room_author_from_metadata(&request.metadata),
                    },
                )
                .await;
            let _ = svc
                .emit_event(
                    &request.session_key,
                    crate::session::events::SessionEvent::AssistantMessage {
                        turn_id,
                        content: crate::session::events::MessageContent {
                            text: error_response.clone(),
                            blocks: Vec::new(),
                            thinking: None,
                            thinking_signature: None,
                        },
                        // Error echo — no LLM call, nothing billed.
                        usage: None,
                        at: crate::session::events::now_ms(),
                    },
                )
                .await;
        }
        let _ = emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq: 1,
                delta: error_response.clone(),
                full_text: String::new(),
                chunk_index: 0,
                is_final: true,
                is_intermediate: false,
            })
            .await;
        let _ = emitter
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq: final_seq,
                summary: RunSummary {
                    // 0 is correct: the L0 slash-command fast path bypasses
                    // the agent loop and makes no LLM call.
                    total_tokens: 0,
                    tool_calls: 1,
                    loops: 0,
                    final_response: Some(error_response),
                    ..Default::default()
                },
                total_duration_ms: duration_ms,
            })
            .await;
        self.publish_session_updated(
            &request.session_key,
            request.metadata.get("channel_id").map(String::as_str),
            &request.run_id,
        );
        warn!(
            run_id = %run_id,
            error = %error_msg,
            "Slash command fast path failed, returning error to user"
        );

        // Clear the session-level "running" marker now that the error receipt
        // has been persisted.
        agent.set_session_idle(&request.session_key).await;

        // Remove from active runs after a short delay (same as normal path)
        let runs_clone = self.active_runs.clone();
        let run_id_owned = run_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            runs_clone.write().await.remove(&run_id_owned);
        });

        Ok(())
    }
}
