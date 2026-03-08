use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::a2a::domain::*;
use crate::a2a::port::*;

/// How the routing decision was made
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMethod {
    ExactName,
    ExactSkill,
    LlmSemantic,
}

/// Result of smart routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub agent: RegisteredAgent,
    pub confidence: f64,
    pub method: RoutingMethod,
    pub reason: Option<String>,
}

/// Callback for LLM-based semantic matching
#[async_trait::async_trait]
pub trait LlmMatcher: Send + Sync {
    /// Given user intent and available agents, return the best match
    async fn match_intent(
        &self,
        intent: &str,
        agents: &[RegisteredAgent],
    ) -> Option<RoutingDecision>;
}

/// Three-tier smart router: exact name -> exact skill -> LLM semantic
pub struct SmartRouter {
    resolver: Arc<dyn AgentResolver>,
    llm_matcher: Option<Arc<dyn LlmMatcher>>,
}

impl SmartRouter {
    pub fn new(resolver: Arc<dyn AgentResolver>) -> Self {
        Self {
            resolver,
            llm_matcher: None,
        }
    }

    pub fn with_llm_matcher(mut self, matcher: Arc<dyn LlmMatcher>) -> Self {
        self.llm_matcher = Some(matcher);
        self
    }

    /// Route user intent to the best matching agent
    pub async fn route(&self, intent: &str) -> A2AResult<Option<RoutingDecision>> {
        let agents = self.resolver.list_agents().await?;
        let healthy_agents: Vec<_> = agents
            .into_iter()
            .filter(|a| a.health != AgentHealth::Unreachable)
            .collect();

        if healthy_agents.is_empty() {
            return Ok(None);
        }

        // Tier 1: Exact name match
        if let Some(decision) = self.try_exact_name(intent, &healthy_agents) {
            return Ok(Some(decision));
        }

        // Tier 2: Exact skill match
        if let Some(decision) = self.try_exact_skill(intent, &healthy_agents) {
            return Ok(Some(decision));
        }

        // Tier 3: LLM semantic match
        if let Some(ref matcher) = self.llm_matcher {
            if let Some(decision) = matcher.match_intent(intent, &healthy_agents).await {
                return Ok(Some(decision));
            }
        }

        Ok(None)
    }

    /// Tier 1: Extract quoted names or direct name references
    fn try_exact_name(&self, intent: &str, agents: &[RegisteredAgent]) -> Option<RoutingDecision> {
        // Try quoted names first: 「xxx」, "xxx"
        let quoted = extract_quoted_name(intent);
        let intent_lower = intent.to_lowercase();

        for agent in agents {
            let name_lower = agent.card.name.to_lowercase();

            // Check quoted match
            if let Some(ref q) = quoted {
                if q.to_lowercase() == name_lower {
                    return Some(RoutingDecision {
                        agent: agent.clone(),
                        confidence: 1.0,
                        method: RoutingMethod::ExactName,
                        reason: Some(format!("Exact name match: {}", agent.card.name)),
                    });
                }
            }

            // Check if agent name appears in intent (require minimum 2 chars to avoid false matches)
            if name_lower.len() >= 2 && intent_lower.contains(&name_lower) {
                return Some(RoutingDecision {
                    agent: agent.clone(),
                    confidence: 0.9,
                    method: RoutingMethod::ExactName,
                    reason: Some(format!("Name found in intent: {}", agent.card.name)),
                });
            }

            // Check skill aliases
            for skill in &agent.card.skills {
                if let Some(ref aliases) = skill.aliases {
                    for alias in aliases {
                        if intent_lower.contains(&alias.to_lowercase()) {
                            return Some(RoutingDecision {
                                agent: agent.clone(),
                                confidence: 0.85,
                                method: RoutingMethod::ExactName,
                                reason: Some(format!("Alias match: {}", alias)),
                            });
                        }
                    }
                }
            }
        }

        None
    }

    /// Tier 2: Match by skill ID or skill name
    fn try_exact_skill(
        &self,
        intent: &str,
        agents: &[RegisteredAgent],
    ) -> Option<RoutingDecision> {
        let intent_lower = intent.to_lowercase();
        for agent in agents {
            for skill in &agent.card.skills {
                if intent_lower.contains(&skill.id.to_lowercase())
                    || intent_lower.contains(&skill.name.to_lowercase())
                {
                    return Some(RoutingDecision {
                        agent: agent.clone(),
                        confidence: 0.8,
                        method: RoutingMethod::ExactSkill,
                        reason: Some(format!("Skill match: {}", skill.name)),
                    });
                }
            }
        }
        None
    }
}

/// Extract a quoted name from intent text.
/// Supports Chinese quotes 「xxx」 and double quotes "xxx".
fn extract_quoted_name(text: &str) -> Option<String> {
    // Try Chinese quotes: 「xxx」
    if let Some(start) = text.find('\u{300C}') {
        // 「
        if let Some(end) = text[start..].find('\u{300D}') {
            // 」
            let inner_start = start + '\u{300C}'.len_utf8();
            let inner_end = start + end;
            if let Some(name) = text.get(inner_start..inner_end) {
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    // Try double quotes: "xxx"
    if let Some(start) = text.find('"') {
        let after_quote = start + 1;
        if let Some(end) = text.get(after_quote..)?.find('"') {
            if let Some(name) = text.get(after_quote..after_quote + end) {
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Mutex;

    // --- Mock AgentResolver ---

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

    #[async_trait::async_trait]
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

        async fn resolve_by_intent(
            &self,
            _intent: &str,
        ) -> A2AResult<Option<RegisteredAgent>> {
            Ok(None)
        }
    }

    // --- Test helpers ---

    fn make_agent(name: &str, skills: Vec<AgentSkill>, health: AgentHealth) -> RegisteredAgent {
        RegisteredAgent {
            card: AgentCard {
                id: format!("{}-id", name.to_lowercase().replace(' ', "-")),
                name: name.to_string(),
                version: "1.0.0".to_string(),
                description: Some(format!("{} agent", name)),
                provider: None,
                documentation_url: None,
                interfaces: vec![],
                skills,
                security: vec![],
                extensions: vec![],
                default_input_modes: vec![],
                default_output_modes: vec![],
            },
            trust_level: TrustLevel::Trusted,
            base_url: format!("http://localhost:8080/{}", name.to_lowercase()),
            last_seen: Utc::now(),
            health,
        }
    }

    fn make_skill(id: &str, name: &str, aliases: Option<Vec<String>>) -> AgentSkill {
        AgentSkill {
            id: id.to_string(),
            name: name.to_string(),
            description: Some(format!("{} skill", name)),
            aliases,
            examples: None,
            input_types: None,
            output_types: None,
        }
    }

    // --- extract_quoted_name tests ---

    #[test]
    fn extract_quoted_name_chinese_quotes() {
        let result = extract_quoted_name("请使用「交易助手」来处理");
        assert_eq!(result, Some("交易助手".to_string()));
    }

    #[test]
    fn extract_quoted_name_double_quotes() {
        let result = extract_quoted_name("use \"trading assistant\" for this");
        assert_eq!(result, Some("trading assistant".to_string()));
    }

    #[test]
    fn extract_quoted_name_no_quotes() {
        let result = extract_quoted_name("just a plain intent without quotes");
        assert_eq!(result, None);
    }

    #[test]
    fn extract_quoted_name_empty_quotes() {
        assert_eq!(extract_quoted_name("empty \"\" quotes"), None);
        assert_eq!(extract_quoted_name("空「」引号"), None);
    }

    // --- try_exact_name tests ---

    #[test]
    fn try_exact_name_quoted_match() {
        let agent = make_agent("Trading Assistant", vec![], AgentHealth::Healthy);
        let router = SmartRouter::new(Arc::new(MockResolver::new(vec![])));
        let result =
            router.try_exact_name("请使用「Trading Assistant」", &[agent]);
        assert!(result.is_some());
        let decision = result.unwrap();
        assert_eq!(decision.confidence, 1.0);
        assert_eq!(decision.method, RoutingMethod::ExactName);
    }

    #[test]
    fn try_exact_name_in_text() {
        let agent = make_agent("CodeBot", vec![], AgentHealth::Healthy);
        let router = SmartRouter::new(Arc::new(MockResolver::new(vec![])));
        let result = router.try_exact_name("ask codebot to review", &[agent]);
        assert!(result.is_some());
        let decision = result.unwrap();
        assert_eq!(decision.confidence, 0.9);
        assert_eq!(decision.method, RoutingMethod::ExactName);
    }

    #[test]
    fn try_exact_name_alias_match() {
        let skill = make_skill("s1", "Skill", Some(vec!["reviewer".to_string()]));
        let agent = make_agent("Agent", vec![skill], AgentHealth::Healthy);
        let router = SmartRouter::new(Arc::new(MockResolver::new(vec![])));
        let result = router.try_exact_name("ask the reviewer to help", &[agent]);
        assert!(result.is_some());
        let decision = result.unwrap();
        assert_eq!(decision.confidence, 0.85);
    }

    // --- route() integration tests ---

    #[tokio::test]
    async fn route_exact_name_match() {
        let agent = make_agent("Aleph", vec![], AgentHealth::Healthy);
        let resolver = Arc::new(MockResolver::new(vec![agent]));
        let router = SmartRouter::new(resolver);

        let result = router.route("ask \"Aleph\" to help").await.unwrap();
        assert!(result.is_some());
        let decision = result.unwrap();
        assert_eq!(decision.confidence, 1.0);
        assert_eq!(decision.method, RoutingMethod::ExactName);
    }

    #[tokio::test]
    async fn route_no_agents_returns_none() {
        let resolver = Arc::new(MockResolver::new(vec![]));
        let router = SmartRouter::new(resolver);

        let result = router.route("do something").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn route_filters_unreachable_agents() {
        let agent = make_agent("Helper", vec![], AgentHealth::Unreachable);
        let resolver = Arc::new(MockResolver::new(vec![agent]));
        let router = SmartRouter::new(resolver);

        let result = router.route("ask helper").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn route_skill_match() {
        let skill = make_skill("code-review", "Code Review", None);
        let agent = make_agent("Dev Agent", vec![skill], AgentHealth::Healthy);
        let resolver = Arc::new(MockResolver::new(vec![agent]));
        let router = SmartRouter::new(resolver);

        // "code review" won't match the agent name "Dev Agent", so it falls to tier 2
        let result = router.route("do a code review").await.unwrap();
        assert!(result.is_some());
        let decision = result.unwrap();
        assert_eq!(decision.confidence, 0.8);
        assert_eq!(decision.method, RoutingMethod::ExactSkill);
    }

    #[tokio::test]
    async fn route_degraded_agent_still_routable() {
        let agent = make_agent("SlowBot", vec![], AgentHealth::Degraded);
        let resolver = Arc::new(MockResolver::new(vec![agent]));
        let router = SmartRouter::new(resolver);

        let result = router.route("ask slowbot").await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn route_no_match_returns_none() {
        let agent = make_agent("SpecificBot", vec![], AgentHealth::Healthy);
        let resolver = Arc::new(MockResolver::new(vec![agent]));
        let router = SmartRouter::new(resolver);

        let result = router
            .route("something completely unrelated xyz")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn route_llm_fallback() {
        let agent = make_agent("SmartAgent", vec![], AgentHealth::Healthy);
        let resolver = Arc::new(MockResolver::new(vec![agent.clone()]));

        struct FakeLlm;
        #[async_trait::async_trait]
        impl LlmMatcher for FakeLlm {
            async fn match_intent(
                &self,
                _intent: &str,
                agents: &[RegisteredAgent],
            ) -> Option<RoutingDecision> {
                agents.first().map(|a| RoutingDecision {
                    agent: a.clone(),
                    confidence: 0.7,
                    method: RoutingMethod::LlmSemantic,
                    reason: Some("LLM matched".to_string()),
                })
            }
        }

        let router = SmartRouter::new(resolver).with_llm_matcher(Arc::new(FakeLlm));

        let result = router
            .route("something unrelated to name or skill")
            .await
            .unwrap();
        assert!(result.is_some());
        let decision = result.unwrap();
        assert_eq!(decision.confidence, 0.7);
        assert_eq!(decision.method, RoutingMethod::LlmSemantic);
    }
}
