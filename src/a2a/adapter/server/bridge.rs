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
use futures::{FutureExt, Stream};
use tracing::info;

use crate::a2a::domain::{
    A2AError, A2AMessage, A2ARole, A2ATask, Artifact, Part, TaskArtifactUpdateEvent, TaskState,
    TaskStatusUpdateEvent, UpdateEvent,
};
use crate::a2a::port::{A2AMessageHandler, A2AResult, A2AStreamingHandler};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{
    BUSY_INPUT_MODE_KEY, ExecutionError, RunRequest, UNATTENDED_KEY,
};
use crate::gateway::router::SessionKey;
use crate::tools::turn_context::{TURN_CONTEXT, TurnContext};

use super::task_store::{TaskClaim, TaskStore};

/// Fallback response text when a run finished without a final response.
const DEFAULT_COMPLETION_TEXT: &str = "Task completed successfully";

/// Bridges A2A protocol messages to Aleph's Agent Loop execution.
///
/// Holds references to the execution infrastructure and task management ports,
/// translating between A2A domain concepts and Gateway execution primitives.
#[derive(Clone)]
pub struct AgentLoopBridge {
    pub(crate) agent_registry: Arc<AgentRegistry>,
    pub(crate) execution_adapter: Arc<dyn ExecutionAdapter>,
    pub(crate) task_manager: Arc<TaskStore>,
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
            *self
                .final_response
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = summary.final_response;
        }
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

impl AgentLoopBridge {
    /// Create a new bridge with all required dependencies.
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        execution_adapter: Arc<dyn ExecutionAdapter>,
        task_manager: Arc<TaskStore>,
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
    fn build_run_request(task_id: &str, run_id: uuid::Uuid, input: &str) -> RunRequest {
        RunRequest {
            run_id: run_id.to_string(),
            input: input.to_string(),
            session_key: SessionKey::task("main", "a2a", task_id),
            timeout_secs: None,
            metadata: HashMap::from([
                (UNATTENDED_KEY.into(), "true".into()),
                (BUSY_INPUT_MODE_KEY.into(), "queue".into()),
            ]),
            attachments: Vec::new(),
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
            telecom_public: None,
            telecom_authority: None,
        }
    }

    /// Both legacy send faces have the same unattended tool and visibility scope.
    /// The ambient actor is the default agent ID. A credential scope in A2A's
    /// task authority does not grant access to per-person Core resources.
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

    async fn start(
        &self,
        claim: TaskClaim,
        message: A2AMessage,
        subscribe: bool,
    ) -> A2AResult<(
        tokio::task::JoinHandle<A2AResult<A2ATask>>,
        Option<TaskUpdateStream>,
    )> {
        let input = message.text_content();
        if input.trim().is_empty() {
            return Err(A2AError::InvalidParams(
                "Message contains no text content".into(),
            ));
        }
        let (stream_tx, stream_rx) = tokio::sync::oneshot::channel();
        let bridge = self.clone();
        let handle = tokio::spawn(async move {
            if subscribe {
                let stream = bridge.streaming.subscribe_all(&claim.instance_id).await;
                let _ = stream_tx.send(stream);
            }
            let working = TaskStatusUpdateEvent::new(
                claim.task.id.clone(),
                claim.task.context_id.clone(),
                claim.task.status.clone(),
                None,
            );
            if let Err(error) = bridge
                .streaming
                .broadcast_status(&claim.instance_id, working)
                .await
            {
                tracing::warn!(%error, "A2A working update could not be published");
            }
            let emitter = Arc::new(FinalResponseEmitter::default());
            let execution = async {
                let agent = bridge.agent_registry.get_default().await.ok_or_else(|| {
                    crate::gateway::execution_engine::ExecutionError::Failed(
                        "No default agent registered".into(),
                    )
                })?;
                let request = Self::build_run_request(&claim.session_id, claim.run_id, &input);
                let turn = Self::a2a_turn_context(&request);
                info!(run_id = %request.run_id, task_id = %claim.task.id, "Executing A2A task");
                TURN_CONTEXT
                    .scope(
                        turn,
                        bridge.execution_adapter.execute_chat_run(
                            request,
                            agent,
                            emitter.clone(),
                            claim.cancellation.clone(),
                        ),
                    )
                    .await
            };
            let outcome = AssertUnwindSafe(execution).catch_unwind().await;
            let (state, text) = match outcome {
                Ok(Ok(())) => (
                    TaskState::Completed,
                    emitter
                        .take_final_response()
                        .unwrap_or_else(|| DEFAULT_COMPLETION_TEXT.into()),
                ),
                Ok(Err(ExecutionError::Cancelled)) => (TaskState::Canceled, "Task canceled".into()),
                Ok(Err(error)) => (TaskState::Failed, format!("Execution failed: {error}")),
                Err(_) => (TaskState::Failed, "Execution failed unexpectedly".into()),
            };
            let result = bridge.finish(&claim, state, text).await;
            if let Err(error) = bridge.streaming.cleanup_task(&claim.instance_id).await {
                tracing::warn!(%error, "A2A task stream cleanup failed");
            }
            bridge.task_manager.release_run(&claim).await;
            result
        });
        let stream =
            if subscribe {
                Some(stream_rx.await.map_err(|_| {
                    A2AError::InternalError("A2A stream could not be opened".into())
                })??)
            } else {
                None
            };
        Ok((handle, stream))
    }

    async fn finish(
        &self,
        claim: &TaskClaim,
        state: TaskState,
        text: String,
    ) -> A2AResult<A2ATask> {
        let message = A2AMessage::text(A2ARole::Agent, &text);
        let artifact = (state == TaskState::Completed).then(|| Artifact {
            artifact_id: uuid::Uuid::new_v4().to_string(),
            kind: "text".into(),
            parts: vec![Part::Text {
                text,
                metadata: None,
            }],
            metadata: None,
        });
        let task = self
            .task_manager
            .finish_run(claim, state, Some(message), artifact.clone())
            .await?;
        if let Some(artifact) = artifact {
            let event = TaskArtifactUpdateEvent {
                task_id: task.id.clone(),
                context_id: task.context_id.clone(),
                artifact,
                append: false,
                last_chunk: true,
                metadata: None,
            };
            if let Err(error) = self
                .streaming
                .broadcast_artifact(&claim.instance_id, event)
                .await
            {
                tracing::warn!(%error, "A2A artifact update could not be published");
            }
        }
        let event = TaskStatusUpdateEvent::new(
            task.id.clone(),
            task.context_id.clone(),
            task.status.clone(),
            None,
        );
        if let Err(error) = self
            .streaming
            .broadcast_status(&claim.instance_id, event)
            .await
        {
            tracing::warn!(%error, "A2A terminal update could not be published");
        }
        Ok(task)
    }
}

type TaskUpdateStream = Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>;

#[async_trait]
impl A2AMessageHandler for AgentLoopBridge {
    async fn handle_message(&self, claim: TaskClaim, message: A2AMessage) -> A2AResult<A2ATask> {
        let (run, _) = self.start(claim, message, false).await?;
        run.await
            .map_err(|e| A2AError::InternalError(format!("A2A run join failed: {e}")))?
    }

    async fn handle_message_stream(
        &self,
        claim: TaskClaim,
        message: A2AMessage,
    ) -> A2AResult<TaskUpdateStream> {
        let (_, stream) = self.start(claim, message, true).await?;
        Ok(stream.expect("stream requested"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::a2a::adapter::server::StreamHub;
    use crate::a2a::adapter::server::TaskStore;
    use crate::a2a::port::authenticator::TaskOwner;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::event_emitter::EventEmitter;

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

        async fn execute_chat_run(
            &self,
            request: RunRequest,
            agent: Arc<AgentInstance>,
            emitter: Arc<dyn EventEmitter + Send + Sync>,
            cancellation: crate::gateway::execution_adapter::ChatRunCancellation,
        ) -> Result<(), ExecutionError> {
            cancellation.check()?;
            self.execute(request, agent, emitter).await
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

    async fn claim(bridge: &AgentLoopBridge, alias: &str, context: Option<&str>) -> TaskClaim {
        bridge
            .task_manager
            .claim_legacy(TaskOwner::Loopback, alias, context, None)
            .await
            .unwrap()
    }

    /// The peer on an A2A wire is a machine. There is no channel to render an
    /// approval card on and no human to press it, so a delegated run must fail
    /// closed on confirm-gated tools rather than park on the 120 s approval
    /// timeout for an approval that can never arrive.
    #[test]
    fn a_delegated_run_is_unattended() {
        use crate::gateway::execution_engine::UNATTENDED_KEY;
        let request = AgentLoopBridge::build_run_request("task-1", uuid::Uuid::new_v4(), "hi");
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
            .handle_message(claim(&bridge, "task-1", None).await, msg)
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
            .handle_message(claim(&bridge, "task-fail", None).await, msg)
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

        let result = bridge
            .handle_message(claim(&bridge, "task-empty", None).await, msg)
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), A2AError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn handle_message_whitespace_only_returns_error() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "   \n\t  ");

        let result = bridge
            .handle_message(claim(&bridge, "task-ws", None).await, msg)
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), A2AError::InvalidParams(_)));
    }

    #[tokio::test]
    async fn handle_message_uses_session_id_as_context() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "With session");

        let task = bridge
            .handle_message(claim(&bridge, "task-ctx", Some("session-abc")).await, msg)
            .await
            .expect("should succeed");

        assert_eq!(task.context_id, "session-abc");
    }

    #[tokio::test]
    async fn handle_message_uses_task_id_as_context_when_no_session() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "No session");

        let task = bridge
            .handle_message(claim(&bridge, "task-nosess", None).await, msg)
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
            .handle_message(claim(&bridge, "task-count", None).await, msg)
            .await
            .unwrap();

        assert_eq!(adapter_arc.call_count().await, 1);
    }

    #[tokio::test]
    async fn handle_message_stream_returns_stream() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "Stream me");

        let mut stream = bridge
            .handle_message_stream(claim(&bridge, "task-stream", None).await, msg)
            .await
            .expect("should return stream");

        // A successful run publishes its artifact before terminal status.
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

        assert!(
            matches!(second, UpdateEvent::ArtifactUpdate(ref event) if event.task_id == "task-stream")
        );
        let third = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match &third {
            UpdateEvent::StatusUpdate(e) => {
                assert_eq!(e.task_id, "task-stream");
                assert_eq!(e.status.state, TaskState::Completed);
                assert!(e.is_final);
            }
            _ => panic!("Expected Completed StatusUpdate, got {:?}", third),
        }
    }

    #[tokio::test]
    async fn handle_message_stream_failure_broadcasts_failed() {
        let (_scratch, bridge) = make_bridge(MockExecutionAdapter::failing()).await;
        let msg = A2AMessage::text(A2ARole::User, "Will fail");

        let mut stream = bridge
            .handle_message_stream(claim(&bridge, "task-stream-fail", None).await, msg)
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

        let result = bridge
            .handle_message_stream(claim(&bridge, "task-empty", None).await, msg)
            .await;
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

        let result = bridge
            .handle_message_stream(claim(&bridge, "task-ws", None).await, msg)
            .await;
        match result {
            Err(A2AError::InvalidParams(_)) => {}
            Err(other) => panic!("Expected InvalidParams, got: {other}"),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn build_run_request_uses_task_session_key() {
        let req = AgentLoopBridge::build_run_request("task-42", uuid::Uuid::new_v4(), "hello");
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
        let request =
            AgentLoopBridge::build_run_request("task-actor", uuid::Uuid::new_v4(), "hello");
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
}
