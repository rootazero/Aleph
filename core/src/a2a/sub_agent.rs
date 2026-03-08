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
pub struct A2ASubAgent {
    smart_router: Arc<SmartRouter>,
    client_pool: Arc<A2AClientPool>,
}

impl A2ASubAgent {
    pub fn new(smart_router: Arc<SmartRouter>, client_pool: Arc<A2AClientPool>) -> Self {
        Self {
            smart_router,
            client_pool,
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
        // Only handle requests explicitly targeted at "a2a"
        // can_handle is sync, so we cannot do async SmartRouter lookup here
        request.target.as_deref() == Some("a2a")
    }

    async fn execute(&self, request: SubAgentRequest) -> crate::error::Result<SubAgentResult> {
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
                // Extract summary from the task response
                let summary = if !task.history.is_empty() {
                    // Prefer the last agent message from history
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
    use crate::a2a::port::{A2AResult, AgentResolver, RegisteredAgent};
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
    fn can_handle_without_target_returns_false() {
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
