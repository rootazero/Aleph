//! `SessionReadTool` — read a collaborative session's transcript, status, and outcome.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::sessions::store::SessionStore;
use crate::teams::sessions::types::{CollaborativeSession, SessionOutcome};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for reading a session.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionReadArgs {
    /// Session ID to read
    pub session_id: String,
}

/// A single turn in serializable form.
#[derive(Debug, Clone, Serialize)]
pub struct TurnEntry {
    pub agent_id: String,
    pub content: String,
    pub turn_number: u32,
    pub timestamp: String,
}

/// Output from `session_read`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionReadOutput {
    pub session_id: String,
    pub team_id: String,
    pub topic: String,
    pub status: String,
    pub participants: Vec<String>,
    pub max_rounds: u32,
    pub turns_count: usize,
    pub transcript: Vec<TurnEntry>,
    pub outcome: Option<SessionOutcome>,
    pub created_at: String,
}

impl From<CollaborativeSession> for SessionReadOutput {
    fn from(s: CollaborativeSession) -> Self {
        let transcript = s
            .transcript
            .iter()
            .map(|t| TurnEntry {
                agent_id: t.agent_id.clone(),
                content: t.content.clone(),
                turn_number: t.turn_number,
                timestamp: t.timestamp.to_rfc3339(),
            })
            .collect::<Vec<_>>();
        let turns_count = transcript.len();

        Self {
            session_id: s.id,
            team_id: s.team_id,
            topic: s.topic,
            status: s.status.as_str().to_string(),
            participants: s.participants,
            max_rounds: s.max_rounds,
            turns_count,
            transcript,
            outcome: s.outcome,
            created_at: s.created_at.to_rfc3339(),
        }
    }
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that reads a collaborative session's full state including transcript.
#[derive(Clone)]
pub struct SessionReadTool {
    store: Arc<dyn SessionStore>,
    team_store: Arc<dyn crate::teams::TeamStore>,
    current_agent_id: String,
}

impl SessionReadTool {
    pub fn new(
        store: Arc<dyn SessionStore>,
        team_store: Arc<dyn crate::teams::TeamStore>,
        current_agent_id: String,
    ) -> Self {
        Self {
            store,
            team_store,
            current_agent_id,
        }
    }

    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        crate::builtin_tools::acting_agent::acting_agent_id(&self.current_agent_id)
    }
}

#[async_trait]
impl AlephTool for SessionReadTool {
    const NAME: &'static str = "session_read";
    const DESCRIPTION: &'static str =
        "Read a collaborative session's transcript, status, and outcome";

    type Args = SessionReadArgs;
    type Output = SessionReadOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(session_id = %args.session_id, "session_read");

        let session = self
            .store
            .get_session(&args.session_id)
            .await
            .map_err(|e| AlephError::other(format!("Failed to read session: {e}")))?
            .ok_or_else(|| AlephError::other(format!("Session not found: {}", args.session_id)))?;

        // BT-D-R4-23: gate before any transcript read — sessions hold
        // multi-turn agent-to-agent content that is not the caller's.
        // Fetch the session first so we can auth against its owning team.
        let actor = self.actor();
        super::require_team_auth(&*self.team_store, &session.team_id, &actor).await?;

        Ok(SessionReadOutput::from(session))
    }
}
