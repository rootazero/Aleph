//! ReviewScoreTool — submit a structured review score for a task artifact.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact};
use crate::teams::events::{EventLogStore, NewTeamEvent, TeamEventType};
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::teams::roles::review::{Challenge, DimensionScore, ReviewScore};
use crate::teams::roles::types::TeamRoleConfig;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for submitting a structured review score.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ReviewScoreArgs {
    /// The team this review belongs to.
    pub team_id: String,
    /// The task being reviewed.
    pub task_id: String,
    /// The artifact being reviewed.
    pub artifact_id: String,
    /// Per-dimension scores (1-10).
    pub scores: Vec<DimensionScore>,
    /// Whether the artifact passes overall.
    pub overall_pass: bool,
    /// Challenges raised during review.
    pub challenges: Vec<Challenge>,
    /// Suggestions for improvement.
    #[serde(default)]
    pub improvement_suggestions: Vec<String>,
    /// Risks if the artifact is accepted as-is.
    #[serde(default)]
    pub risks_if_accepted: Vec<String>,
}

/// Output from review_score.
#[derive(Debug, Serialize)]
pub struct ReviewScoreOutput {
    /// Whether the review was accepted (passed validation).
    pub accepted: bool,
    /// Validation errors (empty when accepted).
    pub validation_errors: Vec<String>,
    /// ID of the stored review artifact.
    pub review_id: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that submits a structured review score for a task artifact
/// with configurable validation thresholds.
#[derive(Clone)]
pub struct ReviewScoreTool {
    artifact_store: Arc<dyn ArtifactStore>,
    event_store: Arc<dyn EventLogStore>,
    message_router: Arc<MessageRouter>,
    current_agent_id: String,
}

impl ReviewScoreTool {
    pub fn new(
        artifact_store: Arc<dyn ArtifactStore>,
        event_store: Arc<dyn EventLogStore>,
        message_router: Arc<MessageRouter>,
        current_agent_id: String,
    ) -> Self {
        Self {
            artifact_store,
            event_store,
            message_router,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for ReviewScoreTool {
    const NAME: &'static str = "review_score";
    const DESCRIPTION: &'static str =
        "Submit a structured review score for a task artifact with configurable validation thresholds";

    type Args = ReviewScoreArgs;
    type Output = ReviewScoreOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "review_score(team_id='team-1', task_id='task-1', artifact_id='art-1', scores=[{dimension:'correctness', score:8, rationale:'Good'}], overall_pass=true, challenges=[{point:'Edge case', severity:'minor', evidence:'Line 42'}])".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            team_id = %args.team_id,
            task_id = %args.task_id,
            artifact_id = %args.artifact_id,
            overall_pass = %args.overall_pass,
            agent_id = %self.current_agent_id,
            "review_score: submitting review"
        );

        // Build ReviewScore from args
        let review = ReviewScore {
            task_id: args.task_id.clone(),
            artifact_id: args.artifact_id.clone(),
            scores: args.scores.clone(),
            overall_pass: args.overall_pass,
            challenges: args.challenges.clone(),
            improvement_suggestions: args.improvement_suggestions.clone(),
            risks_if_accepted: args.risks_if_accepted.clone(),
        };

        // Use default TeamRoleConfig (callers can customize thresholds later)
        let config = TeamRoleConfig::default();

        // Validate against thresholds
        if let Err(validation_errors) = review.validate(&config) {
            return Ok(ReviewScoreOutput {
                accepted: false,
                validation_errors,
                review_id: String::new(),
            });
        }

        // Save as TaskArtifact (type: Review)
        let review_content = serde_json::to_string_pretty(&review)
            .unwrap_or_else(|_| "{}".to_string());

        let artifact = self
            .artifact_store
            .create_artifact(NewArtifact {
                task_id: args.task_id.clone(),
                agent_id: self.current_agent_id.clone(),
                artifact_type: ArtifactType::Review,
                title: format!(
                    "Review of artifact {} — {}",
                    args.artifact_id,
                    if args.overall_pass { "PASS" } else { "FAIL" }
                ),
                content: review_content,
                metadata: serde_json::json!({
                    "artifact_id": args.artifact_id,
                    "overall_pass": args.overall_pass,
                    "score_count": args.scores.len(),
                    "challenge_count": args.challenges.len(),
                }),
            })
            .await
            .map_err(|e| AlephError::other(format!("Failed to save review artifact: {e}")))?;

        // Log ReviewScoreSubmitted event
        let _ = self
            .event_store
            .log_event(NewTeamEvent {
                team_id: args.team_id.clone(),
                event_type: TeamEventType::ReviewScoreSubmitted,
                agent_id: self.current_agent_id.clone(),
                payload: serde_json::json!({
                    "review_id": artifact.id,
                    "task_id": args.task_id,
                    "artifact_id": args.artifact_id,
                    "overall_pass": args.overall_pass,
                }),
            })
            .await;

        // Send ReviewResult message to the team via MessageRouter
        let summary = if args.overall_pass {
            format!("Review PASSED for artifact {}", args.artifact_id)
        } else {
            format!(
                "Review FAILED for artifact {} — {} challenges raised",
                args.artifact_id,
                args.challenges.len()
            )
        };

        let _ = self
            .message_router
            .send(SendRequest {
                team_id: args.team_id,
                from_agent: self.current_agent_id.clone(),
                to: vec![], // broadcast — no specific target
                cc: vec![],
                msg_type: MessageType::ReviewResult,
                subject: format!("Review: {}", args.task_id),
                content: summary,
                reply_to: None,
                attachments: vec![artifact.id.clone()],
            })
            .await;

        Ok(ReviewScoreOutput {
            accepted: true,
            validation_errors: vec![],
            review_id: artifact.id,
        })
    }
}
