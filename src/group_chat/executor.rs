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
    /// Sliding-window size for the coordinator's history text. Older rounds
    /// are collapsed into a one-line summary (see
    /// [`GroupChatSession::build_history_text_windowed`]); `0` disables the
    /// window. Sourced from `GroupChatConfig::history_window_rounds` by the
    /// startup wiring.
    history_window_rounds: u32,
    /// Set of `(persona_id, provider_name)` pairs we've already warned about
    /// falling back to the default provider. Kept behind a sync mutex so the
    /// `&self` `resolve_provider` lookup can dedupe across rounds without
    /// requiring `&mut self` (which would conflict with the executor's
    /// caller-facing `&self` API). The critical section is a single
    /// HashSet insert — never held across an `.await`.
    provider_fallback_warned:
        crate::sync_primitives::Mutex<std::collections::HashSet<(String, String)>>,
}

impl GroupChatExecutor {
    /// Create a new executor with the given default-provider handle.
    pub fn new(default_provider: Arc<dyn DefaultProviderHandle>) -> Self {
        Self {
            default_provider,
            provider_registry: None,
            coordinator_visible: false,
            db: None,
            history_window_rounds: 0,
            provider_fallback_warned: crate::sync_primitives::Mutex::new(
                std::collections::HashSet::new(),
            ),
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

    /// Set the coordinator history sliding-window size (rounds). `0` disables
    /// the window and keeps the full history (the pre-fix behavior).
    #[must_use]
    pub const fn with_history_window(mut self, window_rounds: u32) -> Self {
        self.history_window_rounds = window_rounds;
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
                // Dedupe the warn across rounds — without this, a misconfigured
                // persona produces N×M log lines (N rounds × M occurrences per
                // round) and drowns the tracing output.
                let first_miss = self
                    .provider_fallback_warned
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert((persona.id.clone(), provider_name.clone()));
                if first_miss {
                    tracing::warn!(
                        subsystem = "group_chat",
                        persona_id = %persona.id,
                        provider = %provider_name,
                        "persona provider not found in registry, using default (suppressing further warns for this persona+provider pair)"
                    );
                }
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
        let content = content.to_string();

        let (speaker_type, speaker_id, speaker_name) = match &speaker {
            Speaker::Coordinator => ("coordinator", None, "Coordinator"),
            Speaker::System => ("system", None, "System"),
            Speaker::Persona { id, name } => ("persona", Some(id.as_str()), name.as_str()),
        };
        if let Err(e) = db
            .insert_group_chat_turn(
                &session_id,
                round,
                sequence,
                speaker_type,
                speaker_id,
                speaker_name,
                &content,
            )
            .await
        {
            tracing::warn!(
                subsystem = "group_chat",
                error = %e,
                "failed to persist group chat turn to database"
            );
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

        // Transactional staging: build every (turn, message) pair for this round
        // BEFORE mutating `session.history` or persisting to the DB. If a persona
        // LLM call fails mid-round, no partial turn is committed to history or
        // persisted to the DB — without this, a 503 on persona N permanently
        // poisons `session.history` and the persisted `group_chat_turns` table
        // with an unrecoverable half-round (the previous code's behavior).
        let round = session.current_round.saturating_add(1);

        // Enforce the per-session round budget. Without this gate a hostile or
        // misconfigured coordinator could loop the executor for unbounded
        // LLM-billed rounds — the `max_rounds` config field was previously
        // declared but never consulted at the round entry point. `current_round`
        // is updated only after a fully successful commit, so the bound is
        // checked against the projected next round, not the last completed one.
        if let Some(cap) = session.max_rounds {
            if round > cap {
                return Err(GroupChatError::SessionInactive(format!(
                    "round {round} exceeds max_rounds ({cap}) for session {}",
                    session.id
                )));
            }
        }

        // Step 1: Stage user message as a System turn (NOT yet added to history).
        let mut staged_turns: Vec<(u32, u32, Speaker, String)> = Vec::new();
        // persistence sequence: user=0, coordinator=1, persona_0=2, persona_1=3, ...
        let mut persist_seq: u32 = 0;
        staged_turns.push((
            round,
            persist_seq,
            Speaker::System,
            user_message.to_string(),
        ));
        persist_seq = persist_seq.saturating_add(1);

        // Step 2: Build coordinator prompt and call LLM
        let history = session.build_history_text_windowed(self.history_window_rounds);
        let coordinator_prompt = build_coordinator_prompt(
            &session.participants,
            user_message,
            &history,
            &session.topic,
            targets,
        );

        let coordinator_raw = {
            let msgs = [UnifiedMessage::user(&coordinator_prompt)];
            // `select!` on the session cancel token: an `end_session` fired
            // while the coordinator call is in flight unwinds the round here
            // instead of waiting out the provider timeout. Cancellation maps
            // to `SessionInactive` — the same terminal signal the round-entry
            // gate uses, so callers treat it uniformly.
            let cancel = session.cancel_token.clone();
            let provider = self.default_provider.current();
            tokio::select! {
                res = provider.process(RequestPayload::new(&msgs)) => {
                    res.map_err(|e| GroupChatError::ProviderUnavailable(e.to_string()))?
                        .text_content()
                }
                _ = cancel.cancelled() => {
                    return Err(GroupChatError::SessionInactive(format!(
                        "round {round} cancelled for session {}", session.id
                    )));
                }
            }
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
        // respondents) so we don't bother with a HashSet. The same list is
        // surfaced to the caller via the last `GroupChatMessage` returned
        // (carried in the per-round result struct below) so a misbehaving or
        // rate-limited coordinator is observable in the round outcome, not
        // only in logs.
        let mut dropped_targets: Vec<String> = Vec::new();
        for target in targets {
            let is_participant = session.participants.iter().any(|p| &p.id == target);
            let is_in_plan = plan.respondents.iter().any(|r| r.persona_id == *target);
            if is_participant && !is_in_plan {
                tracing::warn!(
                    subsystem = "group_chat",
                    target = %target,
                    "mentioned persona not included in coordinator plan"
                );
                dropped_targets.push(target.clone());
            }
        }

        // Step 3b: Optionally include coordinator plan as a visible message
        let mut messages = Vec::new();
        let mut seq_offset = 0u32;

        if self.coordinator_visible {
            staged_turns.push((
                round,
                persist_seq,
                Speaker::Coordinator,
                coordinator_raw.clone(),
            ));
            persist_seq = persist_seq.saturating_add(1);

            messages.push(GroupChatMessage {
                session_id: session.id.clone(),
                speaker: Speaker::Coordinator,
                content: coordinator_raw.clone(),
                round,
                sequence: 0,
                is_final: plan.respondents.is_empty(),
                dropped_targets: Vec::new(),
            });
            seq_offset = 1;
        }

        // Step 4 & 5: Invoke each persona and prepare responses WITHOUT
        // mutating session.history or persisting to DB. If any persona call
        // fails, the snapshot is restored and no partial turn is committed.
        let mut prior_discussion = String::new();
        let mut sorted_respondents = plan.respondents.clone();
        sorted_respondents.sort_by_key(|r| r.order);
        let total_respondents = sorted_respondents.len();
        let session_id = session.id.clone();

        struct PreparedResponse {
            i: usize,
            persona_id: String,
            persona_name: String,
            content: String,
        }
        let mut prepared: Vec<PreparedResponse> = Vec::with_capacity(total_respondents);
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
            // Per `src/agents/thinking.rs` doc: callers must REJECT rather than
            // default. Silently falling back to `ThinkLevel::default()` would run
            // the turn at a depth the operator never picked. We map the parse
            // error to `None` (use the provider's documented default) and warn,
            // which preserves the spirit of the contract while keeping the
            // round running.
            let think_level: Option<ThinkLevel> = persona
                .thinking_level
                .as_deref()
                .and_then(|level| match level.parse::<ThinkLevel>() {
                    Ok(l) => Some(l),
                    Err(_) => {
                        tracing::warn!(
                            persona = %persona.name,
                            level = %level,
                            "invalid thinking_level on persona; ignoring and falling back to provider default"
                        );
                        None
                    }
                });
            let persona_response = {
                let msgs = [UnifiedMessage::user(&persona_prompt)];
                // Same `select!`-on-cancel as the coordinator call: an
                // `end_session` fired mid-persona unwinds the round instead
                // of holding the session mutex through the provider timeout.
                let cancel = session.cancel_token.clone();
                tokio::select! {
                    res = provider
                        .process(
                            RequestPayload::new(&msgs)
                                .with_system(Some(&persona.system_prompt))
                                .with_model(persona.model.clone())
                                .with_think_level(think_level),
                        ) => {
                        res.map_err(|e| GroupChatError::PersonaInvocationFailed {
                            persona_id: persona.id.clone(),
                            reason: e.to_string(),
                        })?
                        .text_content()
                    }
                    _ = cancel.cancelled() => {
                        return Err(GroupChatError::SessionInactive(format!(
                            "round {round} cancelled for session {}", session.id
                        )));
                    }
                }
            };

            // Accumulate prior discussion for the next persona
            let _ = writeln!(
                prior_discussion,
                "[{}]: {}
",
                persona.name, persona_response
            );

            prepared.push(PreparedResponse {
                i,
                persona_id: persona.id.clone(),
                persona_name: persona.name.clone(),
                content: persona_response,
            });
        }

        // All LLM calls succeeded. Commit: extend session.history, build
        // GroupChatMessages, and persist every staged turn. Past this point
        // any DB persistence error is best-effort (logged) and the round is
        // considered successful from the caller's perspective.
        //
        // **Audit fix**: when the coordinator returns an empty plan
        // (`respondents: []`), `prepared` is empty and nothing persona-side
        // was produced. The previous code still appended the user/system turn
        // to history, advanced `current_round`, and wrote an orphan `system`
        // row to `group_chat_turns` — replays then showed a round gap. An
        // empty plan is a no-op round: skip the commit entirely so the next
        // round reuses the same round number and no orphan row lands in the
        // DB. The caller still gets an empty `messages` list (or just the
        // coordinator's raw plan when `coordinator_visible` is on) and can
        // decide whether to surface the no-op.
        if prepared.is_empty() {
            tracing::debug!(
                subsystem = "group_chat",
                session_id = %session.id,
                round = round,
                "coordinator returned an empty plan; round not committed"
            );
            return Ok(messages);
        }
        // Append the user/system turn to history (matches the original
        // semantic that `add_turn` records both user prompts and persona
        // responses).
        session.add_turn(
            round,
            Speaker::System,
            // Safe: staged_turns[0] is the user turn we pushed at the top of
            // this function. Cloning avoids moving out of the Vec while we
            // still need to iterate it for persistence below.
            staged_turns[0].3.clone(),
        );
        for p in &prepared {
            let speaker = Speaker::Persona {
                id: p.persona_id.clone(),
                name: p.persona_name.clone(),
            };
            session.add_turn(round, speaker.clone(), p.content.clone());
            staged_turns.push((round, persist_seq, speaker, p.content.clone()));
            persist_seq = persist_seq.saturating_add(1);
        }

        // Advance current_round after a successful commit. Note: this is NOT
        // transactional with the DB persistence below — `session.history` and
        // `session.current_round` are mutated before `persist_turn` runs, so a
        // cancel that lands between this assignment and the persistence loop
        // leaves the in-memory session one round ahead of the
        // `group_chat_turns` table. There is no rollback path here (none was
        // ever needed before the staging refactor); the asymmetry is
        // recoverable on next round by re-staging the user turn. If we ever
        // add a transactional guarantee, this is the point to attach it.
        session.current_round = round;

        // Persist every staged turn to the DB. Best-effort: a DB error here
        // is logged via `persist_turn` itself and does not propagate — the
        // round is considered successful from the caller's perspective once
        // the in-memory state is committed above.
        for (round_v, seq, speaker, content) in &staged_turns {
            self.persist_turn(&session.id, *round_v, *seq, speaker, content)
                .await;
        }

        // Build the live-stream `GroupChatMessage` list. The persistence
        // sequence (which includes the user turn) is independent of the
        // live-stream sequence (which omits the user turn and numbers from 0).
        for p in &prepared {
            let speaker = Speaker::Persona {
                id: p.persona_id.clone(),
                name: p.persona_name.clone(),
            };
            let sequence =
                p.i.try_into()
                    .unwrap_or(u32::MAX)
                    .saturating_add(seq_offset);
            let is_final = p.i + 1 == total_respondents;
            // Dropped-mention surfacing: only the FINAL message carries the
            // dropped_targets list — intermediate messages default to empty so
            // older consumers (which did not know about the field) keep
            // working. The audit finding is that the caller never learned a
            // mention was silently dropped by the coordinator; without this
            // the only signal was a `tracing::warn!` line.
            let dropped_for_this_msg = if is_final {
                std::mem::take(&mut dropped_targets)
            } else {
                Vec::new()
            };
            messages.push(GroupChatMessage {
                session_id: session_id.clone(),
                speaker,
                content: p.content.clone(),
                round,
                sequence,
                is_final,
                dropped_targets: dropped_for_this_msg,
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

    /// Regression test for the critical audit finding (batch-3 was the
    /// last reviewer, but the original transactional staging was not yet in
    /// place when this audit ran): a failed persona call must NOT leave
    /// partial turns in `session.history` or advance `current_round`. The
    /// pre-fix behavior was to mutate `history` and call `persist_turn`
    /// incrementally, so a 503 on persona N would silently poison the
    /// session.
    #[tokio::test]
    async fn test_execute_round_rollback_on_persona_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Coordinator succeeds; the first persona succeeds; the second fails.
        let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":""},{"persona_id":"pm","order":1,"guidance":""}],"need_summary":false}"#;

        struct PartialFailProvider {
            coordinator_response: String,
            call_count: AtomicUsize,
        }

        impl AiProvider for PartialFailProvider {
            fn process<'a>(
                &'a self,
                _payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
            {
                let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
                if idx == 0 {
                    let resp = self.coordinator_response.clone();
                    Box::pin(async move { Ok(ProviderResponse::text_only(resp)) })
                } else if idx == 1 {
                    Box::pin(async move { Ok(ProviderResponse::text_only("arch ok".to_string())) })
                } else {
                    Box::pin(async { Err(crate::error::AlephError::provider("model overloaded")) })
                }
            }
            fn name(&self) -> &str {
                "partial-fail"
            }
            fn color(&self) -> &str {
                "#ff0000"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(PartialFailProvider {
            coordinator_response: coordinator_response.to_string(),
            call_count: AtomicUsize::new(0),
        });
        let executor =
            GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(provider)));
        let mut session = make_session();

        let history_before = session.history.len();
        let round_before = session.current_round;

        let result = executor
            .execute_round(&mut session, "Please discuss", &[])
            .await;

        assert!(result.is_err(), "should return error");
        // Session state must be UNCHANGED — no orphan partial turn.
        assert_eq!(session.history.len(), history_before);
        assert_eq!(session.current_round, round_before);
    }

    /// Regression test for the audit finding: a misconfigured persona provider
    /// should warn ONCE, not N×M times across many rounds.
    #[tokio::test]
    async fn test_resolve_provider_warn_is_deduped() {
        use crate::providers::adapter::RequestPayload;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Coordinator returns a single-persona plan; the persona references a
        // provider name that does not exist in the registry, so the executor
        // must fall back to the default AND emit the warn only on first miss.
        let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":""}],"need_summary":false}"#;

        struct TwoCallProvider {
            coordinator_response: String,
            call_count: AtomicUsize,
        }
        impl AiProvider for TwoCallProvider {
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
                    Box::pin(async move { Ok(ProviderResponse::text_only("ok".to_string())) })
                }
            }
            fn name(&self) -> &str {
                "two-call"
            }
            fn color(&self) -> &str {
                "#000000"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(TwoCallProvider {
            coordinator_response: coordinator_response.to_string(),
            call_count: AtomicUsize::new(0),
        });

        let mut registry = crate::providers::ProviderRegistry::new();
        // Register a sentinel "arch" provider that has NO matching name;
        // we resolve via the registry path but on a name that won't be
        // found, triggering the warn path.
        let _ = registry.register("other".to_string(), provider.clone());

        let executor =
            GroupChatExecutor::new(Arc::new(crate::providers::StaticDefault::new(provider)))
                .with_provider_registry(Arc::new(registry));

        let mut session = make_session();
        session.participants[0].provider = Some("nonexistent".to_string());

        let _ = executor.execute_round(&mut session, "round 1", &[]).await;

        // After one round the dedupe set must contain exactly one entry;
        // a second round would also produce exactly one entry (no growth).
        let set_size = executor
            .provider_fallback_warned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        assert_eq!(set_size, 1, "first miss must insert one entry");

        let _ = executor.execute_round(&mut session, "round 2", &[]).await;

        let set_size_after = executor
            .provider_fallback_warned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        assert_eq!(
            set_size_after, 1,
            "second miss with same (persona, provider) must NOT grow the set"
        );
    }
}
