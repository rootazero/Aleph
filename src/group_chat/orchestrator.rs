//! `GroupChat` Orchestrator — ties together persona registry, sessions, and coordination.
//!
//! The orchestrator manages session lifecycle (create, get, end, list) and
//! enforces config-driven limits (max personas, max rounds). It does NOT make
//! LLM calls — those happen at a higher layer that consumes the orchestrator.

use std::collections::HashMap;

use crate::config::types::{GroupChatConfig, PersonaConfig};
use crate::resilience::database::StateDatabase;
use crate::sync_primitives::{Arc, Mutex};

use super::persona::PersonaRegistry;
use super::protocol::{GroupChatError, GroupChatStatus, PersonaSource};
use super::session::GroupChatSession;

/// A shared, async-lockable session handle.
///
/// Handlers hold this Arc after a brief orchestrator lock, then lock the
/// individual session without blocking other sessions.
pub type SharedSession = Arc<tokio::sync::Mutex<GroupChatSession>>;

// Result of removing a session from the orchestrator map. The handle is
// returned so the caller can perform a final state read or DB update while
// holding the per-session lock atomically.
struct RemovedSession {
    session_id: String,
    handle: SharedSession,
}

/// Orchestrator for multi-agent group chat sessions.
///
/// Owns the persona registry and a map of active sessions.
/// Enforces configuration limits and provides session lifecycle management.
///
/// Optionally persists sessions and turns to a [`StateDatabase`].
pub struct GroupChatOrchestrator {
    config: GroupChatConfig,
    persona_registry: PersonaRegistry,
    sessions: Mutex<HashMap<String, SharedSession>>,
    db: Option<Arc<StateDatabase>>,
}

impl GroupChatOrchestrator {
    /// Create a new orchestrator from config and persona definitions.
    #[must_use]
    pub fn new(config: GroupChatConfig, persona_configs: &[PersonaConfig]) -> Self {
        Self {
            config,
            persona_registry: PersonaRegistry::from_configs(persona_configs),
            sessions: Mutex::new(HashMap::new()),
            db: None,
        }
    }

    /// Set the database for session persistence (optional).
    pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self {
        self.db = Some(db);
        self
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
    /// - [`GroupChatError::InvalidPersona`] if an inline persona fails validation.
    pub async fn create_session(
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

        // 2. Validate inline personas BEFORE resolve() so an inline persona's
        // validation error is surfaced even when a later source is a missing
        // preset. Without this, `resolve()` short-circuits on the preset error
        // and the operator never sees the inline error.
        for source in &sources {
            if let PersonaSource::Inline(p) = source {
                p.validate()?;
            }
        }

        // 3. Resolve personas (validates that all presets exist)
        let participants = self.persona_registry.resolve(&sources)?;

        // 3a. Validate EVERY resolved persona, not just inline ones. Inline
        // personas were already validated at step 2 (before resolve, so an
        // inline error isn't masked by a missing preset) — revalidating them
        // here is idempotent. Preset personas loaded from config skip the
        // step-2 path, and a config typo like `thinking_level = "hgh"` would
        // otherwise only surface as a silent provider-default fallback at
        // round time (see `Persona::validate` for the REJECT-not-default
        // contract this enforces).
        for p in &participants {
            p.validate()?;
        }

        if participants.is_empty() {
            return Err(GroupChatError::InvalidPersona(
                "at least one persona is required".into(),
            ));
        }

        // 3b. Reject duplicate persona IDs in the same session. The
        // coordinator's lookup by id resolves the FIRST match, so a duplicate
        // id would silently route responses to the wrong persona.
        let mut seen_ids: std::collections::HashSet<&str> =
            std::collections::HashSet::with_capacity(participants.len());
        for p in &participants {
            if !seen_ids.insert(p.id.as_str()) {
                return Err(GroupChatError::InvalidPersona(format!(
                    "duplicate persona id '{}' in session",
                    p.id
                )));
            }
        }

        let participant_count = participants.len();

        // 4. Generate session ID
        let session_id = uuid::Uuid::new_v4().to_string();

        // 5. Create and store the session
        let session = GroupChatSession::new(
            session_id.clone(),
            topic.clone(),
            participants,
            source_channel.clone(),
            source_session_key.clone(),
        )
        // Stamp the per-session round budget from config so `execute_round`
        // can enforce it. The config value is u32-clamped in `max_rounds()`;
        // we propagate the same clamp here. `0` is treated as "unbounded"
        // (None) for backwards compat with the existing default of 0 in
        // several test fixtures.
        .with_max_rounds({
            let cap = self.max_rounds();
            if cap == 0 { None } else { Some(cap) }
        });
        // Capture the ownership stamp BEFORE moving `session` into the Arc<Mutex>;
        // `GroupChatSession::new` reads it from `crate::scope::current_scope()`
        // and once the session is behind the mutex we'd have to lock+unlock just
        // to read it back for persistence.
        let owner_user_id = session.owner_user_id.clone();
        let handle = Arc::new(tokio::sync::Mutex::new(session));
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.clone(), Arc::clone(&handle));

        // 6. Persist to database if available
        if let Some(db) = &self.db {
            if let Err(e) = db
                .insert_group_chat_session(
                    &session_id,
                    topic.as_deref(),
                    &source_channel,
                    &source_session_key,
                    owner_user_id.as_deref(),
                )
                .await
            {
                tracing::warn!(
                    subsystem = "group_chat",
                    error = %e,
                    "failed to persist group chat session to database"
                );
            }
        }

        tracing::info!(
            subsystem = "group_chat",
            event = "session_created",
            session_id = %session_id,
            persona_count = participant_count,
            "group chat session created"
        );

        Ok((session_id, handle))
    }

    /// Look up a session by ID, returning a cloned [`SharedSession`] handle.
    ///
    /// The caller should drop the orchestrator lock before awaiting the
    /// session lock.
    pub fn get_session(&self, session_id: &str) -> Option<SharedSession> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
    }

    /// End a session and remove it from the active sessions map.
    ///
    /// Returns the [`SharedSession`] handle if the session existed, so the
    /// caller can still read its final state. Returns `None` if the session
    /// was not found.
    ///
    /// **Atomicity contract**: this function awaits the per-session lock
    /// AFTER releasing the orchestrator-map lock, so a round that holds the
    /// session mutex cannot slip past `end_session` while the map already
    /// shows the session gone. The orchestrator mutex is dropped before the
    /// await (no nested-lock deadlock is possible — no code path in
    /// `execute_round` reacquires the orchestrator mutex). The previous
    /// implementation used `try_lock`, which left `session.status == Active`
    /// whenever a round was in flight, producing a three-way split-brain
    /// (orchestrator map / in-memory / DB).
    pub async fn end_session(&mut self, session_id: &str) -> Option<SharedSession> {
        let RemovedSession { session_id, handle } = self.remove_session(session_id)?;

        // Authoritatively end the session under its own mutex.
        {
            let mut session = handle.lock().await;
            session.end();
        }

        // Persist status change to database
        if let Some(db) = &self.db {
            if let Err(e) = db
                .update_group_chat_session_status(&session_id, GroupChatStatus::Ended.as_str())
                .await
            {
                tracing::warn!(
                    subsystem = "group_chat",
                    error = %e,
                    "failed to persist group chat session end to database"
                );
            }
        }

        tracing::info!(
            subsystem = "group_chat",
            event = "session_ended",
            session_id = %session_id,
            "group chat session ended and removed"
        );

        Some(handle)
    }

    /// Helper: remove a session from the map and return its handle. Split
    /// out so `end_session`'s atomicity is visible (lock dropped before await).
    fn remove_session(&self, session_id: &str) -> Option<RemovedSession> {
        let (session_id, handle) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove_entry(session_id)?;
        Some(RemovedSession { session_id, handle })
    }

    /// Returns the configured `max_rounds` value.
    pub fn max_rounds(&self) -> u32 {
        u32::try_from(self.config.max_rounds).unwrap_or_else(|_| {
            tracing::warn!(
                subsystem = "group_chat",
                "max_rounds exceeds u32::MAX, clamping to u32::MAX"
            );
            u32::MAX
        })
    }

    /// Return handles to all active sessions.
    ///
    /// The caller can then lock each session individually to inspect status.
    pub fn all_sessions(&self) -> Vec<(String, SharedSession)> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(id, handle)| (id.clone(), Arc::clone(handle)))
            .collect()
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
        assert_eq!(orch.max_rounds(), 3);
    }

    #[tokio::test]
    async fn test_create_session() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let sources = vec![
            PersonaSource::Preset("arch".into()),
            PersonaSource::Preset("pm".into()),
        ];
        let result = orch
            .create_session(
                sources,
                Some("Design review".into()),
                "telegram".into(),
                "tg:12345".into(),
            )
            .await;

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
        let result = orch
            .create_session(sources, None, "cli".into(), "cli:1".into())
            .await;

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
        let result = orch
            .create_session(sources, None, "cli".into(), "cli:1".into())
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GroupChatError::TooManyPersonas { count: 2, max: 1 }),
            "expected TooManyPersonas, got: {err:?}"
        );
        assert_eq!(orch.all_sessions().len(), 0);
    }

    #[tokio::test]
    async fn test_create_session_invalid_inline_persona() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let sources = vec![PersonaSource::Inline(
            crate::group_chat::protocol::Persona {
                id: "".into(), // empty id -> invalid
                name: "Bad".into(),
                system_prompt: "prompt".into(),
                provider: None,
                model: None,
                thinking_level: None,
            },
        )];
        let result = orch
            .create_session(sources, None, "cli".into(), "cli:1".into())
            .await;
        assert!(matches!(
            result.unwrap_err(),
            GroupChatError::InvalidPersona(_)
        ));
    }

    #[tokio::test]
    async fn test_end_session() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let sources = vec![PersonaSource::Preset("arch".into())];
        let (session_id, _) = orch
            .create_session(sources, None, "cli".into(), "cli:1".into())
            .await
            .unwrap();

        assert_eq!(orch.all_sessions().len(), 1);

        // End session via orchestrator — removes from map
        let handle = orch
            .end_session(&session_id)
            .await
            .expect("session should exists");
        assert_eq!(
            orch.all_sessions().len(),
            0,
            "session should be removed from map"
        );

        let session = handle.lock().await;
        assert_eq!(session.status, GroupChatStatus::Ended);
        drop(session);
    }

    #[tokio::test]
    async fn test_end_session_not_found() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        assert!(orch.end_session("nonexistent-session").await.is_none());
    }

    #[tokio::test]
    async fn test_list_active_sessions() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        // Create two sessions
        let sources_a = vec![PersonaSource::Preset("arch".into())];
        let (id_a, _) = orch
            .create_session(
                sources_a,
                Some("Session A".into()),
                "cli".into(),
                "cli:a".into(),
            )
            .await
            .unwrap();

        let sources_b = vec![PersonaSource::Preset("pm".into())];
        let (_id_b, _) = orch
            .create_session(
                sources_b,
                Some("Session B".into()),
                "cli".into(),
                "cli:b".into(),
            )
            .await
            .unwrap();

        assert_eq!(orch.all_sessions().len(), 2);

        // End session A — removes from map
        orch.end_session(&id_a).await;
        assert_eq!(orch.all_sessions().len(), 1);

        // Remaining session should be Session B
        let remaining = orch.all_sessions();
        let session = remaining[0].1.lock().await;
        assert_eq!(session.topic, Some("Session B".to_string()));
    }

    /// Regression test: validate inline personas BEFORE resolve() so an
    /// inline persona's validation error surfaces even when a later source
    /// is a missing preset.
    #[tokio::test]
    async fn test_create_session_inline_validation_before_resolve() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        // First source: invalid inline (empty id). Second source: missing preset.
        // Pre-fix: resolve() short-circuits on the missing preset and the inline
        // error is never surfaced. Post-fix: inline validation runs first.
        let sources = vec![
            PersonaSource::Inline(crate::group_chat::protocol::Persona {
                id: String::new(), // empty id -> invalid
                name: "Bad".into(),
                system_prompt: "prompt".into(),
                provider: None,
                model: None,
                thinking_level: None,
            }),
            PersonaSource::Preset("nonexistent".into()),
        ];
        let result = orch
            .create_session(sources, None, "cli".into(), "cli:1".into())
            .await;
        assert!(matches!(
            result.unwrap_err(),
            GroupChatError::InvalidPersona(_)
        ));
    }

    /// Regression test: duplicate persona IDs in the same session are
    /// rejected. Without this, the coordinator's id-based lookup routes to
    /// whichever persona appears first, silently misrouting responses.
    #[tokio::test]
    async fn test_create_session_rejects_duplicate_persona_ids() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());

        let sources = vec![
            PersonaSource::Preset("arch".into()),
            PersonaSource::Inline(crate::group_chat::protocol::Persona {
                id: "arch".into(), // duplicate of the preset
                name: "Shadow Arch".into(),
                system_prompt: "you are shadow arch".into(),
                provider: None,
                model: None,
                thinking_level: None,
            }),
        ];
        let result = orch
            .create_session(sources, None, "cli".into(), "cli:1".into())
            .await;
        match result.unwrap_err() {
            GroupChatError::InvalidPersona(msg) => {
                assert!(
                    msg.contains("duplicate"),
                    "error should mention duplicate: {msg}"
                );
                assert!(msg.contains("arch"), "error should name the id: {msg}");
            }
            other => panic!("expected InvalidPersona, got: {other:?}"),
        }
        assert_eq!(orch.all_sessions().len(), 0);
    }
}
