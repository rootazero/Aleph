//! AgentLoopBridge — bridges A2A protocol messages to Aleph's internal Agent Loop.
//!
//! Implements the `A2AMessageHandler` port by translating incoming `A2AMessage`
//! into `RunRequest`, executing via `ExecutionAdapter`, and mapping results
//! back to A2A task state transitions.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures::Stream;
use tracing::{error, info};

use crate::a2a::domain::*;
use crate::a2a::port::{A2AMessageHandler, A2AResult, A2AStreamingHandler, A2ATaskManager};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_emitter::NoOpEventEmitter;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::router::SessionKey;

/// Bridges A2A protocol messages to Aleph's Agent Loop execution.
///
/// Holds references to the execution infrastructure and task management ports,
/// translating between A2A domain concepts and Gateway execution primitives.
pub struct AgentLoopBridge {
    pub agent_registry: Arc<AgentRegistry>,
    pub execution_adapter: Arc<dyn ExecutionAdapter>,
    pub task_manager: Arc<dyn A2ATaskManager>,
    pub streaming: Arc<dyn A2AStreamingHandler>,
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
    fn build_run_request(task_id: &str, input: &str) -> RunRequest {
        RunRequest {
            run_id: uuid::Uuid::new_v4().to_string(),
            input: input.to_string(),
            session_key: SessionKey::task("main", "a2a", task_id),
            timeout_secs: None,
            metadata: HashMap::new(),
        }
    }

    /// Determine the context_id: use session_id if provided, otherwise task_id.
    fn context_id<'a>(task_id: &'a str, session_id: Option<&'a str>) -> &'a str {
        session_id.unwrap_or(task_id)
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
        if input.is_empty() {
            return Err(A2AError::InvalidParams(
                "Message contains no text content".to_string(),
            ));
        }

        // Create or reuse the task
        let context_id = Self::context_id(task_id, session_id);
        let _task = self.task_manager.create_task(task_id, context_id).await?;

        // Transition to Working
        self.task_manager
            .update_status(task_id, TaskState::Working, None)
            .await?;

        // Get the default agent
        let agent = self
            .agent_registry
            .get_default()
            .await
            .ok_or_else(|| A2AError::InternalError("No default agent registered".to_string()))?;

        // Build and execute the run request
        let request = Self::build_run_request(task_id, &input);
        let run_id = request.run_id.clone();
        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            Arc::new(NoOpEventEmitter::new());

        info!(task_id, run_id = %run_id, "A2A bridge: executing synchronous run");

        match self.execution_adapter.execute(request, agent, emitter).await {
            Ok(()) => {
                // Execution succeeded — mark task as Completed with a response message
                let response_msg = A2AMessage::text(A2ARole::Agent, "Task completed successfully");
                let task = self
                    .task_manager
                    .update_status(task_id, TaskState::Completed, Some(response_msg))
                    .await?;
                info!(task_id, "A2A bridge: task completed");
                Ok(task)
            }
            Err(e) => {
                // Execution failed — mark task as Failed
                let error_msg = A2AMessage::text(A2ARole::Agent, format!("Execution failed: {e}"));
                let task = self
                    .task_manager
                    .update_status(task_id, TaskState::Failed, Some(error_msg))
                    .await?;
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
        if input.is_empty() {
            return Err(A2AError::InvalidParams(
                "Message contains no text content".to_string(),
            ));
        }

        // Create or reuse the task
        let context_id = Self::context_id(task_id, session_id);
        let _task = self.task_manager.create_task(task_id, context_id).await?;

        // Subscribe to streaming updates BEFORE starting execution
        let stream = self.streaming.subscribe_all(task_id).await?;

        // Transition to Working and broadcast
        self.task_manager
            .update_status(task_id, TaskState::Working, None)
            .await?;

        let working_event = TaskStatusUpdateEvent {
            task_id: task_id.to_string(),
            context_id: context_id.to_string(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: false,
            metadata: None,
        };
        let _ = self
            .streaming
            .broadcast_status(task_id, working_event)
            .await;

        // Get the default agent
        let agent = self
            .agent_registry
            .get_default()
            .await
            .ok_or_else(|| A2AError::InternalError("No default agent registered".to_string()))?;

        // Build the run request
        let request = Self::build_run_request(task_id, &input);
        let run_id = request.run_id.clone();
        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            Arc::new(NoOpEventEmitter::new());

        info!(task_id, run_id = %run_id, "A2A bridge: executing streaming run");

        // Spawn execution in background
        let task_manager = Arc::clone(&self.task_manager);
        let streaming = Arc::clone(&self.streaming);
        let execution_adapter = Arc::clone(&self.execution_adapter);
        let task_id_owned = task_id.to_string();
        let context_id_owned = context_id.to_string();

        tokio::spawn(async move {
            match execution_adapter.execute(request, agent, emitter).await {
                Ok(()) => {
                    let response_msg =
                        A2AMessage::text(A2ARole::Agent, "Task completed successfully");
                    if let Err(e) = task_manager
                        .update_status(&task_id_owned, TaskState::Completed, Some(response_msg.clone()))
                        .await
                    {
                        error!(task_id = %task_id_owned, error = %e, "Failed to update task to Completed");
                        return;
                    }

                    let completed_event = TaskStatusUpdateEvent {
                        task_id: task_id_owned.clone(),
                        context_id: context_id_owned,
                        status: TaskStatus {
                            state: TaskState::Completed,
                            message: Some(response_msg),
                            timestamp: Utc::now(),
                        },
                        is_final: true,
                        metadata: None,
                    };
                    let _ = streaming
                        .broadcast_status(&task_id_owned, completed_event)
                        .await;
                    info!(task_id = %task_id_owned, "A2A bridge: streaming task completed");
                }
                Err(e) => {
                    let error_msg =
                        A2AMessage::text(A2ARole::Agent, format!("Execution failed: {e}"));
                    if let Err(update_err) = task_manager
                        .update_status(&task_id_owned, TaskState::Failed, Some(error_msg.clone()))
                        .await
                    {
                        error!(task_id = %task_id_owned, error = %update_err, "Failed to update task to Failed");
                        return;
                    }

                    let failed_event = TaskStatusUpdateEvent {
                        task_id: task_id_owned.clone(),
                        context_id: context_id_owned,
                        status: TaskStatus {
                            state: TaskState::Failed,
                            message: Some(error_msg),
                            timestamp: Utc::now(),
                        },
                        is_final: true,
                        metadata: None,
                    };
                    let _ = streaming
                        .broadcast_status(&task_id_owned, failed_event)
                        .await;
                    error!(task_id = %task_id_owned, error = %e, "A2A bridge: streaming task failed");
                }
            }
        });

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::a2a::adapter::server::StreamHub;
    use crate::a2a::adapter::server::TaskStore;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_engine::ExecutionError;

    use futures::StreamExt;

    /// Mock execution adapter that tracks calls
    struct MockExecutionAdapter {
        should_fail: bool,
        calls: Mutex<Vec<String>>,
    }

    impl MockExecutionAdapter {
        fn succeeding() -> Self {
            Self {
                should_fail: false,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn failing() -> Self {
            Self {
                should_fail: true,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
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
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.run_id);
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
    }

    /// Helper to create a test bridge with real TaskStore/StreamHub and mock adapter
    async fn make_bridge(adapter: MockExecutionAdapter) -> AgentLoopBridge {
        let registry = Arc::new(AgentRegistry::new());
        let temp = tempfile::tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "main".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/main"),
            ..Default::default()
        };
        let instance = AgentInstance::new(config).unwrap();
        registry.register(instance).await;

        AgentLoopBridge::new(
            registry,
            Arc::new(adapter),
            Arc::new(TaskStore::new()),
            Arc::new(StreamHub::new()),
        )
    }

    #[tokio::test]
    async fn handle_message_success() {
        let bridge = make_bridge(MockExecutionAdapter::succeeding()).await;
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
        let bridge = make_bridge(MockExecutionAdapter::failing()).await;
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
        let bridge = make_bridge(MockExecutionAdapter::succeeding()).await;
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
    async fn handle_message_uses_session_id_as_context() {
        let bridge = make_bridge(MockExecutionAdapter::succeeding()).await;
        let msg = A2AMessage::text(A2ARole::User, "With session");

        let task = bridge
            .handle_message("task-ctx", msg, Some("session-abc"))
            .await
            .expect("should succeed");

        assert_eq!(task.context_id, "session-abc");
    }

    #[tokio::test]
    async fn handle_message_uses_task_id_as_context_when_no_session() {
        let bridge = make_bridge(MockExecutionAdapter::succeeding()).await;
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
        let config = AgentInstanceConfig {
            agent_id: "main".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/main"),
            ..Default::default()
        };
        let instance = AgentInstance::new(config).unwrap();
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

        assert_eq!(adapter_arc.call_count(), 1);
    }

    #[tokio::test]
    async fn handle_message_stream_returns_stream() {
        let bridge = make_bridge(MockExecutionAdapter::succeeding()).await;
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
        let bridge = make_bridge(MockExecutionAdapter::failing()).await;
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
        let bridge = make_bridge(MockExecutionAdapter::succeeding()).await;
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

    #[test]
    fn context_id_prefers_session_id() {
        assert_eq!(
            AgentLoopBridge::context_id("task-1", Some("sess-x")),
            "sess-x"
        );
        assert_eq!(AgentLoopBridge::context_id("task-1", None), "task-1");
    }
}
