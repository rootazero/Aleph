//! `A2ASubAgent` — A2A remote delegation engine
//!
//! Integrates `SmartRouter` (routing) + `A2AClientPool` (communication) to delegate
//! tasks to remote agents via the A2A protocol.

#[cfg(test)]
use async_trait::async_trait;

use crate::a2a::adapter::client::{fold_stream, A2AClient, A2AClientPool};
use crate::a2a::domain::{A2AMessage, A2ARole};
use crate::a2a::port::RegisteredAgent;
use crate::a2a::service::SmartRouter;
use crate::agents::sub_agents::{SubAgentRequest, SubAgentResult};
use crate::memory::extensions::types::CaptureCtx;
use crate::memory::extensions::{insert_with_capture_filter, MemoryExtensionRegistry};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::sync_primitives::Arc;

/// `SubAgent` implementation that delegates tasks to remote A2A agents.
///
/// Uses `SmartRouter` for intent-based agent discovery and `A2AClientPool`
/// for managing HTTP connections to remote agents.
pub struct A2ASubAgent {
    smart_router: Arc<SmartRouter>,
    client_pool: Arc<A2AClientPool>,
    /// Optional writer for raw memory delegation hooks (Spec 1 G2).
    /// When set, `execute` will write a `RawMemory(Delegation{child_agent_id})`
    /// row before returning a successful result.
    raw_memory_writer: Option<Arc<dyn RawMemoryStore>>,
    /// Optional capture-filter registry (Spec 4 Task 6).
    /// When set, delegation raw-memory writes go through `insert_with_capture_filter`.
    /// Task 11 wires the real registry at startup; `None` falls back to direct insert.
    capture_registry: Option<Arc<MemoryExtensionRegistry>>,
}

/// Outcome of an A2A delegation via [`A2ASubAgent::execute_delegation`].
///
/// Unlike the bare [`SubAgentResult`], this also reports *which* remote agent
/// handled the task, so the `a2a_delegate` tool can surface it to the model.
#[derive(Debug, Clone)]
pub struct DelegationOutcome {
    /// Name of the remote agent that handled the task, or `None` when routing
    /// found no matching agent.
    pub agent: Option<String>,
    /// The delegation result (success summary or failure error).
    pub result: SubAgentResult,
}

impl A2ASubAgent {
    pub fn new(smart_router: Arc<SmartRouter>, client_pool: Arc<A2AClientPool>) -> Self {
        Self {
            smart_router,
            client_pool,
            raw_memory_writer: None,
            capture_registry: None,
        }
    }

    /// Attach an optional raw-memory writer for delegation hooks (Spec 1 G2).
    ///
    /// When set, `execute` will write a `RawMemory(Delegation{child_agent_id})`
    /// row carrying the delegation prompt + sub-agent summary before returning,
    /// allowing `CompressionService` to distil durable lessons for the parent agent.
    pub fn with_raw_memory_writer(mut self, writer: Arc<dyn RawMemoryStore>) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// Attach a capture-filter registry (Spec 4 Task 6).
    ///
    /// When set, delegation raw-memory writes go through `insert_with_capture_filter`.
    /// Task 11 wires the real registry at startup; `None` falls back to direct insert.
    pub fn with_capture_registry(mut self, registry: Arc<MemoryExtensionRegistry>) -> Self {
        self.capture_registry = Some(registry);
        self
    }

    /// Send a delegation request to an already-resolved remote agent.
    ///
    /// Streaming-first: consumes the remote agent's SSE stream (idle-timeout
    /// liveness + live progress). When the remote has no `/a2a/stream` route
    /// (non-Aleph agents), transparently falls back to [`Self::dispatch_sync`].
    /// Used by [`Self::execute_delegation`].
    async fn dispatch(
        &self,
        agent: &RegisteredAgent,
        request: &SubAgentRequest,
    ) -> crate::error::Result<SubAgentResult> {
        let client = self.client_pool.get_or_create(agent).await.map_err(|e| {
            crate::error::AlephError::other(format!("A2A client creation failed: {e}"))
        })?;

        let message = A2AMessage::text(A2ARole::User, &request.prompt);
        let task_id = uuid::Uuid::new_v4().to_string();

        match client.send_message_stream(&task_id, &message, None).await {
            Ok(stream) => {
                let outcome = fold_stream(stream, |chunk| {
                    crate::builtin_tools::notify_tool_streaming_chunk("a2a_delegate", chunk);
                })
                .await;

                let result = if outcome.success {
                    SubAgentResult::success(request.id.clone(), outcome.summary)
                } else {
                    SubAgentResult::failure(
                        request.id.clone(),
                        outcome
                            .error
                            .unwrap_or_else(|| "A2A streaming delegation failed".to_string()),
                    )
                };

                // Spec 1 G2: record delegation outcome for parent-agent memory.
                // Record unconditionally (matching sync path behaviour) so the
                // parent sees failed attempts and can learn / retry.
                if let Some(w) = self.raw_memory_writer.clone() {
                    emit_delegation_raw_with_registry(
                        w,
                        request,
                        &result,
                        &agent.card.id,
                        self.capture_registry.clone(),
                    );
                }
                Ok(result)
            }
            Err(e) => {
                tracing::info!(
                    error = %e,
                    "A2A streaming unavailable; falling back to sync send_message"
                );
                self.dispatch_sync(&client, agent, request, &task_id, &message)
                    .await
            }
        }
    }

    /// Synchronous delegation — POSTs `message/send` and waits for the full
    /// task. Fallback for remote agents without a streaming endpoint.
    async fn dispatch_sync(
        &self,
        client: &A2AClient,
        agent: &RegisteredAgent,
        request: &SubAgentRequest,
        task_id: &str,
        message: &A2AMessage,
    ) -> crate::error::Result<SubAgentResult> {
        match client.send_message(task_id, message, None).await {
            Ok(task) => {
                let summary = if !task.history.is_empty() {
                    task.history
                        .iter()
                        .rev()
                        .find(|m| m.role == A2ARole::Agent)
                        .map_or_else(
                            || format!("Task {} completed", task.id),
                            |m| m.text_content(),
                        )
                } else if let Some(ref msg) = task.status.message {
                    msg.text_content()
                } else {
                    format!(
                        "Task {} completed with state: {:?}",
                        task.id, task.status.state
                    )
                };

                let output = serde_json::to_value(&task).unwrap_or_else(|e| {
                    tracing::warn!("Failed to serialize A2ATask: {}", e);
                    serde_json::Value::Null
                });
                // A transport-level success can still carry a task that ended in
                // a failed terminal state — mirror fold_stream (streaming path)
                // and report it as a failure instead of a false success.
                let failed = matches!(
                    task.status.state,
                    crate::a2a::domain::TaskState::Failed
                        | crate::a2a::domain::TaskState::Rejected
                        | crate::a2a::domain::TaskState::Canceled
                );
                let result = if failed {
                    SubAgentResult::failure(request.id.clone(), summary).with_output(output)
                } else {
                    SubAgentResult::success(request.id.clone(), summary).with_output(output)
                };

                // Spec 1 G2: record delegation outcome for parent-agent memory.
                if let Some(w) = self.raw_memory_writer.clone() {
                    emit_delegation_raw_with_registry(
                        w,
                        request,
                        &result,
                        &agent.card.id,
                        self.capture_registry.clone(),
                    );
                }

                Ok(result)
            }
            Err(e) => Ok(SubAgentResult::failure(
                request.id.clone(),
                format!("A2A call failed: {e}"),
            )),
        }
    }

    /// Delegate a task to a remote A2A agent — the entry point for the
    /// `a2a_delegate` builtin tool.
    ///
    /// When `agent` is `Some`, the delegation is pinned to the agent whose id
    /// or name matches case-insensitively. When `None`, [`SmartRouter`] selects
    /// the best match from the prompt. A missing target is reported as a failed
    /// [`DelegationOutcome`] (not an `Err`) so the caller can surface a clean
    /// message to the model.
    ///
    /// `parent_agent_id` / `parent_session_id` identify the delegating local
    /// agent turn; they are stamped onto the emitted `RawMemory(Delegation)`
    /// row so per-agent memory attribution matches the intra-process spawner
    /// (`None` falls back to `"default"`, the legacy behaviour).
    pub async fn execute_delegation(
        &self,
        prompt: &str,
        agent: Option<&str>,
        parent_agent_id: Option<String>,
        parent_session_id: Option<String>,
    ) -> crate::error::Result<DelegationOutcome> {
        let target = match agent {
            Some(name) => {
                let needle = name.trim().to_lowercase();
                let agents = self.smart_router.list_agents().await.map_err(|e| {
                    crate::error::AlephError::other(format!("A2A agent lookup failed: {e}"))
                })?;
                agents.into_iter().find(|a| {
                    a.card.id.to_lowercase() == needle || a.card.name.to_lowercase() == needle
                })
            }
            None => self
                .smart_router
                .route(prompt)
                .await
                .map_err(|e| crate::error::AlephError::other(format!("A2A routing failed: {e}")))?
                .map(|d| {
                    tracing::info!(
                        agent = %d.agent.card.name,
                        confidence = %d.confidence,
                        method = ?d.method,
                        "Routed to remote agent"
                    );
                    d.agent
                }),
        };

        let target = match target {
            Some(t) => t,
            None => {
                let msg = match agent {
                    Some(name) => format!("No A2A agent registered matching '{name}'"),
                    None => "No matching A2A agent found for this request".to_string(),
                };
                return Ok(DelegationOutcome {
                    agent: None,
                    result: SubAgentResult::failure(uuid::Uuid::new_v4().to_string(), msg),
                });
            }
        };

        let mut request = SubAgentRequest::new(prompt);
        if let Some(aid) = parent_agent_id {
            request = request.with_parent_agent(aid);
        }
        if let Some(sid) = parent_session_id {
            request = request.with_parent_session(sid);
        }
        let result = self.dispatch(&target, &request).await?;
        Ok(DelegationOutcome {
            agent: Some(target.card.name.clone()),
            result,
        })
    }
}

/// Emit a `RawMemory(Delegation{child_agent_id})` row in a fire-and-forget spawn.
///
/// Carries the delegation prompt and sub-agent summary so `CompressionService`
/// can distil durable lessons for the parent agent's long-term memory.
/// The parent `agent_id` is taken from `request.parent_agent_id`
/// (falls back to `"default"` when absent — direct callers with no turn context).
#[allow(dead_code)] // test-only helper
pub(crate) fn emit_delegation_raw(
    writer: Arc<dyn RawMemoryStore>,
    request: &SubAgentRequest,
    result: &SubAgentResult,
    child_agent_id: impl Into<String>,
) {
    emit_delegation_raw_with_registry(writer, request, result, child_agent_id, None);
}

/// SubAgentRequest/Result shaped wrapper around `emit_delegation_primitives`.
/// Kept as the entry point for the A2A path which still has those types in
/// scope; the new harness-based `subagent_spawner` calls the primitive helper
/// directly to avoid coupling on a2a request/result shapes.
pub(crate) fn emit_delegation_raw_with_registry(
    writer: Arc<dyn RawMemoryStore>,
    request: &SubAgentRequest,
    result: &SubAgentResult,
    child_agent_id: impl Into<String>,
    registry: Option<Arc<MemoryExtensionRegistry>>,
) {
    let parent_agent_id = request
        .parent_agent_id
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let parent_session_id = request.parent_session_id.clone();
    emit_delegation_primitives(
        writer,
        request.prompt.clone(),
        result.summary.clone(),
        parent_agent_id,
        parent_session_id,
        child_agent_id.into(),
        registry,
    );
}

/// Primitive-arg core of the G2 hook — agnostic to a2a request/result types.
///
/// Public-in-crate so the harness-based subagent spawner (intra-process
/// delegation, post-phase7 traffic flip) can emit the same `RawMemory(Delegation)`
/// row that the A2A cross-process path emits. Without this hook the parent
/// agent loses every lesson its local subagents learn.
///
/// Spawns a fire-and-forget tokio task to write the row and (optionally) run
/// it through the `MemoryExtensionRegistry` capture filter. Logs and returns
/// when no tokio runtime is available rather than panicking.
pub(crate) fn emit_delegation_primitives(
    writer: Arc<dyn RawMemoryStore>,
    prompt: String,
    summary: String,
    parent_agent_id: String,
    parent_session_id: Option<String>,
    child_agent_id: String,
    registry: Option<Arc<MemoryExtensionRegistry>>,
) {
    let content = format!("DELEGATION_PROMPT:\n{prompt}\n\nDELEGATION_RESULT:\n{summary}",);

    let mut raw = RawMemory::new(content, RawMemorySource::Delegation { child_agent_id })
        .with_agent(parent_agent_id.clone());

    if let Some(sid) = parent_session_id.clone() {
        raw = raw.with_session(sid);
    }

    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(async move {
            if let Some(reg) = registry {
                let ctx = CaptureCtx {
                    agent_id: parent_agent_id,
                    namespace: NamespaceScope::Owner,
                    session_id: parent_session_id,
                    source_hint: "delegation".into(),
                };
                if let Err(e) = insert_with_capture_filter(&writer, &reg, &ctx, raw).await {
                    tracing::warn!("delegation raw_memory write failed: {e}");
                }
            } else if let Err(e) = writer.insert_raw_memory(&raw).await {
                tracing::warn!("delegation raw_memory write failed: {e}");
            }
        });
    } else {
        tracing::warn!("no tokio runtime for delegation emit; skipping");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::*;
    use crate::a2a::port::{A2AResult, AgentHealth, AgentResolver, RegisteredAgent};
    // --- Mock AgentResolver for SmartRouter ---

    struct MockResolver {
        agents: tokio::sync::Mutex<Vec<RegisteredAgent>>,
    }

    impl MockResolver {
        fn new(agents: Vec<RegisteredAgent>) -> Self {
            Self {
                agents: tokio::sync::Mutex::new(agents),
            }
        }
    }

    #[async_trait]
    impl AgentResolver for MockResolver {
        async fn register(
            &self,
            _card: AgentCard,
            _base_url: &str,
            _trust_level: TrustLevel,
        ) -> A2AResult<()> {
            Ok(())
        }

        async fn unregister(&self, _agent_id: &str) -> A2AResult<()> {
            Ok(())
        }

        async fn list_agents(&self) -> A2AResult<Vec<RegisteredAgent>> {
            let agents = self.agents.lock().await;
            Ok(agents.clone())
        }

        async fn resolve_by_id(&self, _agent_id: &str) -> A2AResult<Option<RegisteredAgent>> {
            Ok(None)
        }
    }

    fn build_sub_agent(agents: Vec<RegisteredAgent>) -> A2ASubAgent {
        let resolver = Arc::new(MockResolver::new(agents));
        let router = Arc::new(SmartRouter::new(resolver));
        let pool = Arc::new(A2AClientPool::new());
        A2ASubAgent::new(router, pool)
    }

    #[tokio::test]
    async fn execute_delegation_no_agents_returns_outcome_without_agent() {
        let agent = build_sub_agent(vec![]);
        let outcome = agent
            .execute_delegation("do something", None, None, None)
            .await
            .unwrap();
        assert!(outcome.agent.is_none());
        assert!(!outcome.result.success);
        assert!(outcome
            .result
            .error
            .as_ref()
            .unwrap()
            .contains("No matching A2A agent"));
    }

    #[tokio::test]
    async fn execute_delegation_explicit_unknown_agent_reports_name() {
        let agent = build_sub_agent(vec![]);
        let outcome = agent
            .execute_delegation("do something", Some("ghost-agent"), None, None)
            .await
            .unwrap();
        assert!(outcome.agent.is_none());
        assert!(!outcome.result.success);
        assert!(outcome
            .result
            .error
            .as_ref()
            .unwrap()
            .contains("ghost-agent"));
    }

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn streaming_registered(name: &str, url: &str) -> RegisteredAgent {
        RegisteredAgent {
            card: AgentCard {
                id: "streamer".to_string(),
                name: name.to_string(),
                version: "1.0".to_string(),
                description: None,
                provider: None,
                documentation_url: None,
                interfaces: vec![],
                skills: vec![],
                security: vec![],
                extensions: vec![],
                default_input_modes: vec![],
                default_output_modes: vec![],
            },
            trust_level: TrustLevel::Trusted,
            base_url: url.to_string(),
            last_seen: chrono::Utc::now(),
            health: AgentHealth::Healthy,
            auth_token: None,
        }
    }

    fn sse_completed_body(answer: &str) -> String {
        let completed = UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(A2AMessage::text(A2ARole::Agent, answer)),
                timestamp: chrono::Utc::now(),
            },
            is_final: true,
            metadata: None,
        });
        let env = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": completed}).to_string();
        format!("event: status-update\ndata: {}\n\n", env)
    }

    #[tokio::test]
    async fn execute_delegation_streams_remote_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                sse_completed_body("streamed answer 42"),
                "text/event-stream",
            ))
            .mount(&server)
            .await;

        let sub = build_sub_agent(vec![streaming_registered("Streamer", &server.uri())]);
        let outcome = sub
            .execute_delegation("do the thing", Some("Streamer"), None, None)
            .await
            .unwrap();
        assert!(outcome.result.success, "got: {:?}", outcome.result);
        assert_eq!(outcome.agent.as_deref(), Some("Streamer"));
        assert!(outcome.result.summary.contains("streamed answer 42"));
    }

    #[tokio::test]
    async fn execute_delegation_falls_back_to_sync_when_no_stream_route() {
        let server = MockServer::start().await;
        // No streaming endpoint.
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // Synchronous JSON-RPC endpoint returns a completed task.
        let mut task = A2ATask::new("t", "c");
        task.status.state = TaskState::Completed;
        task.history
            .push(A2AMessage::text(A2ARole::Agent, "sync answer 99"));
        let rpc = serde_json::json!({"jsonrpc": "2.0", "id": "x", "result": task});
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(rpc))
            .mount(&server)
            .await;

        let sub = build_sub_agent(vec![streaming_registered("Streamer", &server.uri())]);
        let outcome = sub
            .execute_delegation("do it", Some("Streamer"), None, None)
            .await
            .unwrap();
        assert!(outcome.result.success, "got: {:?}", outcome.result);
        assert!(outcome.result.summary.contains("sync answer 99"));
    }
}

#[cfg(test)]
mod spec1_tests {
    use super::*;
    use crate::agents::sub_agents::SubAgentRequest;
    use crate::error::AlephError;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::sync_primitives::Arc;

    #[derive(Default)]
    struct FakeWriter(tokio::sync::Mutex<Vec<RawMemory>>);

    #[async_trait::async_trait]
    impl RawMemoryStore for FakeWriter {
        async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
            self.0.lock().await.push(raw.clone());
            Ok(())
        }

        async fn get_unprocessed_raw_memories(
            &self,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(vec![])
        }

        async fn mark_raw_as_processed(&self, _ids: &[String]) -> Result<usize, AlephError> {
            Ok(0)
        }

        async fn count_unprocessed(&self, _agent_id: &str) -> Result<usize, AlephError> {
            Ok(0)
        }

        async fn get_raw_by_path_prefix(
            &self,
            _path_prefix: &str,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn emit_delegation_raw_writes_expected_row() {
        let fake = Arc::new(FakeWriter::default());
        let writer: Arc<dyn RawMemoryStore> = fake.clone();

        let request = SubAgentRequest::new("Summarise the trading report")
            .with_parent_session("parent-session-42");
        let result = crate::agents::sub_agents::SubAgentResult::success(
            request.id.clone(),
            "Completed: found 3 key insights",
        );

        emit_delegation_raw(writer, &request, &result, "trading-agent-id");

        // Fire-and-forget: let the spawned task complete.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let captured = fake.0.lock().await;
        assert_eq!(captured.len(), 1, "expected exactly one RawMemory row");

        let row = &captured[0];
        match &row.source {
            RawMemorySource::Delegation { child_agent_id } => {
                assert_eq!(child_agent_id, "trading-agent-id");
            }
            other => panic!("expected Delegation source, got {:?}", other),
        }

        assert!(
            row.content.contains("Summarise the trading report"),
            "content should include delegation prompt"
        );
        assert!(
            row.content.contains("Completed: found 3 key insights"),
            "content should include delegation result summary"
        );
        assert_eq!(
            row.session_id,
            Some("parent-session-42".to_string()),
            "session_id should match parent_session_id"
        );
        assert_eq!(
            row.agent_id, "default",
            "agent_id falls back to 'default' when no parent_agent_id in metadata"
        );
    }

    #[tokio::test]
    async fn emit_delegation_raw_uses_request_parent_agent_id() {
        let fake = Arc::new(FakeWriter::default());
        let writer: Arc<dyn RawMemoryStore> = fake.clone();

        let request = SubAgentRequest::new("Do the thing")
            .with_parent_session("sess-99")
            .with_parent_agent("parent-agent-007");
        let result = crate::agents::sub_agents::SubAgentResult::success(request.id.clone(), "Done");

        emit_delegation_raw(writer, &request, &result, "child-007");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let captured = fake.0.lock().await;
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].agent_id, "parent-agent-007");
    }

    /// W16 regression — `execute_delegation` must thread the caller's parent
    /// identity into the emitted `RawMemory(Delegation)` row. Before the fix
    /// the request carried no parent identity, so every outbound A2A
    /// delegation row landed under agent `"default"`.
    #[tokio::test]
    async fn execute_delegation_stamps_parent_identity_on_delegation_row() {
        use crate::a2a::adapter::client::A2AClientPool;
        use crate::a2a::domain::{
            A2AMessage, A2ARole, AgentCard, TaskState, TaskStatus, TaskStatusUpdateEvent,
            TrustLevel, UpdateEvent,
        };
        use crate::a2a::port::{AgentHealth, RegisteredAgent};
        use crate::a2a::service::{CardRegistry, SmartRouter};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let completed = UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(A2AMessage::text(A2ARole::Agent, "remote done")),
                timestamp: chrono::Utc::now(),
            },
            is_final: true,
            metadata: None,
        });
        let env = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": completed}).to_string();
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(format!("event: status-update\ndata: {env}\n\n"), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let registry = Arc::new(CardRegistry::new());
        registry
            .upsert(RegisteredAgent {
                card: AgentCard {
                    id: "remote".to_string(),
                    name: "Remote".to_string(),
                    version: "1.0".to_string(),
                    description: None,
                    provider: None,
                    documentation_url: None,
                    interfaces: vec![],
                    skills: vec![],
                    security: vec![],
                    extensions: vec![],
                    default_input_modes: vec![],
                    default_output_modes: vec![],
                },
                trust_level: TrustLevel::Trusted,
                base_url: server.uri(),
                last_seen: chrono::Utc::now(),
                health: AgentHealth::Healthy,
                auth_token: None,
            })
            .await;

        let fake = Arc::new(FakeWriter::default());
        let writer: Arc<dyn RawMemoryStore> = fake.clone();
        let router = Arc::new(SmartRouter::new(registry));
        let pool = Arc::new(A2AClientPool::new());
        let sub = A2ASubAgent::new(router, pool).with_raw_memory_writer(writer);

        let outcome = sub
            .execute_delegation(
                "do it",
                Some("Remote"),
                Some("main".to_string()),
                Some("sess-main-1".to_string()),
            )
            .await
            .unwrap();
        assert!(outcome.result.success, "got: {:?}", outcome.result);

        // Fire-and-forget emit: let the spawned task complete.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let captured = fake.0.lock().await;
        assert_eq!(captured.len(), 1, "expected exactly one RawMemory row");
        assert_eq!(
            captured[0].agent_id, "main",
            "delegation row must carry the real parent agent id, not 'default'"
        );
        assert_eq!(captured[0].session_id, Some("sess-main-1".to_string()));
    }
}
