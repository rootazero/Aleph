use std::sync::Arc;

use async_trait::async_trait;

use crate::a2a::port::RegisteredAgent;
use crate::providers::AiProvider;

use super::smart_router::{LlmMatcher, RoutingDecision, RoutingMethod};

/// LLM-based semantic matcher for A2A agent routing.
///
/// When exact name/skill matching fails, uses an LLM to understand
/// user intent and match it to the best available agent.
pub struct SemanticLlmMatcher {
    provider: Arc<dyn AiProvider>,
}

impl SemanticLlmMatcher {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    fn build_system_prompt(&self) -> String {
        "You are an agent routing expert. Given a user request and a list of available agents, \
         determine which agent is the best match.\n\n\
         Respond in exactly this format:\n\
         AGENT_INDEX: <number>\n\
         CONFIDENCE: <0.0-1.0>\n\
         REASON: <brief explanation>\n\n\
         If no agent is a good match, respond with:\n\
         AGENT_INDEX: -1\n\
         CONFIDENCE: 0.0\n\
         REASON: No suitable agent found"
            .to_string()
    }

    fn build_routing_prompt(&self, intent: &str, agents: &[RegisteredAgent]) -> String {
        let mut prompt = format!("User request: \"{}\"\n\nAvailable agents:\n", intent);

        for (i, agent) in agents.iter().enumerate() {
            prompt.push_str(&format!("\n{}. {} ({})\n", i, agent.card.name, agent.card.id));

            if let Some(ref desc) = agent.card.description {
                prompt.push_str(&format!("   Description: {}\n", desc));
            }
            if !agent.card.skills.is_empty() {
                prompt.push_str("   Skills:\n");
                for skill in &agent.card.skills {
                    prompt.push_str(&format!("   - {}", skill.name));
                    if let Some(ref d) = skill.description {
                        prompt.push_str(&format!(": {}", d));
                    }
                    prompt.push('\n');
                }
            }
        }

        prompt.push_str("\nWhich agent best handles this request?");
        prompt
    }

    fn parse_response(
        &self,
        response: &str,
        agents: &[RegisteredAgent],
    ) -> Option<RoutingDecision> {
        let mut index: Option<i32> = None;
        let mut confidence: Option<f64> = None;
        let mut reason: Option<String> = None;

        for line in response.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("AGENT_INDEX:") {
                index = val.trim().parse().ok();
            } else if let Some(val) = line.strip_prefix("CONFIDENCE:") {
                confidence = val.trim().parse().ok();
            } else if let Some(val) = line.strip_prefix("REASON:") {
                reason = Some(val.trim().to_string());
            }
        }

        let idx = index?;
        if idx < 0 || idx as usize >= agents.len() {
            return None;
        }

        let conf = confidence.unwrap_or(0.5).clamp(0.0, 1.0);

        // Only return if confidence is above threshold
        if conf < 0.3 {
            return None;
        }

        Some(RoutingDecision {
            agent: agents[idx as usize].clone(),
            confidence: conf,
            method: RoutingMethod::LlmSemantic,
            reason,
        })
    }
}

#[async_trait]
impl LlmMatcher for SemanticLlmMatcher {
    async fn match_intent(
        &self,
        intent: &str,
        agents: &[RegisteredAgent],
    ) -> Option<RoutingDecision> {
        if agents.is_empty() {
            return None;
        }

        let system = self.build_system_prompt();
        let prompt = self.build_routing_prompt(intent, agents);

        match self.provider.process(&prompt, Some(&system)).await {
            Ok(response) => self.parse_response(&response, agents),
            Err(e) => {
                tracing::warn!(error = %e, "LLM semantic matching failed, skipping tier 3");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::*;
    use crate::a2a::port::AgentHealth;
    use crate::error::AlephError;
    use chrono::Utc;
    use std::future::Future;
    use std::pin::Pin;

    // --- Mock AiProvider ---

    struct MockProvider {
        response: Result<String, String>,
    }

    impl MockProvider {
        fn with_response(response: &str) -> Self {
            Self {
                response: Ok(response.to_string()),
            }
        }

        fn with_error(error: &str) -> Self {
            Self {
                response: Err(error.to_string()),
            }
        }
    }

    impl AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<String>> + Send + '_>> {
            let result = match &self.response {
                Ok(s) => Ok(s.clone()),
                Err(e) => Err(AlephError::ProviderError {
                    message: e.clone(),
                    suggestion: None,
                }),
            };
            Box::pin(async move { result })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    // --- Test helpers ---

    fn make_agent(name: &str, description: Option<&str>, skills: Vec<AgentSkill>) -> RegisteredAgent {
        RegisteredAgent {
            card: AgentCard {
                id: format!("{}-id", name.to_lowercase().replace(' ', "-")),
                name: name.to_string(),
                version: "1.0.0".to_string(),
                description: description.map(|s| s.to_string()),
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
            health: AgentHealth::Healthy,
        }
    }

    fn make_skill(id: &str, name: &str, description: Option<&str>) -> AgentSkill {
        AgentSkill {
            id: id.to_string(),
            name: name.to_string(),
            description: description.map(|s| s.to_string()),
            aliases: None,
            examples: None,
            input_types: None,
            output_types: None,
        }
    }

    // --- build_routing_prompt tests ---

    #[test]
    fn build_routing_prompt_includes_agent_info() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let skill = make_skill("translate", "Translation", Some("Translate text between languages"));
        let agents = vec![
            make_agent("CodeBot", Some("A coding assistant"), vec![]),
            make_agent("TranslateBot", Some("A translation expert"), vec![skill]),
        ];

        let prompt = matcher.build_routing_prompt("translate this to French", &agents);

        assert!(prompt.contains("translate this to French"));
        assert!(prompt.contains("0. CodeBot"));
        assert!(prompt.contains("1. TranslateBot"));
        assert!(prompt.contains("A coding assistant"));
        assert!(prompt.contains("A translation expert"));
        assert!(prompt.contains("Translation: Translate text between languages"));
    }

    #[test]
    fn build_routing_prompt_handles_no_description() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("SimpleBot", None, vec![])];
        let prompt = matcher.build_routing_prompt("hello", &agents);

        assert!(prompt.contains("0. SimpleBot"));
        assert!(!prompt.contains("Description:"));
    }

    // --- parse_response tests ---

    #[test]
    fn parse_response_valid() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![
            make_agent("Agent0", None, vec![]),
            make_agent("Agent1", None, vec![]),
        ];

        let response = "AGENT_INDEX: 1\nCONFIDENCE: 0.85\nREASON: Best match for the task";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_some());
        let decision = result.unwrap();
        assert_eq!(decision.agent.card.name, "Agent1");
        assert!((decision.confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(decision.method, RoutingMethod::LlmSemantic);
        assert_eq!(decision.reason, Some("Best match for the task".to_string()));
    }

    #[test]
    fn parse_response_negative_index() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let response = "AGENT_INDEX: -1\nCONFIDENCE: 0.0\nREASON: No suitable agent found";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_none());
    }

    #[test]
    fn parse_response_index_out_of_bounds() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let response = "AGENT_INDEX: 5\nCONFIDENCE: 0.9\nREASON: Out of range";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_none());
    }

    #[test]
    fn parse_response_below_confidence_threshold() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let response = "AGENT_INDEX: 0\nCONFIDENCE: 0.1\nREASON: Very weak match";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_none());
    }

    #[test]
    fn parse_response_missing_confidence_defaults_to_half() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let response = "AGENT_INDEX: 0\nREASON: Some reason";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_some());
        let decision = result.unwrap();
        assert!((decision.confidence - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_response_missing_index_returns_none() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let response = "CONFIDENCE: 0.9\nREASON: No index provided";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_none());
    }

    #[test]
    fn parse_response_clamps_confidence() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let response = "AGENT_INDEX: 0\nCONFIDENCE: 1.5\nREASON: Over-confident";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_some());
        assert!((result.unwrap().confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_response_tolerates_extra_whitespace() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let response = "  AGENT_INDEX:   0  \n  CONFIDENCE:  0.7  \n  REASON:  Matched well  ";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_some());
        let decision = result.unwrap();
        assert!((decision.confidence - 0.7).abs() < f64::EPSILON);
        assert_eq!(decision.reason, Some("Matched well".to_string()));
    }

    #[test]
    fn parse_response_garbage_returns_none() {
        let provider = Arc::new(MockProvider::with_response(""));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let response = "I think Agent0 is the best match because it handles coding tasks well.";
        let result = matcher.parse_response(response, &agents);

        assert!(result.is_none());
    }

    // --- match_intent tests ---

    #[tokio::test]
    async fn match_intent_empty_agents() {
        let provider = Arc::new(MockProvider::with_response("should not be called"));
        let matcher = SemanticLlmMatcher::new(provider);

        let result = matcher.match_intent("do something", &[]).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn match_intent_successful_match() {
        let provider = Arc::new(MockProvider::with_response(
            "AGENT_INDEX: 0\nCONFIDENCE: 0.9\nREASON: Best match for translation",
        ));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent(
            "TranslateBot",
            Some("Translates text"),
            vec![make_skill("translate", "Translation", None)],
        )];

        let result = matcher.match_intent("translate hello to French", &agents).await;
        assert!(result.is_some());
        let decision = result.unwrap();
        assert_eq!(decision.agent.card.name, "TranslateBot");
        assert!((decision.confidence - 0.9).abs() < f64::EPSILON);
        assert_eq!(decision.method, RoutingMethod::LlmSemantic);
    }

    #[tokio::test]
    async fn match_intent_provider_error_returns_none() {
        let provider = Arc::new(MockProvider::with_error("API rate limited"));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let result = matcher.match_intent("do something", &agents).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn match_intent_no_match_from_llm() {
        let provider = Arc::new(MockProvider::with_response(
            "AGENT_INDEX: -1\nCONFIDENCE: 0.0\nREASON: No suitable agent found",
        ));
        let matcher = SemanticLlmMatcher::new(provider);

        let agents = vec![make_agent("Agent0", None, vec![])];
        let result = matcher.match_intent("something completely unrelated", &agents).await;
        assert!(result.is_none());
    }
}
