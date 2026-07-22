//! Session coordinator — thin wrapper that orchestrates `SessionStore`,
//! `MessageRouter`, `EventLogStore` and `ArtifactStore` for collaborative sessions.

use chrono::Utc;
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact, TaskStatus};
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::teams::sessions::store::SessionStore;
use crate::teams::sessions::types::{
    CollaborativeSession, NewSession, SessionOutcome, SessionTurn,
};

/// Orchestrates session lifecycle across stores and routers.
pub struct SessionCoordinator {
    session_store: Arc<dyn SessionStore>,
    msg_router: Arc<MessageRouter>,
    event_store: Arc<dyn EventLogStore>,
    artifact_store: Arc<dyn ArtifactStore>,
}

impl SessionCoordinator {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        msg_router: Arc<MessageRouter>,
        event_store: Arc<dyn EventLogStore>,
        artifact_store: Arc<dyn ArtifactStore>,
    ) -> Self {
        Self {
            session_store,
            msg_router,
            event_store,
            artifact_store,
        }
    }

    /// Create a session and notify all participants via a system message.
    pub async fn start_session(
        &self,
        input: NewSession,
        leader_id: &str,
    ) -> Result<CollaborativeSession> {
        let participants = input.participants.clone();
        let topic = input.topic.clone();
        let team_id = input.team_id.clone();

        let session = self.session_store.create_session(input).await?;

        if let Err(e) = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: team_id.clone(),
                event_type: TeamEventType::SessionStarted,
                agent_id: leader_id.to_string(),
                payload: serde_json::json!({
                    "session_id": session.id,
                    "topic": topic,
                    "participants": participants,
                }),
            })
            .await
        {
            tracing::warn!(
                session_id = %session.id,
                team_id = %team_id,
                error = %e,
                "SessionCoordinator: SessionStarted event log failed; session remains active"
            );
        }

        if let Err(e) = self
            .msg_router
            .send(SendRequest {
                team_id: team_id.clone(),
                from_agent: "system".to_string(),
                to: participants,
                cc: vec![],
                msg_type: MessageType::SystemNotification,
                subject: format!("Collaborative session: {topic}"),
                content: format!(
                    "You've been invited to collaborative session: {topic} (session_id: {})",
                    session.id
                ),
                reply_to: None,
                attachments: vec![],
            })
            .await
        {
            tracing::warn!(
                session_id = %session.id,
                team_id = %team_id,
                error = %e,
                "SessionCoordinator: participant notification failed; session remains active but participants may be unaware"
            );
        }

        debug!(session_id = %session.id, "Session started");
        Ok(session)
    }

    /// Add a turn to the session. If `turn_number == max_rounds`, send a
    /// "final round" notification to all participants.
    pub async fn submit_turn(&self, session_id: &str, agent_id: &str, content: &str) -> Result<()> {
        let session = self
            .session_store
            .get_session(session_id)
            .await?
            .ok_or_else(|| AlephError::other(format!("Session not found: {session_id}")))?;

        // Authorization: only declared participants may speak in a session.
        // The `session_participants` table is otherwise never consulted for
        // access control, so without this any agent that knows a session_id
        // could inject turns into a discussion it was never invited to.
        if !session.participants.iter().any(|p| p == agent_id) {
            return Err(AlephError::other(format!(
                "agent '{agent_id}' is not a participant of session {session_id}"
            )));
        }

        let turn_number = self
            .session_store
            .add_turn(
                session_id,
                SessionTurn {
                    agent_id: agent_id.to_string(),
                    content: content.to_string(),
                    turn_number: 0, // ignored — computed atomically by the store
                    timestamp: Utc::now(),
                },
            )
            .await?;

        // If this was the final round, notify participants
        if turn_number == session.max_rounds {
            let _ = self
                .msg_router
                .send(SendRequest {
                    team_id: session.team_id.clone(),
                    from_agent: "system".to_string(),
                    to: session.participants.clone(),
                    cc: vec![],
                    msg_type: MessageType::SystemNotification,
                    subject: format!("Final round reached: {}", session.topic),
                    content: format!(
                        "Session '{}' has reached its final round ({}/{}). \
                         Please conclude the discussion.",
                        session.topic, turn_number, session.max_rounds
                    ),
                    reply_to: None,
                    attachments: vec![],
                })
                .await;
        }

        Ok(())
    }

    /// Conclude the session, log an event, and optionally save an artifact.
    pub async fn finalize(
        &self,
        session_id: &str,
        agent_id: &str,
        outcome: SessionOutcome,
    ) -> Result<()> {
        // Fetch session info before concluding (for event logging + artifact)
        let session = self
            .session_store
            .get_session(session_id)
            .await?
            .ok_or_else(|| AlephError::other(format!("Session not found: {session_id}")))?;

        // Authorization: only a declared participant may conclude the session
        // (the leader orchestrates conclusion via this path). Without this,
        // any agent that knows the session_id could force-conclude a
        // discussion it does not belong to.
        if !session.participants.iter().any(|p| p == agent_id) {
            return Err(AlephError::other(format!(
                "agent '{agent_id}' is not a participant of session {session_id}"
            )));
        }

        self.session_store
            .conclude_session(session_id, outcome.clone())
            .await?;

        // Log SessionConcluded event
        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: session.team_id.clone(),
                event_type: TeamEventType::SessionConcluded,
                agent_id: "system".to_string(),
                payload: serde_json::json!({
                    "session_id": session_id,
                    "conclusion": outcome.conclusion,
                    "agreed_by": outcome.agreed_by,
                }),
            })
            .await;

        // Save conclusion as artifact if there is an associated thread_id
        // (we use thread_id as a proxy for a task reference)
        if let Some(ref thread_id) = session.thread_id {
            let _ = self
                .artifact_store
                .create_artifact(NewArtifact {
                    task_id: thread_id.clone(),
                    agent_id: "system".to_string(),
                    artifact_type: ArtifactType::Report,
                    title: format!("Session conclusion: {}", session.topic),
                    content: outcome.conclusion,
                    metadata: serde_json::json!({
                        "session_id": session_id,
                        "agreed_by": outcome.agreed_by,
                        "dissent": outcome.dissent,
                    }),
                    status: TaskStatus::Completed,
                    blocked_by: vec![],
                    assignee: None,
                    priority: 0,
                })
                .await;
        }

        debug!(session_id = %session_id, "Session finalized");
        Ok(())
    }

    /// Cancel a session. Only a declared participant may abandon a session
    /// (same authorization rule as `submit_turn` / `finalize`).
    pub async fn cancel(&self, session_id: &str, agent_id: &str) -> Result<()> {
        let session = self
            .session_store
            .get_session(session_id)
            .await?
            .ok_or_else(|| AlephError::other(format!("Session not found: {session_id}")))?;

        if !session.participants.iter().any(|p| p == agent_id) {
            return Err(AlephError::other(format!(
                "agent '{agent_id}' is not a participant of session {session_id}"
            )));
        }

        self.session_store.cancel_session(session_id).await?;
        debug!(session_id = %session_id, "Session cancelled");
        Ok(())
    }

    /// Mark stuck sessions Deadlocked and nudge their participants.
    ///
    /// Sessions whose transcript reached `max_rounds` without a conclusion
    /// (further turns are rejected by the store) or that have been silent for
    /// `stale_secs` would otherwise stay Active forever — `Deadlocked` was a
    /// declared status with no writer. For each swept session this logs a
    /// `SessionDeadlocked` team event and asks participants to conclude or
    /// cancel. Returns the number of sessions marked.
    pub async fn sweep_deadlocked(&self, grace_secs: i64, stale_secs: i64) -> Result<usize> {
        let ids = self
            .session_store
            .sweep_deadlocked(grace_secs, stale_secs)
            .await?;

        for id in &ids {
            let Some(session) = self.session_store.get_session(id).await? else {
                continue;
            };

            let _ = self
                .event_store
                .log_event(NewTeamEvent {
                    team_id: session.team_id.clone(),
                    event_type: TeamEventType::SessionDeadlocked,
                    agent_id: "system".to_string(),
                    payload: serde_json::json!({
                        "session_id": id,
                        "topic": session.topic,
                        "turns": session.transcript.len(),
                        "max_rounds": session.max_rounds,
                    }),
                })
                .await;

            let _ = self
                .msg_router
                .send(SendRequest {
                    team_id: session.team_id.clone(),
                    from_agent: "system".to_string(),
                    to: session.participants.clone(),
                    cc: vec![],
                    msg_type: MessageType::SystemNotification,
                    subject: format!("Session deadlocked: {}", session.topic),
                    content: format!(
                        "Session '{}' (session_id: {id}) was marked deadlocked after \
                         {} of {} rounds with no conclusion. Please wrap it up: \
                         session_turn(mode='conclude') to record the outcome, or \
                         session_turn(mode='cancel') to abandon it.",
                        session.topic,
                        session.transcript.len(),
                        session.max_rounds
                    ),
                    reply_to: None,
                    attachments: vec![],
                })
                .await;
        }

        Ok(ids.len())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::artifacts::SqliteArtifactStore;
    use crate::teams::events::SqliteEventLogStore;
    use crate::teams::messages::router::EscalationRule;
    use crate::teams::messages::store::SqliteMessageStore;
    use crate::teams::sessions::store::SqliteSessionStore;
    use crate::teams::sessions::types::SessionTrigger;

    async fn make_coordinator() -> SessionCoordinator {
        let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await);
        let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await);
        let session_store = Arc::new(SqliteSessionStore::new_in_memory().await);
        let artifact_store = Arc::new(SqliteArtifactStore::new_in_memory().await);

        let router = Arc::new(MessageRouter::new(
            msg_store,
            event_store.clone(),
            EscalationRule::default(),
            None,
        ));

        SessionCoordinator::new(session_store, router, event_store, artifact_store)
    }

    #[tokio::test]
    async fn test_start_session_logs_event() {
        let coord = make_coordinator().await;

        let session = coord
            .start_session(
                NewSession {
                    team_id: "team-1".to_string(),
                    participants: vec!["agent-a".to_string(), "agent-b".to_string()],
                    topic: "Design review".to_string(),
                    trigger: SessionTrigger::Explicit {
                        requested_by: "agent-a".to_string(),
                    },
                    thread_id: None,
                    max_rounds: 10,
                },
                "agent-a",
            )
            .await
            .unwrap();

        assert_eq!(session.topic, "Design review");
        assert_eq!(session.participants.len(), 2);

        // Verify event was logged
        let events = coord
            .event_store
            .get_events("team-1", None, None)
            .await
            .unwrap();
        // At least the SessionStarted event (MessageRouter also logs MessageSent)
        let session_started = events
            .iter()
            .any(|e| e.event_type == TeamEventType::SessionStarted);
        assert!(session_started);
    }

    #[tokio::test]
    async fn test_non_participant_cannot_submit_turn_or_finalize() {
        // Regression: only declared participants may speak in or conclude a
        // session. A stranger who knows the session_id must be rejected.
        let coord = make_coordinator().await;

        let session = coord
            .start_session(
                NewSession {
                    team_id: "team-1".to_string(),
                    participants: vec!["agent-a".to_string(), "agent-b".to_string()],
                    topic: "Members only".to_string(),
                    trigger: SessionTrigger::Explicit {
                        requested_by: "agent-a".to_string(),
                    },
                    thread_id: None,
                    max_rounds: 5,
                },
                "agent-a",
            )
            .await
            .unwrap();

        let turn_err = coord
            .submit_turn(&session.id, "agent-x", "I don't belong here")
            .await;
        assert!(turn_err.is_err(), "non-participant turn must be rejected");

        let outcome = SessionOutcome {
            conclusion: "hijack".to_string(),
            agreed_by: vec![],
            dissent: None,
        };
        let conclude_err = coord.finalize(&session.id, "agent-x", outcome).await;
        assert!(
            conclude_err.is_err(),
            "non-participant conclude must be rejected"
        );

        // A genuine participant still works.
        coord
            .submit_turn(&session.id, "agent-a", "hello team")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_submit_turn_and_finalize() {
        let coord = make_coordinator().await;

        let session = coord
            .start_session(
                NewSession {
                    team_id: "team-1".to_string(),
                    participants: vec!["agent-a".to_string(), "agent-b".to_string()],
                    topic: "Quick chat".to_string(),
                    trigger: SessionTrigger::Explicit {
                        requested_by: "agent-a".to_string(),
                    },
                    thread_id: Some("task-99".to_string()),
                    max_rounds: 5,
                },
                "agent-a",
            )
            .await
            .unwrap();

        // Submit a turn
        coord
            .submit_turn(&session.id, "agent-a", "Hello there")
            .await
            .unwrap();

        // Verify the turn was recorded
        let fetched = coord
            .session_store
            .get_session(&session.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.transcript.len(), 1);

        // Finalize
        let outcome = SessionOutcome {
            conclusion: "We agreed".to_string(),
            agreed_by: vec!["agent-a".to_string(), "agent-b".to_string()],
            dissent: None,
        };
        coord
            .finalize(&session.id, "agent-a", outcome)
            .await
            .unwrap();

        let concluded = coord
            .session_store
            .get_session(&session.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            concluded.status,
            crate::teams::sessions::types::SessionStatus::Concluded
        );

        // Verify artifact was saved (thread_id was present)
        let artifacts = coord
            .artifact_store
            .get_artifacts_for_task("task-99")
            .await
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert!(artifacts[0].title.contains("Session conclusion"));
    }

    #[tokio::test]
    async fn test_cancel_session() {
        let coord = make_coordinator().await;

        let session = coord
            .start_session(
                NewSession {
                    team_id: "team-1".to_string(),
                    participants: vec!["agent-a".to_string()],
                    topic: "To be cancelled".to_string(),
                    trigger: SessionTrigger::Explicit {
                        requested_by: "agent-a".to_string(),
                    },
                    thread_id: None,
                    max_rounds: 10,
                },
                "agent-a",
            )
            .await
            .unwrap();

        // A stranger cannot cancel someone else's session.
        assert!(coord.cancel(&session.id, "agent-x").await.is_err());

        coord.cancel(&session.id, "agent-a").await.unwrap();

        let fetched = coord
            .session_store
            .get_session(&session.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fetched.status,
            crate::teams::sessions::types::SessionStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn test_sweep_deadlocked_logs_event_and_notifies() {
        let coord = make_coordinator().await;

        let session = coord
            .start_session(
                NewSession {
                    team_id: "team-1".to_string(),
                    participants: vec!["agent-a".to_string(), "agent-b".to_string()],
                    topic: "Stuck debate".to_string(),
                    trigger: SessionTrigger::Explicit {
                        requested_by: "agent-a".to_string(),
                    },
                    thread_id: None,
                    max_rounds: 1,
                },
                "agent-a",
            )
            .await
            .unwrap();

        coord
            .submit_turn(&session.id, "agent-a", "only round")
            .await
            .unwrap();

        let swept = coord.sweep_deadlocked(0, 86_400).await.unwrap();
        assert_eq!(swept, 1);

        let fetched = coord
            .session_store
            .get_session(&session.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fetched.status,
            crate::teams::sessions::types::SessionStatus::Deadlocked
        );

        // SessionDeadlocked event logged.
        let events = coord
            .event_store
            .get_events("team-1", None, None)
            .await
            .unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == TeamEventType::SessionDeadlocked));
    }

    #[tokio::test]
    async fn test_start_session_records_event_and_notification() {
        let coord = make_coordinator().await;

        let session = coord
            .start_session(
                NewSession {
                    team_id: "team-1".to_string(),
                    participants: vec!["agent-a".to_string(), "agent-b".to_string()],
                    topic: "Observability probe".to_string(),
                    trigger: SessionTrigger::Explicit {
                        requested_by: "agent-a".to_string(),
                    },
                    thread_id: None,
                    max_rounds: 5,
                },
                "agent-a",
            )
            .await
            .unwrap();

        let events = coord
            .event_store
            .get_events("team-1", None, None)
            .await
            .unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == TeamEventType::SessionStarted));
        assert_eq!(
            session.status,
            crate::teams::sessions::types::SessionStatus::Active
        );
    }
}
