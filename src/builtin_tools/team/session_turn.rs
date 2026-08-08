//! `SessionTurnTool` — respond in a collaborative session or propose its conclusion.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::sessions::coordinator::SessionCoordinator;
use crate::teams::sessions::types::SessionOutcome;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

fn default_respond() -> String {
    "respond".to_string()
}

/// Arguments for submitting a turn or concluding a session.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionTurnArgs {
    /// Session to act on
    pub session_id: String,
    /// Message content for this turn
    pub content: String,
    /// "respond" to add a turn, "conclude" to propose conclusion,
    /// "cancel" to abandon the session without an outcome
    #[serde(default = "default_respond")]
    pub mode: String,
    /// Conclusion summary (required when mode="conclude")
    pub conclusion: Option<String>,
    /// Agents who agree (required when mode="conclude")
    pub agreed_by: Option<Vec<String>>,
    /// Dissenting opinion (optional, mode="conclude")
    pub dissent: Option<String>,
}

/// Output from `session_turn`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionTurnOutput {
    pub session_id: String,
    pub action: String,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that responds in a collaborative session or proposes its conclusion.
#[derive(Clone)]
pub struct SessionTurnTool {
    coordinator: Arc<SessionCoordinator>,
    current_agent_id: String,
}

impl SessionTurnTool {
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
impl AlephTool for SessionTurnTool {
    const NAME: &'static str = "session_turn";
    const DESCRIPTION: &'static str =
        "Respond in a collaborative session, propose its conclusion, or cancel it";

    type Args = SessionTurnArgs;
    type Output = SessionTurnOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            session_id = %args.session_id,
            mode = %args.mode,
            agent_id = %self.actor(),
            "session_turn"
        );

        match args.mode.as_str() {
            "respond" => {
                self.coordinator
                    .submit_turn(&args.session_id, &self.actor(), &args.content)
                    .await
                    .map_err(|e| AlephError::other(format!("Failed to submit turn: {e}")))?;

                Ok(SessionTurnOutput {
                    session_id: args.session_id,
                    action: "responded".to_string(),
                    message: "Turn submitted successfully".to_string(),
                })
            }
            "conclude" => {
                let conclusion = args.conclusion.unwrap_or(args.content);
                let agreed_by = args.agreed_by.unwrap_or_default();

                let outcome = SessionOutcome {
                    conclusion,
                    agreed_by,
                    dissent: args.dissent,
                };

                self.coordinator
                    .finalize(&args.session_id, &self.actor(), outcome)
                    .await
                    .map_err(|e| AlephError::other(format!("Failed to conclude session: {e}")))?;

                Ok(SessionTurnOutput {
                    session_id: args.session_id,
                    action: "concluded".to_string(),
                    message: "Session concluded successfully".to_string(),
                })
            }
            "cancel" => {
                self.coordinator
                    .cancel(&args.session_id, &self.actor())
                    .await
                    .map_err(|e| AlephError::other(format!("Failed to cancel session: {e}")))?;

                Ok(SessionTurnOutput {
                    session_id: args.session_id,
                    action: "cancelled".to_string(),
                    message: "Session cancelled".to_string(),
                })
            }
            other => Err(AlephError::other(format!(
                "Unknown mode '{other}': expected 'respond', 'conclude' or 'cancel'"
            ))),
        }
    }
}
