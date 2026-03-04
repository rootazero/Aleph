//! GroupChat Executor — drives the coordinator→persona LLM loop.
//!
//! Given a session and a user message, the executor:
//! 1. Records the user message as a System turn
//! 2. Asks the Coordinator LLM which personas should respond
//! 3. Invokes each selected persona's LLM in order
//! 4. Records each persona response and returns the collected messages

use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::coordinator::{
    build_coordinator_prompt, build_fallback_plan, build_persona_prompt, parse_coordinator_plan,
};
use super::protocol::{GroupChatError, GroupChatMessage, Speaker};
use super::session::GroupChatSession;

/// Executor that drives the coordinator→persona LLM loop for a single round.
///
/// Holds an `Arc<dyn AiProvider>` used for both coordinator and persona calls.
/// The executor is stateless — all session state lives in [`GroupChatSession`].
pub struct GroupChatExecutor {
    provider: Arc<dyn AiProvider>,
}

impl GroupChatExecutor {
    /// Create a new executor with the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Execute a single discussion round.
    ///
    /// # Steps
    ///
    /// 1. Records the user message as a `Speaker::System` turn in the session.
    /// 2. Builds a coordinator prompt from session state and calls the LLM.
    /// 3. Parses the coordinator plan (falls back to all-personas if parsing fails).
    /// 4. For each respondent in the plan, builds a persona prompt and calls the LLM.
    /// 5. Records each persona response as a turn in the session.
    /// 6. Returns the collected `GroupChatMessage` list for this round.
    ///
    /// # Errors
    ///
    /// - [`GroupChatError::ProviderUnavailable`] if the coordinator LLM call fails.
    /// - [`GroupChatError::PersonaNotFound`] if a respondent references an unknown persona.
    /// - [`GroupChatError::PersonaInvocationFailed`] if a persona LLM call fails.
    pub async fn execute_round(
        &self,
        session: &mut GroupChatSession,
        user_message: &str,
    ) -> Result<Vec<GroupChatMessage>, GroupChatError> {
        let round = session.current_round + 1;

        // Step 1: Record user message as a System turn
        session.add_turn(round, Speaker::System, user_message.to_string());

        // Step 2: Build coordinator prompt and call LLM
        let history = session.build_history_text();
        let coordinator_prompt = build_coordinator_prompt(
            &session.participants,
            user_message,
            &history,
            &session.topic,
        );

        let coordinator_raw = self
            .provider
            .process(&coordinator_prompt, None)
            .await
            .map_err(|e| GroupChatError::ProviderUnavailable(e.to_string()))?;

        // Step 3: Parse the coordinator plan, fallback on failure
        let plan = parse_coordinator_plan(&coordinator_raw)
            .unwrap_or_else(|_| build_fallback_plan(&session.participants));

        // Step 4 & 5: Invoke each persona and collect messages
        let mut messages = Vec::new();
        let mut prior_discussion = String::new();
        let total_respondents = plan.respondents.len();

        for (i, respondent) in plan.respondents.iter().enumerate() {
            // Find the persona in the session participants
            let persona = session
                .participants
                .iter()
                .find(|p| p.id == respondent.persona_id)
                .ok_or_else(|| GroupChatError::PersonaNotFound(respondent.persona_id.clone()))?
                .clone();

            // Build persona prompt with cumulative prior discussion
            let persona_prompt = build_persona_prompt(
                &persona,
                user_message,
                &prior_discussion,
                &respondent.guidance,
            );

            // Call persona LLM
            let persona_response = self
                .provider
                .process(&persona_prompt, Some(&persona.system_prompt))
                .await
                .map_err(|e| GroupChatError::PersonaInvocationFailed {
                    persona_id: persona.id.clone(),
                    reason: e.to_string(),
                })?;

            // Record the turn in session history
            let speaker = Speaker::Persona {
                id: persona.id.clone(),
                name: persona.name.clone(),
            };
            session.add_turn(round, speaker.clone(), persona_response.clone());

            // Accumulate prior discussion for the next persona
            prior_discussion.push_str(&format!("[{}]: {}\n\n", persona.name, persona_response));

            // Build output message
            let is_final = i == total_respondents - 1;
            messages.push(GroupChatMessage {
                session_id: session.id.clone(),
                speaker,
                content: persona_response,
                round,
                sequence: i as u32,
                is_final,
            });
        }

        Ok(messages)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::AiProvider;
    use crate::sync_primitives::Arc;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::protocol::Persona;
    use super::super::session::GroupChatSession;

    /// A mock provider that returns different responses based on call order.
    /// The first call is the coordinator response; subsequent calls are persona responses.
    struct SequentialMockProvider {
        responses: Vec<String>,
        call_count: AtomicUsize,
    }

    impl SequentialMockProvider {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    impl AiProvider for SequentialMockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<String>> + Send + '_>> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let response = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| format!("unexpected call #{idx}"));
            Box::pin(async move { Ok(response) })
        }

        fn name(&self) -> &str {
            "sequential-mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    fn test_personas() -> Vec<Persona> {
        vec![
            Persona {
                id: "arch".to_string(),
                name: "Architect".to_string(),
                system_prompt: "You are a software architect.".to_string(),
                provider: None,
                model: None,
                thinking_level: None,
            },
            Persona {
                id: "pm".to_string(),
                name: "Product Manager".to_string(),
                system_prompt: "You are a product manager.".to_string(),
                provider: None,
                model: None,
                thinking_level: None,
            },
        ]
    }

    fn make_session() -> GroupChatSession {
        GroupChatSession::new(
            "test-session-001".to_string(),
            Some("Architecture review".to_string()),
            test_personas(),
            "test".to_string(),
            "test:1".to_string(),
        )
    }

    #[tokio::test]
    async fn test_execute_round_basic() {
        // Coordinator returns a plan selecting both personas in order
        let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":"Focus on architecture"},{"persona_id":"pm","order":1,"guidance":"Focus on user impact"}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
            "Architecture looks solid.".to_string(),
            "Users will love this.".to_string(),
        ]));

        let executor = GroupChatExecutor::new(provider);
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "How should we design auth?")
            .await
            .expect("execute_round should succeed");

        // Should have 2 persona messages
        assert_eq!(messages.len(), 2);

        // First message: Architect
        assert_eq!(messages[0].speaker.name(), "Architect");
        assert_eq!(messages[0].content, "Architecture looks solid.");
        assert_eq!(messages[0].round, 1);
        assert_eq!(messages[0].sequence, 0);
        assert!(!messages[0].is_final);
        assert_eq!(messages[0].session_id, "test-session-001");

        // Second message: Product Manager
        assert_eq!(messages[1].speaker.name(), "Product Manager");
        assert_eq!(messages[1].content, "Users will love this.");
        assert_eq!(messages[1].round, 1);
        assert_eq!(messages[1].sequence, 1);
        assert!(messages[1].is_final);

        // Session state should reflect the round
        assert_eq!(session.current_round, 1);
        // History: 1 system turn + 2 persona turns = 3
        assert_eq!(session.history.len(), 3);
        assert_eq!(session.history[0].speaker, Speaker::System);
        assert_eq!(session.history[0].content, "How should we design auth?");
    }

    #[tokio::test]
    async fn test_execute_round_single_persona() {
        // Coordinator selects only one persona
        let coordinator_response =
            r#"{"respondents":[{"persona_id":"pm","order":0,"guidance":"Be concise"}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
            "Ship it!".to_string(),
        ]));

        let executor = GroupChatExecutor::new(provider);
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "Should we ship?")
            .await
            .expect("execute_round should succeed");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].speaker.name(), "Product Manager");
        assert_eq!(messages[0].content, "Ship it!");
        assert!(messages[0].is_final);
    }

    #[tokio::test]
    async fn test_execute_round_fallback_plan() {
        // Coordinator returns invalid JSON, triggering fallback
        let provider = Arc::new(SequentialMockProvider::new(vec![
            "This is not valid JSON at all".to_string(),
            "Architect response via fallback.".to_string(),
            "PM response via fallback.".to_string(),
        ]));

        let executor = GroupChatExecutor::new(provider);
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "Tell me about caching")
            .await
            .expect("execute_round should succeed with fallback");

        // Fallback includes all personas in config order
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].speaker.name(), "Architect");
        assert_eq!(messages[0].content, "Architect response via fallback.");
        assert_eq!(messages[1].speaker.name(), "Product Manager");
        assert_eq!(messages[1].content, "PM response via fallback.");
        assert!(messages[1].is_final);
    }

    #[tokio::test]
    async fn test_execute_round_coordinator_error() {
        // Provider that always fails
        struct FailingProvider;

        impl AiProvider for FailingProvider {
            fn process(
                &self,
                _input: &str,
                _system_prompt: Option<&str>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<String>> + Send + '_>> {
                Box::pin(async { Err(crate::error::AlephError::network("connection refused")) })
            }
            fn name(&self) -> &str {
                "failing"
            }
            fn color(&self) -> &str {
                "#ff0000"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(FailingProvider);
        let executor = GroupChatExecutor::new(provider);
        let mut session = make_session();

        let result = executor
            .execute_round(&mut session, "Hello?")
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            GroupChatError::ProviderUnavailable(msg) => {
                assert!(msg.contains("connection refused"), "error should mention the cause: {msg}");
            }
            other => panic!("expected ProviderUnavailable, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_execute_round_persona_invocation_failure() {
        // Coordinator succeeds, but persona call fails
        let coordinator_response =
            r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":"go"}],"need_summary":false}"#;

        struct CoordinatorOnlyProvider {
            coordinator_response: String,
            call_count: AtomicUsize,
        }

        impl AiProvider for CoordinatorOnlyProvider {
            fn process(
                &self,
                _input: &str,
                _system_prompt: Option<&str>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<String>> + Send + '_>> {
                let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
                if idx == 0 {
                    let resp = self.coordinator_response.clone();
                    Box::pin(async move { Ok(resp) })
                } else {
                    Box::pin(async { Err(crate::error::AlephError::provider("model overloaded")) })
                }
            }
            fn name(&self) -> &str {
                "coordinator-only"
            }
            fn color(&self) -> &str {
                "#000000"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(CoordinatorOnlyProvider {
            coordinator_response: coordinator_response.to_string(),
            call_count: AtomicUsize::new(0),
        });
        let executor = GroupChatExecutor::new(provider);
        let mut session = make_session();

        let result = executor
            .execute_round(&mut session, "Help me")
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            GroupChatError::PersonaInvocationFailed { persona_id, reason } => {
                assert_eq!(persona_id, "arch");
                assert!(reason.contains("model overloaded"), "reason: {reason}");
            }
            other => panic!("expected PersonaInvocationFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_execute_round_persona_not_found() {
        // Coordinator references a persona not in the session
        let coordinator_response =
            r#"{"respondents":[{"persona_id":"ghost","order":0,"guidance":"boo"}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
        ]));

        let executor = GroupChatExecutor::new(provider);
        let mut session = make_session();

        let result = executor
            .execute_round(&mut session, "Who are you?")
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            GroupChatError::PersonaNotFound(id) => {
                assert_eq!(id, "ghost");
            }
            other => panic!("expected PersonaNotFound, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_execute_round_increments_round_number() {
        let coordinator_response =
            r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":""}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
            "Round 1 response.".to_string(),
            coordinator_response.to_string(),
            "Round 2 response.".to_string(),
        ]));

        let executor = GroupChatExecutor::new(provider);
        let mut session = make_session();

        // Round 1
        let msgs1 = executor.execute_round(&mut session, "First").await.unwrap();
        assert_eq!(msgs1[0].round, 1);
        assert_eq!(session.current_round, 1);

        // Round 2
        let msgs2 = executor.execute_round(&mut session, "Second").await.unwrap();
        assert_eq!(msgs2[0].round, 2);
        assert_eq!(session.current_round, 2);
    }

    #[tokio::test]
    async fn test_prior_discussion_accumulates() {
        // Verify that persona prompts include prior discussion context.
        // We capture the input via a provider that echoes what it receives.
        let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":"go first"},{"persona_id":"pm","order":1,"guidance":"go second"}],"need_summary":false}"#;

        struct EchoAfterCoordinator {
            coordinator_response: String,
            call_count: AtomicUsize,
        }

        impl AiProvider for EchoAfterCoordinator {
            fn process(
                &self,
                input: &str,
                _system_prompt: Option<&str>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<String>> + Send + '_>> {
                let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
                if idx == 0 {
                    let resp = self.coordinator_response.clone();
                    Box::pin(async move { Ok(resp) })
                } else {
                    // Echo a brief summary so we can verify accumulation
                    let has_prior = input.contains("Prior discussion in this round:");
                    let response = format!("call#{idx} prior={has_prior}");
                    Box::pin(async move { Ok(response) })
                }
            }
            fn name(&self) -> &str {
                "echo"
            }
            fn color(&self) -> &str {
                "#000000"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(EchoAfterCoordinator {
            coordinator_response: coordinator_response.to_string(),
            call_count: AtomicUsize::new(0),
        });

        let executor = GroupChatExecutor::new(provider);
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "Discuss caching")
            .await
            .unwrap();

        // First persona should NOT have prior discussion
        assert_eq!(messages[0].content, "call#1 prior=false");
        // Second persona SHOULD have prior discussion (from the first persona)
        assert_eq!(messages[1].content, "call#2 prior=true");
    }
}
