//! `GroupChat` Executor — drives the coordinator->persona LLM loop.
//!
//! Given a session and a user message, the executor:
//! 1. Records the user message as a System turn
//! 2. Asks the Coordinator LLM which personas should respond
//! 3. Invokes each selected persona's LLM in order
//! 4. Records each persona response and returns the collected messages

use std::fmt::Write as _;

use crate::agents::thinking::ThinkLevel;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::providers::DefaultProviderHandle;
use crate::providers::ProviderRegistry;
use crate::resilience::database::StateDatabase;
use crate::sync_primitives::Arc;

use super::coordinator::{
    build_coordinator_prompt, build_fallback_plan, build_persona_prompt, parse_coordinator_plan,
};
use super::protocol::{GroupChatError, GroupChatMessage, GroupChatStatus, Persona, Speaker};
use super::session::GroupChatSession;

/// Executor that drives the coordinator->persona LLM loop for a single round.
///
/// Holds an `Arc<dyn DefaultProviderHandle>` (Step 5 hot-reload) so the
/// coordinator's default LLM tracks UI-driven default-provider swaps without
/// a server restart. An optional [`ProviderRegistry`] resolves per-persona
/// provider overrides.
pub struct GroupChatExecutor {
    default_provider: Arc<dyn DefaultProviderHandle>,
    provider_registry: Option<Arc<ProviderRegistry>>,
    coordinator_visible: bool,
    db: Option<Arc<StateDatabase>>,
}

impl GroupChatExecutor {
    /// Create a new executor with the given default-provider handle.
    pub fn new(default_provider: Arc<dyn DefaultProviderHandle>) -> Self {
        Self {
            default_provider,
            provider_registry: None,
            coordinator_visible: false,
            db: None,
        }
    }

    /// Set a provider registry for per-persona provider resolution.
    ///
    /// When a persona specifies a `provider` field, the executor looks it up
    /// in this registry. Falls back to the default provider if not found.
    #[must_use]
    pub fn with_provider_registry(mut self, registry: Arc<ProviderRegistry>) -> Self {
        self.provider_registry = Some(registry);
        self
    }

    /// Set whether the coordinator's plan is included as a message.
    #[must_use]
    pub const fn with_coordinator_visible(mut self, visible: bool) -> Self {
        self.coordinator_visible = visible;
        self
    }

    /// Set the database for turn persistence.
    #[must_use]
    pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self {
        self.db = Some(db);
        self
    }

    /// Resolve the AI provider for a persona.
    ///
    /// If the persona specifies a `provider` name and a registry is available,
    /// looks up the provider. Falls back to the default provider.
    fn resolve_provider(&self, persona: &Persona) -> Arc<dyn AiProvider> {
        if let Some(ref provider_name) = persona.provider {
            if let Some(ref registry) = self.provider_registry {
                if let Some(provider) = registry.get(provider_name) {
                    return provider;
                }
                tracing::warn!(
                    subsystem = "group_chat",
                    persona_id = %persona.id,
                    provider = %provider_name,
                    "persona provider not found in registry, using default"
                );
            }
        }
        self.default_provider.current()
    }

    /// Persist a conversation turn to the database (best-effort).
    async fn persist_turn(
        &self,
        session_id: &str,
        round: u32,
        sequence: u32,
        speaker: &Speaker,
        content: &str,
    ) {
        let Some(db) = self.db.clone() else { return };
        let session_id = session_id.to_string();
        let speaker = speaker.clone();
        let content = content.to_string();

        let result = tokio::task::spawn_blocking(move || {
            let (speaker_type, speaker_id, speaker_name) = match &speaker {
                Speaker::Coordinator => ("coordinator", None, "Coordinator"),
                Speaker::System => ("system", None, "System"),
                Speaker::Persona { id, name } => ("persona", Some(id.as_str()), name.as_str()),
            };
            db.insert_group_chat_turn(
                &session_id,
                round,
                sequence,
                speaker_type,
                speaker_id,
                speaker_name,
                &content,
            )
        })
        .await;

        match result {
            Ok(Err(e)) => {
                tracing::warn!(
                    subsystem = "group_chat",
                    error = %e,
                    "failed to persist group chat turn to database"
                );
            }
            Err(e) => {
                tracing::warn!(
                    subsystem = "group_chat",
                    error = %e,
                    "group chat turn persistence task failed to join"
                );
            }
            Ok(Ok(())) => {}
        }
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
    /// The `targets` parameter is used by the Mention action to instruct the
    /// coordinator to prioritize specific personas.
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
        targets: &[String],
    ) -> Result<Vec<GroupChatMessage>, GroupChatError> {
        if session.status != GroupChatStatus::Active {
            return Err(GroupChatError::SessionInactive(session.id.clone()));
        }
        let round = session.current_round.saturating_add(1);

        // Step 1: Record user message as a System turn
        session.add_turn(round, Speaker::System, user_message.to_string());
        self.persist_turn(&session.id, round, 0, &Speaker::System, user_message)
            .await;

        // Step 2: Build coordinator prompt and call LLM
        let history = session.build_history_text();
        let coordinator_prompt = build_coordinator_prompt(
            &session.participants,
            user_message,
            &history,
            &session.topic,
            targets,
        );

        let coordinator_raw = {
            let msgs = [UnifiedMessage::user(&coordinator_prompt)];
            self.default_provider
                .current()
                .process(RequestPayload::new(&msgs))
                .await
                .map_err(|e| GroupChatError::ProviderUnavailable(e.to_string()))?
                .text_content()
        };

        // Step 3: Parse the coordinator plan, fallback on failure
        let plan = parse_coordinator_plan(&coordinator_raw).unwrap_or_else(|e| {
            tracing::warn!(
                subsystem = "group_chat",
                error = %e,
                "coordinator plan parse failed, using fallback"
            );
            build_fallback_plan(&session.participants)
        });

        // Warn when a mentioned persona was dropped by the coordinator — a valid
        // mention that the coordinator chose to exclude leaves the caller with
        // no feedback. Detection is cheap (linear over typically < 10 targets /
        // respondents) so we don't bother with a HashSet.
        for target in targets {
            if !plan.respondents.iter().any(|r| r.persona_id == *target)
                && session.participants.iter().any(|p| &p.id == target)
            {
                tracing::warn!(
                    subsystem = "group_chat",
                    target = %target,
                    "mentioned persona not included in coordinator plan"
                );
            }
        }

        // Step 3b: Optionally include coordinator plan as a visible message
        let mut messages = Vec::new();
        let mut seq_offset = 0u32;
        // Monotonic persistence sequence within this round. The user turn above
        // occupies slot 0; each subsequent persisted turn (coordinator, then
        // personas) takes the next distinct slot so that the documented
        // `ORDER BY round, sequence` replay is unambiguous. This is independent
        // of the live-stream `sequence` field on the returned messages, which
        // omits the user turn and therefore numbers from 0.
        let mut persist_seq = 1u32;

        if self.coordinator_visible {
            let speaker = Speaker::Coordinator;
            session.add_turn(round, speaker.clone(), coordinator_raw.clone());
            self.persist_turn(&session.id, round, persist_seq, &speaker, &coordinator_raw)
                .await;
            persist_seq += 1;

            messages.push(GroupChatMessage {
                session_id: session.id.clone(),
                speaker,
                content: coordinator_raw.clone(),
                round,
                sequence: 0,
                is_final: plan.respondents.is_empty(),
            });
            seq_offset = 1;
        }

        // Step 4 & 5: Invoke each persona and collect messages
        let mut prior_discussion = String::new();
        let mut sorted_respondents = plan.respondents.clone();
        sorted_respondents.sort_by_key(|r| r.order);
        let total_respondents = sorted_respondents.len();
        let session_id = session.id.clone();

        for (i, respondent) in sorted_respondents.iter().enumerate() {
            // Find the persona in the session participants
            let persona = session
                .participants
                .iter()
                .find(|p| p.id == respondent.persona_id)
                .ok_or_else(|| GroupChatError::PersonaNotFound(respondent.persona_id.clone()))?;

            // Build persona prompt with cumulative prior discussion
            let persona_prompt = build_persona_prompt(
                persona,
                user_message,
                &prior_discussion,
                &respondent.guidance,
            );

            // Call persona LLM (resolve per-persona provider, model, thinking level).
            // `model` / `thinking_level` are honored only when the persona sets
            // them; otherwise these resolve to `None` and the request is identical
            // to using the provider's defaults.
            let provider = self.resolve_provider(persona);
            let think_level = persona
                .thinking_level
                .as_deref()
                .map(|level| {
                    level.parse::<ThinkLevel>().unwrap_or_else(|_| {
                        tracing::warn!(
                            persona = %persona.name,
                            level = %level,
                            "invalid thinking_level on persona; ignoring"
                        );
                        ThinkLevel::default()
                    })
                });
            let persona_response = {
                let msgs = [UnifiedMessage::user(&persona_prompt)];
                provider
                    .process(
                        RequestPayload::new(&msgs)
                            .with_system(Some(&persona.system_prompt))
                            .with_model(persona.model.clone())
                            .with_think_level(think_level),
                    )
                    .await
                    .map_err(|e| GroupChatError::PersonaInvocationFailed {
                        persona_id: persona.id.clone(),
                        reason: e.to_string(),
                    })?
                    .text_content()
            };

            // Record the turn in session history. Clone the persona name up front
            // so the immutable borrow of `session.participants` ends before the
            // mutable `session.add_turn` below (the name is still needed later).
            let persona_name = persona.name.clone();
            let speaker = Speaker::Persona {
                id: persona.id.clone(),
                name: persona_name.clone(),
            };
            session.add_turn(round, speaker.clone(), persona_response.clone());

            let sequence = i.try_into().unwrap_or(u32::MAX).saturating_add(seq_offset);
            self.persist_turn(&session.id, round, persist_seq, &speaker, &persona_response)
                .await;
            persist_seq = persist_seq.saturating_add(1);

            // Accumulate prior discussion for the next persona
            let _ = writeln!(prior_discussion, "[{}]: {}\n", persona_name, persona_response);

            // Build output message
            let is_final = i == total_respondents - 1;
            messages.push(GroupChatMessage {
                session_id: session_id.clone(),
                speaker,
                content: persona_response,
                round,
                sequence,
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
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;

    use crate::sync_primitives::{Arc, AtomicUsize, Ordering};
    use std::future::Future;
    use std::pin::Pin;

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
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let response = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| format!("unexpected call #{idx}"));
            Box::pin(async move { Ok(ProviderResponse::text_only(response)) })
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

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "How should we design auth?", &[])
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
        let coordinator_response = r#"{"respondents":[{"persona_id":"pm","order":0,"guidance":"Be concise"}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
            "Ship it!".to_string(),
        ]));

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "Should we ship?", &[])
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

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "Tell me about caching", &[])
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
            fn process<'a>(
                &'a self,
                _payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
            {
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
        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        let result = executor.execute_round(&mut session, "Hello?", &[]).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            GroupChatError::ProviderUnavailable(msg) => {
                assert!(
                    msg.contains("connection refused"),
                    "error should mention the cause: {msg}"
                );
            }
            other => panic!("expected ProviderUnavailable, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_execute_round_persona_invocation_failure() {
        // Coordinator succeeds, but persona call fails
        let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":"go"}],"need_summary":false}"#;

        struct CoordinatorOnlyProvider {
            coordinator_response: String,
            call_count: AtomicUsize,
        }

        impl AiProvider for CoordinatorOnlyProvider {
            fn process<'a>(
                &'a self,
                _payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
            {
                let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
                if idx == 0 {
                    let resp = self.coordinator_response.clone();
                    Box::pin(async move { Ok(ProviderResponse::text_only(resp)) })
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
        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        let result = executor.execute_round(&mut session, "Help me", &[]).await;

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
        let coordinator_response = r#"{"respondents":[{"persona_id":"ghost","order":0,"guidance":"boo"}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string()
        ]));

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        let result = executor
            .execute_round(&mut session, "Who are you?", &[])
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
        let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":""}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
            "Round 1 response.".to_string(),
            coordinator_response.to_string(),
            "Round 2 response.".to_string(),
        ]));

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        // Round 1
        let msgs1 = executor
            .execute_round(&mut session, "First", &[])
            .await
            .unwrap();
        assert_eq!(msgs1[0].round, 1);
        assert_eq!(session.current_round, 1);

        // Round 2
        let msgs2 = executor
            .execute_round(&mut session, "Second", &[])
            .await
            .unwrap();
        assert_eq!(msgs2[0].round, 2);
        assert_eq!(session.current_round, 2);
    }

    #[tokio::test]
    async fn test_prior_discussion_accumulates() {
        let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":"go first"},{"persona_id":"pm","order":1,"guidance":"go second"}],"need_summary":false}"#;

        struct EchoAfterCoordinator {
            coordinator_response: String,
            call_count: AtomicUsize,
        }

        impl AiProvider for EchoAfterCoordinator {
            fn process<'a>(
                &'a self,
                payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
            {
                let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
                if idx == 0 {
                    let resp = self.coordinator_response.clone();
                    Box::pin(async move { Ok(ProviderResponse::text_only(resp)) })
                } else {
                    let input = payload
                        .messages
                        .first()
                        .and_then(|m| m.content_blocks().first())
                        .and_then(|b| b.as_text())
                        .unwrap_or("");
                    let has_prior = input.contains("Prior discussion in this round:");
                    let response = format!("call#{idx} prior={has_prior}");
                    Box::pin(async move { Ok(ProviderResponse::text_only(response)) })
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

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "Discuss caching", &[])
            .await
            .unwrap();

        // First persona should NOT have prior discussion
        assert_eq!(messages[0].content, "call#1 prior=false");
        // Second persona SHOULD have prior discussion (from the first persona)
        assert_eq!(messages[1].content, "call#2 prior=true");
    }

    #[tokio::test]
    async fn test_coordinator_visible() {
        let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":"go"}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
            "Architect says hi.".to_string(),
        ]));

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )))
        .with_coordinator_visible(true);
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "Hello", &[])
            .await
            .unwrap();

        // Should have coordinator message + persona message
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].speaker.name(), "Coordinator");
        assert!(messages[0].content.contains("respondents"));
        assert_eq!(messages[0].sequence, 0);
        assert!(!messages[0].is_final);

        assert_eq!(messages[1].speaker.name(), "Architect");
        assert_eq!(messages[1].content, "Architect says hi.");
        assert_eq!(messages[1].sequence, 1);
        assert!(messages[1].is_final);
    }

    #[tokio::test]
    async fn test_per_persona_provider_resolution() {
        // Create a provider registry with a named provider
        let mut registry = ProviderRegistry::new();
        let special_provider = Arc::new(SequentialMockProvider::new(vec![
            "Special provider response.".to_string(),
        ]));
        registry
            .register("special".to_string(), special_provider)
            .unwrap();

        // Coordinator returns plan selecting the persona with provider override
        let coordinator_response = r#"{"respondents":[{"persona_id":"custom","order":0,"guidance":""}],"need_summary":false}"#;

        let default_provider: Arc<dyn AiProvider> = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
            // This should NOT be used for the persona call
            "Default provider response (should not appear).".to_string(),
        ]));

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            default_provider,
        )))
        .with_provider_registry(Arc::new(registry));

        let mut session = GroupChatSession::new(
            "test-provider".to_string(),
            None,
            vec![Persona {
                id: "custom".to_string(),
                name: "Custom".to_string(),
                system_prompt: "You are custom.".to_string(),
                provider: Some("special".to_string()),
                model: None,
                thinking_level: None,
            }],
            "test".to_string(),
            "test:1".to_string(),
        );

        let messages = executor
            .execute_round(&mut session, "Test provider resolution", &[])
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Special provider response.");
    }

    #[tokio::test]
    async fn test_execute_round_respects_order_field() {
        let coordinator_response = r#"{"respondents":[{"persona_id":"pm","order":1,"guidance":"second"},{"persona_id":"arch","order":0,"guidance":"first"}],"need_summary":false}"#;

        let provider = Arc::new(SequentialMockProvider::new(vec![
            coordinator_response.to_string(),
            "Architect goes first.".to_string(),
            "PM goes second.".to_string(),
        ]));

        let executor = GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(
            provider as Arc<dyn AiProvider>,
        )));
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "Who should go first?", &[])
            .await
            .expect("execute_round should succeed");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].speaker.name(), "Architect");
        assert_eq!(messages[0].content, "Architect goes first.");
        assert_eq!(messages[0].sequence, 0);

        assert_eq!(messages[1].speaker.name(), "Product Manager");
        assert_eq!(messages[1].content, "PM goes second.");
        assert_eq!(messages[1].sequence, 1);
        assert!(messages[1].is_final);
    }
}
