//! GroupChat Orchestrator — ties together persona registry, sessions, and coordination.
//!
//! The orchestrator manages session lifecycle (create, get, end, list) and
//! enforces config-driven limits (max personas, max rounds). It does NOT make
//! LLM calls — those happen at a higher layer that consumes the orchestrator.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::types::{GroupChatConfig, PersonaConfig};

use super::persona::PersonaRegistry;
use super::protocol::{GroupChatError, PersonaSource};
use super::session::GroupChatSession;

/// A shared, async-lockable session handle.
///
/// Handlers hold this Arc after a brief orchestrator lock, then lock the
/// individual session without blocking other sessions.
pub type SharedSession = Arc<tokio::sync::Mutex<GroupChatSession>>;

/// Orchestrator for multi-agent group chat sessions.
///
/// Owns the persona registry and a map of active/ended sessions.
/// Enforces configuration limits and provides session lifecycle management.
pub struct GroupChatOrchestrator {
    config: GroupChatConfig,
    persona_registry: PersonaRegistry,
    sessions: HashMap<String, SharedSession>,
}

impl GroupChatOrchestrator {
    /// Create a new orchestrator from config and persona definitions.
    pub fn new(config: GroupChatConfig, persona_configs: &[PersonaConfig]) -> Self {
        Self {
            config,
            persona_registry: PersonaRegistry::from_configs(persona_configs),
            sessions: HashMap::new(),
        }
    }

    /// Returns a reference to the current configuration.
    pub fn config(&self) -> &GroupChatConfig {
        &self.config
    }

    /// Returns a reference to the persona registry.
    pub fn persona_registry(&self) -> &PersonaRegistry {
        &self.persona_registry
    }

    /// Create a new group chat session.
    ///
    /// Returns both the session ID and a [`SharedSession`] handle so the caller
    /// can immediately lock the session after releasing the orchestrator lock.
    ///
    /// # Errors
    ///
    /// - [`GroupChatError::TooManyPersonas`] if the number of persona sources
    ///   exceeds `config.max_personas_per_session`.
    /// - [`GroupChatError::PersonaNotFound`] if a `Preset` source references
    ///   a persona ID that is not in the registry.
    pub fn create_session(
        &mut self,
        sources: Vec<PersonaSource>,
        topic: Option<String>,
        source_channel: String,
        source_session_key: String,
    ) -> Result<(String, SharedSession), GroupChatError> {
        // 1. Validate persona count
        let max = self.config.max_personas_per_session;
        if sources.len() > max {
            return Err(GroupChatError::TooManyPersonas {
                count: sources.len(),
                max,
            });
        }

        // 2. Resolve personas (validates that all presets exist)
        let participants = self.persona_registry.resolve(&sources)?;
        let participant_count = participants.len();

        // 3. Generate session ID
        let session_id = uuid::Uuid::new_v4().to_string();

        // 4. Create and store the session
        let session = GroupChatSession::new(
            session_id.clone(),
            topic,
            participants,
            source_channel,
            source_session_key,
        );
        let handle = Arc::new(tokio::sync::Mutex::new(session));
        self.sessions.insert(session_id.clone(), Arc::clone(&handle));

        tracing::info!(
            subsystem = "group_chat",
            event = "session_created",
            session_id = %session_id,
            persona_count = participant_count,
            "group chat session created"
        );

        // 5. Return both the session ID and the handle
        Ok((session_id, handle))
    }

    /// Look up a session by ID, returning a cloned [`SharedSession`] handle.
    ///
    /// The caller should drop the orchestrator lock before awaiting the
    /// session lock.
    pub fn get_session(&self, session_id: &str) -> Option<SharedSession> {
        self.sessions.get(session_id).cloned()
    }

    /// Check whether a session has exceeded the configured maximum number of rounds.
    ///
    /// The caller provides the session's `current_round` (read while holding the
    /// session lock) so the orchestrator does not need to lock the session itself.
    ///
    /// # Errors
    ///
    /// - [`GroupChatError::MaxRoundsReached`] if `current_round >= max_rounds`.
    pub fn check_round_limit(&self, current_round: u32) -> Result<(), GroupChatError> {
        let max_rounds = self.config.max_rounds as u32;
        if current_round >= max_rounds {
            return Err(GroupChatError::MaxRoundsReached(max_rounds));
        }
        Ok(())
    }

    /// Returns the configured `max_rounds` value.
    pub fn max_rounds(&self) -> u32 {
        self.config.max_rounds as u32
    }

    /// Return handles to all sessions (both active and ended).
    ///
    /// The caller can then lock each session individually to inspect status.
    pub fn all_sessions(&self) -> Vec<(String, SharedSession)> {
        self.sessions
            .iter()
            .map(|(id, handle)| (id.clone(), Arc::clone(handle)))
            .collect()
    }

    /// Reload configuration and persona definitions (e.g., after hot-reload).
    pub fn reload_config(&mut self, config: GroupChatConfig, persona_configs: &[PersonaConfig]) {
        self.config = config;
        self.persona_registry.reload(persona_configs);
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_chat::protocol::GroupChatStatus;

    fn test_config() -> GroupChatConfig {
        GroupChatConfig {
            max_personas_per_session: 4,
            max_rounds: 3,
            ..Default::default()
        }
    }

    fn test_personas() -> Vec<PersonaConfig> {
        vec![
            PersonaConfig {
                id: "arch".into(),
                name: "架构师".into(),
                system_prompt: "You are an architect".into(),
                ..Default::default()
            },
            PersonaConfig {
                id: "pm".into(),
                name: "产品经理".into(),
                system_prompt: "You are a PM".into(),
                ..Default::default()
            },
        ]
    }

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        assert_eq!(orch.all_sessions().len(), 0);
        assert_eq!(orch.config().max_personas_per_session, 4);
        assert_eq!(orch.config().max_rounds, 3);
        assert_eq!(orch.persona_registry().len(), 2);
    }

    #[tokio::test]
    async fn test_create_session() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let sources = vec![
            PersonaSource::Preset("arch".into()),
            PersonaSource::Preset("pm".into()),
        ];
        let result = orch.create_session(
            sources,
            Some("Design review".into()),
            "telegram".into(),
            "tg:12345".into(),
        );

        assert!(result.is_ok());
        let (session_id, handle) = result.unwrap();
        assert!(!session_id.is_empty());

        let session = handle.lock().await;
        assert_eq!(session.topic, Some("Design review".to_string()));
        assert_eq!(session.participants.len(), 2);
        assert_eq!(session.source_channel, "telegram");
        assert_eq!(session.source_session_key, "tg:12345");
        assert_eq!(session.status, GroupChatStatus::Active);
    }

    #[tokio::test]
    async fn test_create_session_preset_not_found() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let sources = vec![
            PersonaSource::Preset("arch".into()),
            PersonaSource::Preset("nonexistent".into()),
        ];
        let result = orch.create_session(
            sources,
            None,
            "cli".into(),
            "cli:1".into(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GroupChatError::PersonaNotFound(ref id) if id == "nonexistent"),
            "expected PersonaNotFound, got: {err:?}"
        );
        assert_eq!(orch.all_sessions().len(), 0);
    }

    #[tokio::test]
    async fn test_create_session_too_many_personas() {
        let config = GroupChatConfig {
            max_personas_per_session: 1,
            ..Default::default()
        };
        let mut orch = GroupChatOrchestrator::new(config, &test_personas());

        let sources = vec![
            PersonaSource::Preset("arch".into()),
            PersonaSource::Preset("pm".into()),
        ];
        let result = orch.create_session(
            sources,
            None,
            "cli".into(),
            "cli:1".into(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GroupChatError::TooManyPersonas { count: 2, max: 1 }),
            "expected TooManyPersonas, got: {err:?}"
        );
        assert_eq!(orch.all_sessions().len(), 0);
    }

    #[tokio::test]
    async fn test_end_session() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let sources = vec![PersonaSource::Preset("arch".into())];
        let (session_id, _) = orch
            .create_session(sources, None, "cli".into(), "cli:1".into())
            .unwrap();

        let handle = orch.get_session(&session_id).expect("session should exist");
        {
            let mut session = handle.lock().await;
            assert_eq!(session.status, GroupChatStatus::Active);
            session.end();
        }

        let session = handle.lock().await;
        assert_eq!(session.status, GroupChatStatus::Ended);
    }

    #[tokio::test]
    async fn test_end_session_not_found() {
        let orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        assert!(orch.get_session("nonexistent-session").is_none());
    }

    #[tokio::test]
    async fn test_list_active_sessions() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        // Create two sessions
        let sources_a = vec![PersonaSource::Preset("arch".into())];
        let (id_a, _) = orch
            .create_session(sources_a, Some("Session A".into()), "cli".into(), "cli:a".into())
            .unwrap();

        let sources_b = vec![PersonaSource::Preset("pm".into())];
        let (_id_b, _) = orch
            .create_session(sources_b, Some("Session B".into()), "cli".into(), "cli:b".into())
            .unwrap();

        let all = orch.all_sessions();
        assert_eq!(all.len(), 2);

        // End session A
        let handle_a = orch.get_session(&id_a).unwrap();
        handle_a.lock().await.end();

        // Count active sessions
        let mut active_count = 0;
        let mut active_topic = None;
        for (_, handle) in orch.all_sessions() {
            let s = handle.lock().await;
            if s.status == GroupChatStatus::Active {
                active_count += 1;
                active_topic = s.topic.clone();
            }
        }
        assert_eq!(active_count, 1);
        assert_eq!(active_topic, Some("Session B".to_string()));
    }
}
