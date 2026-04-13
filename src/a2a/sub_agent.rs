//! A2ASubAgent — SubAgent trait implementation for A2A remote delegation
//!
//! Integrates SmartRouter (routing) + A2AClientPool (communication) to delegate
//! tasks to remote agents via the A2A protocol.

use async_trait::async_trait;

use crate::a2a::adapter::client::A2AClientPool;
use crate::a2a::domain::{A2AMessage, A2ARole};
use crate::a2a::service::SmartRouter;
use crate::agents::sub_agents::{SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult};
use crate::memory::extensions::{insert_with_capture_filter, MemoryExtensionRegistry};
use crate::memory::extensions::types::CaptureCtx;
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::sync_primitives::Arc;

/// SubAgent implementation that delegates tasks to remote A2A agents.
///
/// Uses SmartRouter for intent-based agent discovery and A2AClientPool
/// for managing HTTP connections to remote agents.
///
/// The `cached_names` field holds a lowercased list of agent names, skill names,
/// and skill aliases from the registry. This enables `can_handle` (which is sync)
/// to match user prompts that mention registered agent names without needing
/// an async resolver call. Call `refresh_agent_names()` after agent registration
/// changes to keep the cache current.
pub struct A2ASubAgent {
    smart_router: Arc<SmartRouter>,
    client_pool: Arc<A2AClientPool>,
    /// Cached lowercased agent/skill names for sync can_handle matching
    cached_names: crate::sync_primitives::RwLock<Vec<String>>,
    /// Optional writer for raw memory delegation hooks (Spec 1 G2).
    /// When set, `execute` will write a `RawMemory(Delegation{child_agent_id})`
    /// row before returning a successful result.
    raw_memory_writer: Option<std::sync::Arc<dyn RawMemoryStore>>,
    /// Optional capture-filter registry (Spec 4 Task 6).
    /// When set, delegation raw-memory writes go through `insert_with_capture_filter`.
    /// Task 11 wires the real registry at startup; `None` falls back to direct insert.
    capture_registry: Option<std::sync::Arc<MemoryExtensionRegistry>>,
}

impl A2ASubAgent {
    pub fn new(smart_router: Arc<SmartRouter>, client_pool: Arc<A2AClientPool>) -> Self {
        Self {
            smart_router,
            client_pool,
            cached_names: crate::sync_primitives::RwLock::new(Vec::new()),
            raw_memory_writer: None,
            capture_registry: None,
        }
    }

    /// Attach an optional raw-memory writer for delegation hooks (Spec 1 G2).
    ///
    /// When set, `execute` will write a `RawMemory(Delegation{child_agent_id})`
    /// row carrying the delegation prompt + sub-agent summary before returning,
    /// allowing CompressionService to distil durable lessons for the parent agent.
    pub fn with_raw_memory_writer(
        mut self,
        writer: std::sync::Arc<dyn RawMemoryStore>,
    ) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// Attach a capture-filter registry (Spec 4 Task 6).
    ///
    /// When set, delegation raw-memory writes go through `insert_with_capture_filter`.
    /// Task 11 wires the real registry at startup; `None` falls back to direct insert.
    pub fn with_capture_registry(
        mut self,
        registry: std::sync::Arc<MemoryExtensionRegistry>,
    ) -> Self {
        self.capture_registry = Some(registry);
        self
    }

    /// Refresh the cached agent names from the resolver.
    ///
    /// Call this after registering or unregistering agents in the CardRegistry
    /// so that `can_handle` can match natural language prompts against current
    /// agent names, skill names, and aliases.
    pub async fn refresh_agent_names(&self) {
        if let Ok(agents) = self.smart_router.list_agents().await {
            let mut names = Vec::new();
            for agent in &agents {
                let name_lower = agent.card.name.to_lowercase();
                // Only cache names with >= 2 chars to avoid false positives
                if name_lower.chars().count() >= 2 {
                    names.push(name_lower);
                }
                for skill in &agent.card.skills {
                    let skill_lower = skill.name.to_lowercase();
                    if skill_lower.chars().count() >= 2 {
                        names.push(skill_lower);
                    }
                    if let Some(ref aliases) = skill.aliases {
                        for alias in aliases {
                            let alias_lower = alias.to_lowercase();
                            if alias_lower.chars().count() >= 2 {
                                names.push(alias_lower);
                            }
                        }
                    }
                }
            }
            tracing::debug!(count = names.len(), "Refreshed A2A agent name cache");
            let mut cache = self.cached_names.write().unwrap_or_else(|e| e.into_inner());
            *cache = names;
        } else {
            tracing::warn!("Failed to list agents from SmartRouter for name cache");
        }
    }
}

/// Emit a `RawMemory(Delegation{child_agent_id})` row in a fire-and-forget spawn.
///
/// Carries the delegation prompt and sub-agent summary so CompressionService
/// can distil durable lessons for the parent agent's long-term memory.
/// The parent agent_id is taken from `execution_context.metadata["parent_agent_id"]`
/// (falls back to `"default"` when absent — Task 10 will wire the real value at startup).
pub(crate) fn emit_delegation_raw(
    writer: std::sync::Arc<dyn RawMemoryStore>,
    request: &SubAgentRequest,
    result: &SubAgentResult,
    child_agent_id: impl Into<String>,
) {
    emit_delegation_raw_with_registry(writer, request, result, child_agent_id, None);
}

/// Inner implementation that optionally threads an extension registry.
pub(crate) fn emit_delegation_raw_with_registry(
    writer: std::sync::Arc<dyn RawMemoryStore>,
    request: &SubAgentRequest,
    result: &SubAgentResult,
    child_agent_id: impl Into<String>,
    registry: Option<std::sync::Arc<MemoryExtensionRegistry>>,
) {
    let child_agent_id = child_agent_id.into();
    let prompt_text = request.prompt.clone();
    let summary_text = result.summary.clone();
    let content = format!(
        "DELEGATION_PROMPT:\n{prompt}\n\nDELEGATION_RESULT:\n{summary}",
        prompt = prompt_text,
        summary = summary_text,
    );

    let parent_agent_id = request
        .execution_context
        .as_ref()
        .and_then(|ctx| ctx.metadata.get("parent_agent_id").cloned())
        .unwrap_or_else(|| "default".to_string());

    let parent_session_id = request.parent_session_id.clone();

    let mut raw = RawMemory::new(
        content,
        RawMemorySource::Delegation { child_agent_id },
    )
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

#[async_trait]
impl SubAgent for A2ASubAgent {
    fn id(&self) -> &str {
        "a2a"
    }

    fn name(&self) -> &str {
        "A2A Remote Agent"
    }

    fn description(&self) -> &str {
        "Delegates tasks to remote agents via A2A protocol"
    }

    fn capabilities(&self) -> Vec<SubAgentCapability> {
        vec![SubAgentCapability::Custom]
    }

    fn can_handle(&self, request: &SubAgentRequest) -> bool {
        // Priority 1: Explicit target
        if request.target.as_deref() == Some("a2a") {
            return true;
        }

        // Priority 2: Check if prompt mentions any cached agent/skill name
        let names = self.cached_names.read().unwrap_or_else(|e| e.into_inner());
        if names.is_empty() {
            return false;
        }

        let prompt_lower = request.prompt.to_lowercase();
        names.iter().any(|name| prompt_lower.contains(name))
    }

    async fn execute(&self, request: SubAgentRequest) -> crate::error::Result<SubAgentResult> {
        tracing::info!(
            request_id = %request.id,
            prompt = %request.prompt.chars().take(100).collect::<String>(),
            "Executing A2A delegation"
        );

        // 1. Route to best matching agent
        let decision = self
            .smart_router
            .route(&request.prompt)
            .await
            .map_err(|e| crate::error::AlephError::other(format!("A2A routing failed: {}", e)))?;

        let decision = match decision {
            Some(d) => d,
            None => {
                return Ok(SubAgentResult::failure(
                    request.id.clone(),
                    "No matching A2A agent found for this request".to_string(),
                ));
            }
        };

        tracing::info!(
            agent = %decision.agent.card.name,
            confidence = %decision.confidence,
            method = ?decision.method,
            "Routed to remote agent"
        );

        // 2. Get or create HTTP client for the target agent
        let client = self
            .client_pool
            .get_or_create(&decision.agent)
            .await
            .map_err(|e| {
                crate::error::AlephError::other(format!("A2A client creation failed: {}", e))
            })?;

        // 3. Build A2A message from the request prompt
        let message = A2AMessage::text(A2ARole::User, &request.prompt);

        // 4. Send message and wait for result
        let task_id = uuid::Uuid::new_v4().to_string();
        let task_result = client.send_message(&task_id, &message, None).await;

        match task_result {
            Ok(task) => {
                let summary = if !task.history.is_empty() {
                    task.history
                        .iter()
                        .rev()
                        .find(|m| m.role == A2ARole::Agent)
                        .map(|m| m.text_content())
                        .unwrap_or_else(|| format!("Task {} completed", task.id))
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
                let result =
                    SubAgentResult::success(request.id.clone(), summary).with_output(output);

                // Spec 1 G2: record delegation outcome for parent-agent memory.
                if let Some(w) = self.raw_memory_writer.clone() {
                    emit_delegation_raw_with_registry(
                        w,
                        &request,
                        &result,
                        &decision.agent.card.id,
                        self.capture_registry.clone(),
                    );
                }

                Ok(result)
            }
            Err(e) => Ok(SubAgentResult::failure(
                request.id.clone(),
                format!("A2A call failed: {}", e),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::*;
    use crate::a2a::port::{A2AResult, AgentHealth, AgentResolver, RegisteredAgent};
    use crate::sync_primitives::Mutex;

    // --- Mock AgentResolver for SmartRouter ---

    struct MockResolver {
        agents: Mutex<Vec<RegisteredAgent>>,
    }

    impl MockResolver {
        fn new(agents: Vec<RegisteredAgent>) -> Self {
            Self {
                agents: Mutex::new(agents),
            }
        }
    }

    #[async_trait]
    impl AgentResolver for MockResolver {
        async fn fetch_card(&self, _url: &str) -> A2AResult<AgentCard> {
            Err(A2AError::InternalError("not implemented".into()))
        }

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
            let agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
            Ok(agents.clone())
        }

        async fn resolve_by_id(&self, _agent_id: &str) -> A2AResult<Option<RegisteredAgent>> {
            Ok(None)
        }

        async fn resolve_by_intent(&self, _intent: &str) -> A2AResult<Option<RegisteredAgent>> {
            Ok(None)
        }
    }

    fn build_sub_agent(agents: Vec<RegisteredAgent>) -> A2ASubAgent {
        let resolver = Arc::new(MockResolver::new(agents));
        let router = Arc::new(SmartRouter::new(resolver));
        let pool = Arc::new(A2AClientPool::new());
        A2ASubAgent::new(router, pool)
    }

    #[test]
    fn id_returns_a2a() {
        let agent = build_sub_agent(vec![]);
        assert_eq!(agent.id(), "a2a");
    }

    #[test]
    fn name_returns_correct_value() {
        let agent = build_sub_agent(vec![]);
        assert_eq!(agent.name(), "A2A Remote Agent");
    }

    #[test]
    fn description_is_nonempty() {
        let agent = build_sub_agent(vec![]);
        assert!(!agent.description().is_empty());
    }

    #[test]
    fn capabilities_includes_custom() {
        let agent = build_sub_agent(vec![]);
        let caps = agent.capabilities();
        assert!(caps.contains(&SubAgentCapability::Custom));
    }

    #[test]
    fn can_handle_with_a2a_target() {
        let agent = build_sub_agent(vec![]);
        let request = SubAgentRequest::new("Do something").with_target("a2a");
        assert!(agent.can_handle(&request));
    }

    #[test]
    fn can_handle_without_target_returns_false_when_no_cache() {
        let agent = build_sub_agent(vec![]);
        let request = SubAgentRequest::new("Do something");
        assert!(!agent.can_handle(&request));
    }

    #[test]
    fn can_handle_with_other_target_returns_false() {
        let agent = build_sub_agent(vec![]);
        let request = SubAgentRequest::new("Do something").with_target("mcp");
        assert!(!agent.can_handle(&request));
    }

    #[tokio::test]
    async fn can_handle_matches_agent_name_in_prompt() {
        let agents = vec![RegisteredAgent {
            card: AgentCard {
                id: "trading-id".to_string(),
                name: "交易助手".to_string(),
                version: "1.0.0".to_string(),
                description: Some("Trading agent".to_string()),
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
            base_url: "http://localhost:8080/trading".to_string(),
            last_seen: chrono::Utc::now(),
            health: AgentHealth::Healthy,
            auth_token: None,
        }];
        let sub = build_sub_agent(agents);
        sub.refresh_agent_names().await;

        // Should match — prompt contains agent name
        let request = SubAgentRequest::new("请使用交易助手agent分析黄金走势");
        assert!(sub.can_handle(&request));

        // Should not match — unrelated prompt
        let request = SubAgentRequest::new("今天天气怎么样");
        assert!(!sub.can_handle(&request));
    }

    #[tokio::test]
    async fn can_handle_matches_skill_name() {
        let agents = vec![RegisteredAgent {
            card: AgentCard {
                id: "dev-id".to_string(),
                name: "DevBot".to_string(),
                version: "1.0.0".to_string(),
                description: None,
                provider: None,
                documentation_url: None,
                interfaces: vec![],
                skills: vec![AgentSkill {
                    id: "code-review".to_string(),
                    name: "Code Review".to_string(),
                    description: None,
                    aliases: Some(vec!["审查代码".to_string()]),
                    examples: None,
                    input_types: None,
                    output_types: None,
                }],
                security: vec![],
                extensions: vec![],
                default_input_modes: vec![],
                default_output_modes: vec![],
            },
            trust_level: TrustLevel::Trusted,
            base_url: "http://localhost:8080/dev".to_string(),
            last_seen: chrono::Utc::now(),
            health: AgentHealth::Healthy,
            auth_token: None,
        }];
        let sub = build_sub_agent(agents);
        sub.refresh_agent_names().await;

        // Match by skill name
        let request = SubAgentRequest::new("please do a code review on this PR");
        assert!(sub.can_handle(&request));

        // Match by alias
        let request = SubAgentRequest::new("帮我审查代码");
        assert!(sub.can_handle(&request));
    }

    #[tokio::test]
    async fn can_handle_case_insensitive() {
        let agents = vec![RegisteredAgent {
            card: AgentCard {
                id: "bot-id".to_string(),
                name: "CodeBot".to_string(),
                version: "1.0.0".to_string(),
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
            base_url: "http://localhost:8080/codebot".to_string(),
            last_seen: chrono::Utc::now(),
            health: AgentHealth::Healthy,
            auth_token: None,
        }];
        let sub = build_sub_agent(agents);
        sub.refresh_agent_names().await;

        let request = SubAgentRequest::new("ask CODEBOT to help");
        assert!(sub.can_handle(&request));
    }

    #[test]
    fn can_handle_empty_cache_returns_false() {
        let agent = build_sub_agent(vec![]);
        // No refresh_agent_names called, cache is empty
        let request = SubAgentRequest::new("请使用交易助手分析黄金");
        assert!(!agent.can_handle(&request));
    }

    #[tokio::test]
    async fn execute_no_agents_returns_failure() {
        let agent = build_sub_agent(vec![]);
        let request = SubAgentRequest::new("Do something").with_target("a2a");
        let result = agent.execute(request).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("No matching A2A agent"));
    }
}

#[cfg(test)]
mod spec1_tests {
    use super::*;
    use crate::agents::sub_agents::SubAgentRequest;
    use crate::error::AlephError;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use std::sync::Arc;

    #[derive(Default)]
    struct FakeWriter(std::sync::Mutex<Vec<RawMemory>>);

    #[async_trait::async_trait]
    impl RawMemoryStore for FakeWriter {
        async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
            self.0.lock().unwrap().push(raw.clone());
            Ok(())
        }

        async fn get_unprocessed_raw_memories(
            &self,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(vec![])
        }

        async fn mark_raw_as_processed(
            &self,
            _ids: &[String],
        ) -> Result<usize, AlephError> {
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

        let captured = fake.0.lock().unwrap();
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
    async fn emit_delegation_raw_uses_metadata_parent_agent_id() {
        let fake = Arc::new(FakeWriter::default());
        let writer: Arc<dyn RawMemoryStore> = fake.clone();

        use crate::agents::sub_agents::ExecutionContextInfo;
        let ctx = ExecutionContextInfo::new()
            .with_metadata("parent_agent_id", "parent-agent-007");
        let request = SubAgentRequest::new("Do the thing")
            .with_parent_session("sess-99")
            .with_execution_context(ctx);
        let result = crate::agents::sub_agents::SubAgentResult::success(
            request.id.clone(),
            "Done",
        );

        emit_delegation_raw(writer, &request, &result, "child-007");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let captured = fake.0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].agent_id, "parent-agent-007");
    }
}
