//! A2ASubAgent — SubAgent trait implementation for A2A remote delegation
//!
//! Integrates SmartRouter (routing) + A2AClientPool (communication) to delegate
//! tasks to remote agents via the A2A protocol.

use std::sync::Arc;

use async_trait::async_trait;

use crate::a2a::adapter::client::A2AClientPool;
use crate::a2a::domain::{A2AMessage, A2ARole};
use crate::a2a::service::SmartRouter;
use crate::agents::sub_agents::{
    SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult,
};

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
    cached_names: std::sync::RwLock<Vec<String>>,
}

impl A2ASubAgent {
    pub fn new(smart_router: Arc<SmartRouter>, client_pool: Arc<A2AClientPool>) -> Self {
        Self {
            smart_router,
            client_pool,
            cached_names: std::sync::RwLock::new(Vec::new()),
        }
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
                if name_lower.len() >= 2 {
                    names.push(name_lower);
                }
                for skill in &agent.card.skills {
                    let skill_lower = skill.name.to_lowercase();
                    if skill_lower.len() >= 2 {
                        names.push(skill_lower);
                    }
                    if let Some(ref aliases) = skill.aliases {
                        for alias in aliases {
                            let alias_lower = alias.to_lowercase();
                            if alias_lower.len() >= 2 {
                                names.push(alias_lower);
                            }
                        }
                    }
                }
            }
            tracing::debug!(count = names.len(), "Refreshed A2A agent name cache");
            let mut cache = self
                .cached_names
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *cache = names;
        } else {
            tracing::warn!("Failed to list agents from SmartRouter for name cache");
        }
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
        let names = self
            .cached_names
            .read()
            .unwrap_or_else(|e| e.into_inner());
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
                    format!("Task {} completed with state: {:?}", task.id, task.status.state)
                };

                let result = SubAgentResult::success(request.id.clone(), summary).with_output(
                    serde_json::to_value(&task).unwrap_or_default(),
                );
                Ok(result)
            }
            Err(e) => {
                Ok(SubAgentResult::failure(
                    request.id.clone(),
                    format!("A2A call failed: {}", e),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::*;
    use crate::a2a::port::{A2AResult, AgentHealth, AgentResolver, RegisteredAgent};
    use std::sync::Mutex;

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
            let agents = self.agents.lock().unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());
            Ok(agents.clone())
        }

        async fn resolve_by_id(&self, _agent_id: &str) -> A2AResult<Option<RegisteredAgent>> {
            Ok(None)
        }

        async fn resolve_by_intent(
            &self,
            _intent: &str,
        ) -> A2AResult<Option<RegisteredAgent>> {
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
