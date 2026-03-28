//! SessionTurnTool — respond in a collaborative session or propose its conclusion.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

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
    /// "respond" to add a turn, "conclude" to propose conclusion
    #[serde(default = "default_respond")]
    pub mode: String,
    /// Conclusion summary (required when mode="conclude")
    pub conclusion: Option<String>,
    /// Agents who agree (required when mode="conclude")
    pub agreed_by: Option<Vec<String>>,
    /// Dissenting opinion (optional, mode="conclude")
    pub dissent: Option<String>,
}

/// Output from session_turn.
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
    pub fn new(coordinator: Arc<SessionCoordinator>, current_agent_id: String) -> Self {
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
        "Respond in a collaborative session or propose its conclusion";

    type Args = SessionTurnArgs;
    type Output = SessionTurnOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "session_turn(session_id='sess-1', content='I think we should use approach A')".to_string(),
            "session_turn(session_id='sess-1', content='Final summary', mode='conclude', conclusion='Use approach A', agreed_by=['agent-a','agent-b'])".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            session_id = %args.session_id,
            mode = %args.mode,
            agent_id = %self.current_agent_id,
            "session_turn"
        );

        match args.mode.as_str() {
            "respond" => {
                self.coordinator
                    .submit_turn(&args.session_id, &self.current_agent_id, &args.content)
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
                    .finalize(&args.session_id, outcome)
                    .await
                    .map_err(|e| AlephError::other(format!("Failed to conclude session: {e}")))?;

                Ok(SessionTurnOutput {
                    session_id: args.session_id,
                    action: "concluded".to_string(),
                    message: "Session concluded successfully".to_string(),
                })
            }
            other => Err(AlephError::other(format!(
                "Unknown mode '{other}': expected 'respond' or 'conclude'"
            ))),
        }
    }
}
