//! `SessionCollaborateTool` — start a collaborative session between team members.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::sessions::coordinator::SessionCoordinator;
use crate::teams::sessions::types::{NewSession, SessionTrigger};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

const fn default_max_rounds() -> u32 {
    10
}

/// Arguments for starting a collaborative session.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionCollaborateArgs {
    /// Team that owns this session
    pub team_id: String,
    /// Agent IDs that should participate
    pub participants: Vec<String>,
    /// Discussion topic
    pub topic: String,
    /// Maximum number of turns before the session must conclude
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
    /// Optional thread to link the session to
    pub thread_id: Option<String>,
}

/// Output from `session_collaborate`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionCollaborateOutput {
    pub session_id: String,
    pub topic: String,
    pub participants: Vec<String>,
    pub max_rounds: u32,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that starts a collaborative session for real-time multi-turn discussion
/// between team members.
#[derive(Clone)]
pub struct SessionCollaborateTool {
    coordinator: Arc<SessionCoordinator>,
    current_agent_id: String,
}

impl SessionCollaborateTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    #[must_use]
    pub const fn new(coordinator: Arc<SessionCoordinator>, current_agent_id: String) -> Self {
        Self {
            coordinator,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for SessionCollaborateTool {
    const NAME: &'static str = "session_collaborate";
    const DESCRIPTION: &'static str =
        "Start a collaborative session between team members for real-time multi-turn discussion";

    type Args = SessionCollaborateArgs;
    type Output = SessionCollaborateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            team_id = %args.team_id,
            topic = %args.topic,
            participants = ?args.participants,
            "session_collaborate: starting session"
        );

        let input = NewSession {
            team_id: args.team_id,
            participants: args.participants.clone(),
            topic: args.topic.clone(),
            trigger: SessionTrigger::Explicit {
                requested_by: self.actor(),
            },
            thread_id: args.thread_id,
            max_rounds: args.max_rounds,
        };

        let session = self
            .coordinator
            .start_session(input, &self.actor())
            .await
            .map_err(|e| AlephError::other(format!("Failed to start session: {e}")))?;

        Ok(SessionCollaborateOutput {
            session_id: session.id,
            topic: session.topic,
            participants: session.participants,
            max_rounds: session.max_rounds,
            message: format!(
                "Collaborative session started on topic '{}' with {} participants",
                args.topic,
                args.participants.len()
            ),
        })
    }
}
