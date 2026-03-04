//! GroupChat Orchestrator — ties together persona registry, sessions, and coordination.
//!
//! The orchestrator manages session lifecycle (create, get, end, list) and
//! enforces config-driven limits (max personas, max rounds). It does NOT make
//! LLM calls — those happen at a higher layer that consumes the orchestrator.

use std::collections::HashMap;

use crate::config::types::{GroupChatConfig, PersonaConfig};

use super::persona::PersonaRegistry;
use super::protocol::{GroupChatError, GroupChatStatus, PersonaSource};
use super::session::GroupChatSession;

/// Orchestrator for multi-agent group chat sessions.
///
/// Owns the persona registry and a map of active/ended sessions.
/// Enforces configuration limits and provides session lifecycle management.
pub struct GroupChatOrchestrator {
    config: GroupChatConfig,
    persona_registry: PersonaRegistry,
    sessions: HashMap<String, GroupChatSession>,
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
    ) -> Result<String, GroupChatError> {
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
        self.sessions.insert(session_id.clone(), session);

        // 5. Return the session ID
        Ok(session_id)
    }

    /// Look up a session by ID (immutable).
    pub fn get_session(&self, session_id: &str) -> Option<&GroupChatSession> {
        self.sessions.get(session_id)
    }

    /// Look up a session by ID (mutable).
    pub fn get_session_mut(&mut self, session_id: &str) -> Option<&mut GroupChatSession> {
        self.sessions.get_mut(session_id)
    }

    /// End a session, setting its status to [`GroupChatStatus::Ended`].
    ///
    /// # Errors
    ///
    /// Returns [`GroupChatError::SessionNotFound`] if the session does not exist.
    pub fn end_session(&mut self, session_id: &str) -> Result<(), GroupChatError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| GroupChatError::SessionNotFound(session_id.to_string()))?;
        session.end();
        Ok(())
    }

    /// Count sessions that are currently [`GroupChatStatus::Active`].
    pub fn active_session_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|s| s.status == GroupChatStatus::Active)
            .count()
    }

    /// Return references to all sessions that are currently [`GroupChatStatus::Active`].
    pub fn list_active_sessions(&self) -> Vec<&GroupChatSession> {
        self.sessions
            .values()
            .filter(|s| s.status == GroupChatStatus::Active)
            .collect()
    }

    /// Check whether a session has exceeded the configured maximum number of rounds.
    ///
    /// # Errors
    ///
    /// - [`GroupChatError::SessionNotFound`] if the session does not exist.
    /// - [`GroupChatError::MaxRoundsReached`] if `current_round >= max_rounds`.
    pub fn check_round_limit(&self, session_id: &str) -> Result<(), GroupChatError> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| GroupChatError::SessionNotFound(session_id.to_string()))?;

        let max_rounds = self.config.max_rounds as u32;
        if session.current_round >= max_rounds {
            return Err(GroupChatError::MaxRoundsReached(max_rounds));
        }

        Ok(())
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

    #[test]
    fn test_orchestrator_creation() {
        let orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        assert_eq!(orch.active_session_count(), 0);
        assert_eq!(orch.config().max_personas_per_session, 4);
        assert_eq!(orch.config().max_rounds, 3);
        assert_eq!(orch.persona_registry().len(), 2);
    }

    #[test]
    fn test_create_session() {
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
        let session_id = result.unwrap();
        assert!(!session_id.is_empty());
        assert_eq!(orch.active_session_count(), 1);

        let session = orch.get_session(&session_id).expect("session should exist");
        assert_eq!(session.topic, Some("Design review".to_string()));
        assert_eq!(session.participants.len(), 2);
        assert_eq!(session.source_channel, "telegram");
        assert_eq!(session.source_session_key, "tg:12345");
        assert_eq!(session.status, GroupChatStatus::Active);
    }

    #[test]
    fn test_create_session_preset_not_found() {
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
        assert_eq!(orch.active_session_count(), 0);
    }

    #[test]
    fn test_create_session_too_many_personas() {
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
        assert_eq!(orch.active_session_count(), 0);
    }

    #[test]
    fn test_end_session() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let sources = vec![PersonaSource::Preset("arch".into())];
        let session_id = orch
            .create_session(sources, None, "cli".into(), "cli:1".into())
            .unwrap();

        assert_eq!(orch.active_session_count(), 1);

        let result = orch.end_session(&session_id);
        assert!(result.is_ok());

        let session = orch.get_session(&session_id).expect("session should still exist");
        assert_eq!(session.status, GroupChatStatus::Ended);
        assert_eq!(orch.active_session_count(), 0);
    }

    #[test]
    fn test_end_session_not_found() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let result = orch.end_session("nonexistent-session");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GroupChatError::SessionNotFound(ref id) if id == "nonexistent-session"),
            "expected SessionNotFound, got: {err:?}"
        );
    }

    #[test]
    fn test_list_active_sessions() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        // Create two sessions
        let sources_a = vec![PersonaSource::Preset("arch".into())];
        let id_a = orch
            .create_session(sources_a, Some("Session A".into()), "cli".into(), "cli:a".into())
            .unwrap();

        let sources_b = vec![PersonaSource::Preset("pm".into())];
        let _id_b = orch
            .create_session(sources_b, Some("Session B".into()), "cli".into(), "cli:b".into())
            .unwrap();

        assert_eq!(orch.active_session_count(), 2);
        assert_eq!(orch.list_active_sessions().len(), 2);

        // End one session
        orch.end_session(&id_a).unwrap();

        assert_eq!(orch.active_session_count(), 1);
        let active = orch.list_active_sessions();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].topic, Some("Session B".to_string()));
    }
}
