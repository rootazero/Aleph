//! `AgentLoopBridge` — bridges A2A protocol messages to Aleph's internal Agent Loop.
//!
//! Implements the `A2AMessageHandler` port by translating incoming `A2AMessage`
//! into `RunRequest`, executing via `ExecutionAdapter`, and mapping results
//! back to A2A task state transitions.

use crate::sync_primitives::{Arc, AtomicU64, Mutex, Ordering};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;

use async_trait::async_trait;
use chrono::Utc;
use futures::{FutureExt, Stream};
use tracing::{error, info};

use crate::a2a::domain::{
    A2AError, A2AMessage, A2ARole, A2ATask, Artifact, Part, TaskArtifactUpdateEvent, TaskState,
    TaskStatus, TaskStatusUpdateEvent, UpdateEvent,
};
use crate::a2a::port::{A2AMessageHandler, A2AResult, A2AStreamingHandler, A2ATaskManager};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::router::SessionKey;
use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

/// Fallback response text when a run finished without a final response.
const DEFAULT_COMPLETION_TEXT: &str = "Task completed successfully";

/// Bridges A2A protocol messages to Aleph's Agent Loop execution.
///
/// Holds references to the execution infrastructure and task management ports,
/// translating between A2A domain concepts and Gateway execution primitives.
pub struct AgentLoopBridge {
    pub(crate) agent_registry: Arc<AgentRegistry>,
    pub(crate) execution_adapter: Arc<dyn ExecutionAdapter>,
    pub(crate) task_manager: Arc<dyn A2ATaskManager>,
    pub(crate) streaming: Arc<dyn A2AStreamingHandler>,
}

/// Captures the assistant's final text from a run's terminal `RunComplete` event.
///
/// `ExecutionAdapter::execute` returns only `Result<(), ExecutionError>`, so the
/// answer the agent produced is observable exclusively through the event
/// emitter. The A2A bridge needs that text to build the task's response
/// message instead of a fixed placeholder.
struct FinalResponseEmitter {
    seq_counter: AtomicU64,
    final_response: Mutex<Option<String>>,
}

impl Default for FinalResponseEmitter {
    fn default() -> Self {
        Self {
            seq_counter: AtomicU64::new(0),
            final_response: Mutex::new(None),
        }
    }
}

impl FinalResponseEmitter {
    /// Take the captured final text, if the run produced one.
    fn take_final_response(&self) -> Option<String> {
        self.final_response
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

#[async_trait]
impl EventEmitter for FinalResponseEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        if let StreamEvent::RunComplete { summary, .. } = event {
            *self.final_response.lock().unwrap_or_else(|e| e.into_inner()) = summary.final_response;
        }
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Whether the task already reached a terminal state.
///
/// A cancel that lands while a run is in flight moves the task to `Canceled`
/// and publishes its own terminal event. The run's own result must not
/// overwrite that state, and the event stream must not receive a second,
/// contradictory terminal event.
async fn task_is_terminal(task_manager: &Arc<dyn A2ATaskManager>, task_id: &str) -> bool {
    match task_manager.get_task(task_id, None).await {
        Ok(task) => task.status.state.is_terminal(),
        Err(e) => {
            tracing::error!(
                task_id,
                error = %e,
                "A2A bridge: failed to read task state"
            );
            false
        }
    }
}

/// Store `text` as the task's artifact and publish the artifact event.
///
/// A2A tasks carry their output in `artifacts`; the artifact machinery (the
/// `artifact-update` push-notification filter, the SSE `ArtifactUpdate`
/// branch) had no server-side producer, so a completed task came back with an
/// empty artifact list and an artifact-only subscriber never heard anything.
async fn publish_artifact(
    task_manager: &Arc<dyn A2ATaskManager>,
    streaming: &Arc<dyn A2AStreamingHandler>,
    task_id: &str,
    context_id: &str,
    text: &str,
) {
    let artifact = Artifact {
        artifact_id: uuid::Uuid::new_v4().to_string(),
        kind: "text".to_string(),
        parts: vec![Part::Text {
            text: text.to_string(),
            metadata: None,
        }],
        metadata: None,
    };
    if let Err(e) = task_manager.add_artifact(task_id, artifact.clone()).await {
        tracing::debug!(
            task_id,
            error = %e,
            "A2A bridge: could not store the task artifact"
        );
    }
    let event = TaskArtifactUpdateEvent {
        task_id: task_id.to_string(),
        context_id: context_id.to_string(),
        artifact,
        append: false,
        last_chunk: true,
        metadata: None,
    };
    if let Err(e) = streaming.broadcast_artifact(task_id, event).await {
        tracing::debug!(
            task_id,
            error = %e,
            "A2A bridge: no live subscriber for the artifact event"
        );
    }
}

impl AgentLoopBridge {
    /// Create a new bridge with all required dependencies.
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        execution_adapter: Arc<dyn ExecutionAdapter>,
        task_manager: Arc<dyn A2ATaskManager>,
        streaming: Arc<dyn A2AStreamingHandler>,
    ) -> Self {
        Self {
            agent_registry,
            execution_adapter,
            task_manager,
            streaming,
        }
    }

    /// Build a `RunRequest` for the given task and input text.
    ///
    /// Inbound A2A delegations are external requests — the caller has no
    /// notion of our local project layout, so the workspace falls back to
    /// the agent's default. A2A clients that want to address a specific
    /// folder should send an absolute path inside the request body and
    /// let the agent open it with file tools, not pre-bind the run.
    ///
    /// The peer on an A2A wire is a machine: there is no channel to render an
    /// approval card on and no human to press it, so the run is marked
    /// unattended and confirm-gated tools fail closed instead of parking on an
    /// approval nobody can give.
    fn build_run_request(task_id: &str, input: &str) -> RunRequest {
        RunRequest {
            run_id: uuid::Uuid::new_v4().to_string(),
            input: input.to_string(),
            session_key: SessionKey::task("main", "a2a", task_id),
            timeout_secs: None,
            metadata: HashMap::from([(
                crate::gateway::execution_engine::UNATTENDED_KEY.to_string(),
                "true".to_string(),
            )]),
            attachments: Vec::new(),
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
            telecom_public: None,
            telecom_authority: None,
        }
    }

    /// The `TurnContext` installed around a spawned A2A streaming run.
    ///
    /// PR-7 / BT-D-R4-06 + BT-D-R4-07: PR-4 taught
    /// [`crate::gateway::visibility::ambient_actor`] to fall back to
    /// `TURN_CONTEXT`. Without this scope wrap nothing set the task-local
    /// around the spawn, so the bridge's tool dispatch reached the actor-less
    /// (unrestricted) arm.
    ///
    /// What this does NOT do — the comment that used to stand here asserted
    /// both of the following as fact, and both were false:
    ///
    /// - There is no machine sentinel. `TurnContext` has no actor field for one
    ///   to live in; the only identity it carries is `session_key`, and
    ///   [`AgentLoopBridge::build_run_request`] pins
    ///   `SessionKey::task("main", ...)`. So `ambient_actor()`'s third arm hands
    ///   the literal AGENT id `"main"` to predicates that compare against USER
    ///   ids — every one of them answers false and an A2A streaming run is
    ///   silently DENIED, rather than being unrestricted or being a separate
    ///   (machine) actor.
    /// - Installing this context was not behaviour-preserving. It moved the
    ///   streaming run OFF the unrestricted arm; "denied everywhere" is a
    ///   behaviour change, not "today's behavior unchanged".
    ///
    /// [`crate::gateway::pty::session::SpawnOptions::created_by`] names the same
    /// two-kinds-of-identity hazard correctly and is worth reading first: a
    /// column that holds a user id everywhere else starts holding an agent id
    /// the moment a run is the caller.
    ///
    /// Pinned by `a2a_streaming_run_acts_as_the_agent_id_not_a_machine_sentinel`
    /// so the sentinel cannot be re-derived from prose.
    ///
    /// Promoting this to a real machine principal (a new identity kind plus an
    /// `ambient_actor()` arm) is deliberately NOT done here — it would have zero
    /// consumers today. Filed, not built.
    ///
    /// UNVERIFIED, stated so the next reader checks rather than assumes: the
    /// synchronous `handle_message` path installs no `TURN_CONTEXT` at all, and
    /// the two A2A faces are believed to agree only because the engine supplies
    /// its own at dispatch.
    fn a2a_turn_context(request: &RunRequest) -> TurnContext {
        TurnContext {
            session_key: request.session_key.clone(),
            run_id: request.run_id.clone(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: true,
            plan_gate: None,
            side_question: false,
            telecom_authority: None,
        }
    }

    /// Determine the `context_id`: use `session_id` if provided, otherwise `task_id`.
    fn context_id<'a>(task_id: &'a str, session_id: Option<&'a str>) -> &'a str {
        session_id.unwrap_or(task_id)
    }

    /// Publish the task's terminal state to the event stream.
    ///
    /// The task store and the event stream are separate views: the synchronous
    /// path wrote the task store but never notified subscribers, so a client
    /// that registered a push-notification webhook and used `message/send`
    /// never received the completion.
    async fn publish_terminal_status(&self, task: &A2ATask) {
        let event = TaskStatusUpdateEvent::new(
            task.id.clone(),
            task.context_id.clone(),
            task.status.clone(),
            None,
        );
        if let Err(e) = self.streaming.broadcast_status(&task.id, event).await {
            tracing::debug!(
                task_id = %task.id,
                error = %e,
                "A2A bridge: no live subscriber for the terminal event"
            );
        }
        // The synchronous path opens no subscription of its own, so the hub
        // channel the broadcast just created would linger for every task.
        if let Err(e) = self.streaming.cleanup_task(&task.id).await {
            tracing::warn!(
                task_id = %task.id,
                error = %e,
                "A2A bridge: cleanup_task failed after a synchronous completion"
            );
        }
    }

    /// Store the run's answer as the task's artifact and publish it.
    ///
    /// The A2A task shape carries its output in `artifacts`, and the artifact
    /// machinery (the `artifact-update` push-notification filter, the SSE
    /// `ArtifactUpdate` branch) had no server-side producer: a completed task
    /// came back with an empty artifact list.
    async fn publish_terminal_artifact(&self, task: &A2ATask, text: &str) {
        publish_artifact(
            &self.task_manager,
            &self.streaming,
            &task.id,
            &task.context_id,
            text,
        )
        .await;
    }
}

#[async_trait]
impl A2AMessageHandler for AgentLoopBridge {
    async fn handle_message(
        &self,
        task_id: &str,
        message: A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<A2ATask> {
        let input = message.text_content();
        if input.trim().is_empty() {
            return Err(A2AError::InvalidParams(
                "Message contains no text content".to_string(),
            ));
        }

        // Atomically create-or-claim the task. `claim_task` either
        // creates a new task and promotes it to `Working` in a single
        // critical section (returning `Ok(true)`), or sees the task
        // already owned by another run and returns `Ok(false)`. This
        // closes the A2A-R3-04 TOCTOU window between
        // `create_task` + `update_status(Working)` that previously
        // allowed a concurrent delegation to also reach `execute()`.
        let context_id = Self::context_id(task_id, session_id);
        let claimed = self.task_manager.claim_task(task_id, context_id).await?;
        if !claimed {
            return Err(A2AError::InvalidRequest(format!(
                "Task {task_id} is already being processed by another request"
            )));
        }

        // Get the default agent before running so a missing default fails
        // fast. (The task is already in `Working`; if the agent is missing
        // we transition it to `Failed` so callers do not see a stuck
        // `Working` task.)
        let agent = match self.agent_registry.get_default().await {
            Some(a) => a,
            None => {
                // The task was atomically claimed by `claim_task` and is now in
                // `Working`. A swallowed `update_status` failure here leaves it
                // stuck in `Working` forever; surface the error in logs and
                // still return the user-facing `InternalError` so the caller
                // does not block waiting for a completion that will never come.
                if let Err(e) = self
                    .task_manager
                    .update_status(
                        task_id,
                        TaskState::Failed,
                        Some(A2AMessage::text(
                            A2ARole::Agent,
                            "No default agent registered",
                        )),
                    )
                    .await
                {
                    tracing::error!(
                        task_id,
                        error = %e,
                        "A2A bridge: failed to mark task Failed after missing-default-agent"
                    );
                }
                return Err(A2AError::InternalError(
                    "No default agent registered".to_string(),
                ));
            }
        };

        // Build and execute the run request
        let request = Self::build_run_request(task_id, &input);
        let run_id = request.run_id.clone();
        let emitter = Arc::new(FinalResponseEmitter::default());

        info!(task_id, run_id = %run_id, "A2A bridge: executing synchronous run");

        match self
            .execution_adapter
            .execute(request, agent, emitter.clone())
            .await
        {
            Ok(()) => {
                // Execution succeeded — mark task as Completed with the answer
                // the agent produced.
                let final_text = emitter
                    .take_final_response()
                    .unwrap_or_else(|| DEFAULT_COMPLETION_TEXT.to_string());
                let response_msg = A2AMessage::text(A2ARole::Agent, final_text.clone());
                // A cancel that landed while the run was in flight wins: the
                // task is already terminal, so the run's result must not
                // overwrite it.
                if task_is_terminal(&self.task_manager, task_id).await {
                    let task = self.task_manager.get_task(task_id, None).await?;
                    info!(task_id, "A2A bridge: task already terminal; keeping its terminal state");
                    return Ok(task);
                }
                let task = self
                    .task_manager
                    .update_status(task_id, TaskState::Completed, Some(response_msg))
                    .await?;
                self.publish_terminal_artifact(&task, &final_text).await;
                self.publish_terminal_status(&task).await;
                info!(task_id, "A2A bridge: task completed");
                Ok(task)
            }
            Err(e) => {
                // Execution failed — mark task as Failed
                let error_msg = A2AMessage::text(A2ARole::Agent, format!("Execution failed: {e}"));
                if task_is_terminal(&self.task_manager, task_id).await {
                    let task = self.task_manager.get_task(task_id, None).await?;
                    error!(task_id, error = %e, "A2A bridge: task already terminal; keeping its terminal state");
                    return Ok(task);
                }
                let task = self
                    .task_manager
                    .update_status(task_id, TaskState::Failed, Some(error_msg))
                    .await?;
                self.publish_terminal_status(&task).await;
                error!(task_id, error = %e, "A2A bridge: task failed");
                Ok(task)
            }
        }
    }

    async fn handle_message_stream(
        &self,
        task_id: &str,
        message: A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>> {
        let input = message.text_content();
        if input.trim().is_empty() {
            return Err(A2AError::InvalidParams(
                "Message contains no text content".to_string(),
            ));
        }

        // Atomically claim the task (see handle_message for rationale).
        let context_id = Self::context_id(task_id, session_id);
        let claimed = self.task_manager.claim_task(task_id, context_id).await?;
        if !claimed {
            return Err(A2AError::InvalidRequest(format!(
                "Task {task_id} is already being processed by another request"
            )));
        }

        // Get the default agent before subscribing so a missing default
        // fails fast and we do not need to clean up a half-set-up stream.
        let agent = match self.agent_registry.get_default().await {
            Some(a) => a,
            None => {
                // Companion arm of the sync path above — see that block for the
                // rationale. The streaming path additionally has a
                // half-subscribed SSE stream attached, so the log line must be
                // just as loud here.
                if let Err(e) = self
                    .task_manager
                    .update_status(
                        task_id,
                        TaskState::Failed,
                        Some(A2AMessage::text(
                            A2ARole::Agent,
                            "No default agent registered",
                        )),
                    )
                    .await
                {
                    tracing::error!(
                        task_id,
                        error = %e,
                        "A2A bridge: failed to mark task Failed after missing-default-agent (streaming)"
                    );
                }
                return Err(A2AError::InternalError(
                    "No default agent registered".to_string(),
                ));
            }
        };

        // Subscribe to streaming updates AFTER the task has been atomically
        // claimed and promoted to `Working`. The cleanup guard below
        // guarantees the broadcast channel is released on every error
        // path, including panics in `execution_adapter.execute`.
        let stream = self.streaming.subscribe_all(task_id).await?;

        let working_event = TaskStatusUpdateEvent::new(
            task_id.to_string(),
            context_id.to_string(),
            TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Utc::now(),
            },
            None,
        );
        if let Err(e) = self
            .streaming
            .broadcast_status(task_id, working_event)
            .await
        {
            // A dropped `Working` event silently turns the consumer-side
            // `fold_stream` into a `success=false` outcome even when the run
            // later completes. Log loudly so this is at least diagnosable.
            tracing::error!(
                task_id,
                error = %e,
                "A2A bridge: failed to broadcast Working event; SSE consumers may miss task progression"
            );
        }

        // Build the run request
        let request = Self::build_run_request(task_id, &input);
        let run_id = request.run_id.clone();
        let emitter = Arc::new(FinalResponseEmitter::default());

        info!(task_id, run_id = %run_id, "A2A bridge: executing streaming run");

        // Spawn execution in background
        let task_manager = Arc::clone(&self.task_manager);
        let streaming = Arc::clone(&self.streaming);
        let execution_adapter = Arc::clone(&self.execution_adapter);
        let task_id_owned = task_id.to_string();
        let context_id_owned = context_id.to_string();
        // Kept for the panic path below: `context_id_owned` is moved into the
        // execution body, but a panic still has to publish a terminal event.
        let context_id_for_panic = context_id_owned.clone();

        // What this actor actually is, and what it is NOT, is written on
        // `a2a_turn_context` — read it before adding an identity here.
        let a2a_turn = Self::a2a_turn_context(&request);

        tokio::spawn(TURN_CONTEXT.scope(
            a2a_turn,
            // Catch panics so the trailing `cleanup_task` ALWAYS runs. The
            // previous `AssertUnwindSafe(async move { ... })` was a no-op: it
            // satisfies the `UnwindSafe` trait bound, but does NOT catch
            // panics. A panic inside `execution_adapter.execute` / `update_status`
            // / `broadcast_status` would unwind past the trailing
            // `cleanup_task` call and leak the broadcast channel entry —
            // see A2A-R4-06. `catch_unwind` converts a panic into a `Result`
            // arm so the cleanup runs in both branches.
            async move {
                let body = async {
                    match execution_adapter.execute(request, agent, emitter.clone()).await {
                        Ok(()) => {
                            let final_text = emitter
                                .take_final_response()
                                .unwrap_or_else(|| DEFAULT_COMPLETION_TEXT.to_string());
                            let response_msg = A2AMessage::text(A2ARole::Agent, final_text.clone());
                            // A cancel that landed while the run was in flight
                            // already moved the task to a terminal state and
                            // published its own terminal event: the run's result
                            // must not overwrite it, and the stream must not see
                            // a second, contradictory terminal event.
                            if task_is_terminal(&task_manager, &task_id_owned).await {
                                info!(task_id = %task_id_owned, "A2A bridge: task already terminal; keeping its terminal state");
                                return;
                            }
                            if let Err(e) = task_manager
                                .update_status(
                                    &task_id_owned,
                                    TaskState::Completed,
                                    Some(response_msg.clone()),
                                )
                                .await
                            {
                                // Do not abort the broadcast: the stored state
                                // and the event stream are separate views, and a
                                // consumer that only watches the stream would
                                // otherwise see the channel close with no
                                // terminal event at all.
                                error!(task_id = %task_id_owned, error = %e, "Failed to update task to Completed");
                            }

                            let completed_event = TaskStatusUpdateEvent::new(
                                task_id_owned.clone(),
                                context_id_owned.clone(),
                                TaskStatus {
                                    state: TaskState::Completed,
                                    message: Some(response_msg),
                                    timestamp: Utc::now(),
                                },
                                None,
                            );
                            publish_artifact(
                                &task_manager,
                                &streaming,
                                &task_id_owned,
                                &context_id_owned,
                                &final_text,
                            )
                            .await;
                            if let Err(e) = streaming
                                .broadcast_status(&task_id_owned, completed_event)
                                .await
                            {
                                tracing::error!(
                                    task_id = %task_id_owned,
                                    error = %e,
                                    "A2A bridge: failed to broadcast completion event; \
                                     downstream SSE consumers may not see the task as finished"
                                );
                            }
                            info!(task_id = %task_id_owned, "A2A bridge: streaming task completed");
                        }
                        Err(e) => {
                            let error_msg = A2AMessage::text(
                                A2ARole::Agent,
                                format!("Execution failed: {e}"),
                            );
                            // Same guard as the completion arm: a cancel that
                            // landed during the run owns the terminal state.
                            if task_is_terminal(&task_manager, &task_id_owned).await {
                                error!(task_id = %task_id_owned, error = %e, "A2A bridge: task already terminal; keeping its terminal state");
                                return;
                            }
                            if let Err(update_err) = task_manager
                                .update_status(
                                    &task_id_owned,
                                    TaskState::Failed,
                                    Some(error_msg.clone()),
                                )
                                .await
                            {
                                // Same reasoning as the Completed branch: the
                                // failure event must still reach subscribers.
                                error!(task_id = %task_id_owned, error = %update_err, "Failed to update task to Failed");
                            }

                            let failed_event = TaskStatusUpdateEvent::new(
                                task_id_owned.clone(),
                                context_id_owned,
                                TaskStatus {
                                    state: TaskState::Failed,
                                    message: Some(error_msg),
                                    timestamp: Utc::now(),
                                },
                                None,
                            );
                            if let Err(bcast_err) = streaming
                                .broadcast_status(&task_id_owned, failed_event)
                                .await
                            {
                                tracing::error!(
                                    task_id = %task_id_owned,
                                    error = %bcast_err,
                                    "A2A bridge: failed to broadcast failed-state event"
                                );
                            }
                            error!(task_id = %task_id_owned, error = %e, "A2A bridge: streaming task failed");
                        }
                    }
                };
                // `catch_unwind` returns a `Result<(), Box<dyn Any + Send>>`.
                // Log the panic (the only thing left to do — cleanup_task is
                // already awaited unconditionally below).
                if let Err(panic_payload) = AssertUnwindSafe(body).catch_unwind().await {
                    error!(
                        task_id = %task_id_owned,
                        ?panic_payload,
                        "panic in A2A streaming bridge; task will be marked Failed"
                    );
                    // A panic strands the task in `Working`: no terminal status
                    // is recorded, so the task id stays unusable for later
                    // callers and SSE subscribers never receive a final event.
                    // Drive the task to `Failed` and publish that event.
                    let already_terminal = task_is_terminal(&task_manager, &task_id_owned).await;
                    if !already_terminal {
                        if let Err(e) = task_manager
                            .update_status(&task_id_owned, TaskState::Failed, None)
                            .await
                        {
                            tracing::error!(
                                task_id = %task_id_owned,
                                error = %e,
                                "A2A bridge: failed to mark task Failed after panic"
                            );
                        }
                        let failed_event = TaskStatusUpdateEvent::new(
                            task_id_owned.clone(),
                            context_id_for_panic,
                            TaskStatus {
                                state: TaskState::Failed,
                                message: None,
                                timestamp: Utc::now(),
                            },
                            None,
                        );
                        if let Err(e) = streaming
                            .broadcast_status(&task_id_owned, failed_event)
                            .await
                        {
                            tracing::error!(
                                task_id = %task_id_owned,
                                error = %e,
                                "A2A bridge: failed to broadcast failed-state event after panic"
                            );
                        }
                    }
                }
                // Cleanup runs in BOTH the success and the panic branches.
                if let Err(e) = streaming.cleanup_task(&task_id_owned).await {
                    tracing::warn!(
                        task_id = %task_id_owned,
                        error = %e,
                        "A2A bridge: cleanup_task failed; broadcast channel entry may leak in StreamHub"
                    );
                }
            },
        ));

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::a2a::adapter::server::StreamHub;
    use crate::a2a::adapter::server::TaskStore;
    use crate::a2a::domain::Part;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_engine::ExecutionError;

    use futures::StreamExt;

    /// Mock execution adapter that tracks calls
    struct MockExecutionAdapter {
        should_fail: bool,
        calls: tokio::sync::Mutex<Vec<String>>,
    }

    impl MockExecutionAdapter {
        fn succeeding() -> Self {
            Self {
                should_fail: false,
                calls: tokio::sync::Mutex::new(Vec::new()),
            }
        }

        fn failing() -> Self {
            Self {
                should_fail: true,
                calls: tokio::sync::Mutex::new(Vec::new()),
            }
        }

        async fn call_count(&self) -> usize {
            self.calls.lock().await.len()
        }
    }

    #[async_trait]
    impl ExecutionAdapter for MockExecutionAdapter {
        async fn execute(
            &self,
            request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            self.calls.lock().await.push(request.run_id);
            if self.should_fail {
                Err(ExecutionError::Failed("Mock execution failure".to_string()))
            } else {
                Ok(())
            }
        }

        async fn cancel(&self, _run_id: &str) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn get_status(
            &self,
            _run_id: &str,
        ) -> Option<crate::gateway::execution_engine::RunStatus> {
            None
        }

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    fn test_session_manager(
        temp: &tempfile::TempDir,
    ) -> Arc<crate::gateway::session_manager::SessionManager> {
        let config = crate::gateway::session_manager::SessionManagerConfig {
            db_path: temp.path().join("test_sessions.db"),
            ..Default::default()
        };
        Arc::new(
            crate::gateway::session_manager::SessionManager::new(config)
                .expect("test session manager"),
        )
    }

    /// Helper to create a test bridge with real TaskStore/StreamHub and mock adapter
    async fn make_bridge(adapter: MockExecutionAdapter) -> (tempfile::TempDir, AgentLoopBridge) {
        let registry = Arc::new(AgentRegistry::new());
        let temp = tempfile::tempdir().unwrap();
        let sm = test_session_manager(&temp);
        let config = AgentInstanceConfig {
            agent_id: "main".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/main"),
            ..Default::default()
        };
        let instance = AgentInstance::new(config, sm).unwrap();
        registry.register(instance).await;

        let bridge = AgentLoopBridge::new(
            registry,
            Arc::new(adapter),
            Arc::new(TaskStore::new()),
            Arc::new(StreamHub::new()),
        );
        (temp, bridge)
    }

    /// The peer on an A2A wire is a machine. There is no channel to render an
    /// approval card on and no human to press it, so a delegated run must fail
    /// closed on confirm-gated tools rather than park on the 120 s approval
    /// timeout for an approval that can never arrive.
    #[test]
    fn a_delegated_run_is_unattended() {
        use crate::gateway::execution_engine::UNATTENDED_KEY;
        let request = AgentLoopBridge::build_run_request("task-1", "hi");
        assert_eq!(
            request.metadata.get(UNATTENDED_KEY).map(String::as_str),
            Some("true")
        );
    }

    #[tokio::test]
    async fn handle_message_success() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "Hello, agent!");

        let task = bridge
            .handle_message("task-1", msg, None)
            .await
            .expect("should succeed");

        assert_eq!(task.id, "task-1");
        assert_eq!(task.status.state, TaskState::Completed);
        assert!(task.status.message.is_some());
    }

    #[tokio::test]
    async fn handle_message_failure_marks_task_failed() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::failing()).await;
        let msg = A2AMessage::text(A2ARole::User, "Do something");

        let task = bridge
            .handle_message("task-fail", msg, None)
            .await
            .expect("should return task even on execution failure");

        assert_eq!(task.status.state, TaskState::Failed);
        let status_text = task.status.message.unwrap().text_content();
        assert!(
            status_text.contains("failed"),
            "Error message should mention failure: {status_text}"
        );
    }

    #[tokio::test]
    async fn handle_message_empty_text_returns_error() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        // Message with no text parts
        let msg = A2AMessage {
            message_id: "m1".to_string(),
            role: A2ARole::User,
            parts: vec![Part::Data {
                data: serde_json::Map::new(),
                metadata: None,
            }],
            session_id: None,
            timestamp: None,
            metadata: None,
        };

        let result = bridge.handle_message("task-empty", msg, None).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), A2AError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn handle_message_whitespace_only_returns_error() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "   \n\t  ");

        let result = bridge.handle_message("task-ws", msg, None).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), A2AError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn handle_message_uses_session_id_as_context() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "With session");

        let task = bridge
            .handle_message("task-ctx", msg, Some("session-abc"))
            .await
            .expect("should succeed");

        assert_eq!(task.context_id, "session-abc");
    }

    #[tokio::test]
    async fn handle_message_uses_task_id_as_context_when_no_session() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "No session");

        let task = bridge
            .handle_message("task-nosess", msg, None)
            .await
            .expect("should succeed");

        assert_eq!(task.context_id, "task-nosess");
    }

    #[tokio::test]
    async fn handle_message_calls_adapter_once() {
        let adapter = MockExecutionAdapter::succeeding();
        let adapter_arc: Arc<MockExecutionAdapter> = Arc::new(adapter);

        let registry = Arc::new(AgentRegistry::new());
        let temp = tempfile::tempdir().unwrap();
        let sm = test_session_manager(&temp);
        let config = AgentInstanceConfig {
            agent_id: "main".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/main"),
            ..Default::default()
        };
        let instance = AgentInstance::new(config, sm).unwrap();
        registry.register(instance).await;

        let bridge = AgentLoopBridge::new(
            registry,
            adapter_arc.clone() as Arc<dyn ExecutionAdapter>,
            Arc::new(TaskStore::new()),
            Arc::new(StreamHub::new()),
        );

        let msg = A2AMessage::text(A2ARole::User, "Test call count");
        bridge
            .handle_message("task-count", msg, None)
            .await
            .unwrap();

        assert_eq!(adapter_arc.call_count().await, 1);
    }

    #[tokio::test]
    async fn handle_message_stream_returns_stream() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "Stream me");

        let mut stream = bridge
            .handle_message_stream("task-stream", msg, None)
            .await
            .expect("should return stream");

        // Read exactly 2 events (Working + Completed) with timeout
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("should receive first event within timeout")
            .expect("stream should not be empty")
            .expect("first event should be Ok");

        match &first {
            UpdateEvent::StatusUpdate(e) => {
                assert_eq!(e.status.state, TaskState::Working);
                assert!(!e.is_final);
            }
            _ => panic!("Expected Working StatusUpdate, got {:?}", first),
        }

        let second = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("should receive second event within timeout")
            .expect("stream should have second event")
            .expect("second event should be Ok");

        match &second {
            UpdateEvent::StatusUpdate(e) => {
                assert_eq!(e.status.state, TaskState::Completed);
                assert!(e.is_final);
            }
            _ => panic!("Expected Completed StatusUpdate, got {:?}", second),
        }
    }

    #[tokio::test]
    async fn handle_message_stream_failure_broadcasts_failed() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::failing()).await;
        let msg = A2AMessage::text(A2ARole::User, "Will fail");

        let mut stream = bridge
            .handle_message_stream("task-stream-fail", msg, None)
            .await
            .expect("should return stream");

        // Read Working event
        let _working = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("should receive Working event")
            .expect("stream should not be empty")
            .expect("event should be Ok");

        // Read Failed event
        let failed = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("should receive Failed event")
            .expect("stream should have second event")
            .expect("event should be Ok");

        match &failed {
            UpdateEvent::StatusUpdate(e) => {
                assert_eq!(e.status.state, TaskState::Failed);
                assert!(e.is_final);
            }
            _ => panic!("Expected Failed StatusUpdate, got {:?}", failed),
        }
    }

    #[tokio::test]
    async fn handle_message_stream_empty_text_returns_error() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage {
            message_id: "m1".to_string(),
            role: A2ARole::User,
            parts: vec![],
            session_id: None,
            timestamp: None,
            metadata: None,
        };

        let result = bridge.handle_message_stream("task-empty", msg, None).await;
        match result {
            Err(A2AError::InvalidParams(_)) => {} // expected
            Err(other) => panic!("Expected InvalidParams, got: {other}"),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[tokio::test]
    async fn handle_message_stream_whitespace_only_returns_error() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "   \n\t  ");

        let result = bridge.handle_message_stream("task-ws", msg, None).await;
        match result {
            Err(A2AError::InvalidParams(_)) => {}
            Err(other) => panic!("Expected InvalidParams, got: {other}"),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn build_run_request_uses_task_session_key() {
        let req = AgentLoopBridge::build_run_request("task-42", "hello");
        assert_eq!(req.input, "hello");
        assert!(!req.run_id.is_empty());
        match &req.session_key {
            SessionKey::Task {
                agent_id,
                task_type,
                task_id,
            } => {
                assert_eq!(agent_id, "main");
                assert_eq!(task_type, "a2a");
                assert_eq!(task_id, "task-42");
            }
            other => panic!("Expected Task session key, got {:?}", other),
        }
    }

    /// The ambient actor of an A2A streaming run is the literal AGENT id
    /// `"main"` that `build_run_request` pins — there is no machine sentinel.
    ///
    /// This pins the truth so the deleted `"a2a_peer"` claim cannot be
    /// re-derived from prose: an actor comparable against USER ids is what the
    /// visibility predicates expect, and an agent id makes every one of them
    /// answer false. Reads the production `TurnContext` builder rather than a
    /// re-typed literal, so dropping `a2a_turn_context`'s result would take the
    /// assertion with it.
    #[tokio::test]
    async fn a2a_streaming_run_acts_as_the_agent_id_not_a_machine_sentinel() {
        let request = AgentLoopBridge::build_run_request("task-actor", "hello");
        let turn = AgentLoopBridge::a2a_turn_context(&request);

        let actor = TURN_CONTEXT
            .scope(turn, async { crate::gateway::visibility::ambient_actor() })
            .await;

        assert_eq!(
            actor.as_deref(),
            Some("main"),
            "an A2A run acts as the agent id from its SessionKey; there is no \
             machine sentinel, and an agent id is not comparable against the \
             user ids the visibility predicates hold"
        );
    }

    #[test]
    fn context_id_prefers_session_id() {
        assert_eq!(
            AgentLoopBridge::context_id("task-1", Some("sess-x")),
            "sess-x"
        );
        assert_eq!(AgentLoopBridge::context_id("task-1", None), "task-1");
    }
}
