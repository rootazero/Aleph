//! EventHandler implementation and event-to-part conversion for SessionRecorder.

use async_trait::async_trait;

use crate::event::{
    AiResponse, AlephEvent, EventContext, EventHandler, EventType, HandlerError, InputEvent,
    TaskPlan, ToolCallError, ToolCallResult,
};

use crate::components::{
    AiResponsePart, PlanPart, PlanStep, SessionPart, StepStatus, ToolCallPart, ToolCallStatus,
    UserInputPart,
};

use super::SessionRecorder;

// ============================================================================
// EventHandler Implementation
// ============================================================================

#[async_trait]
impl EventHandler for SessionRecorder {
    fn name(&self) -> &'static str {
        "SessionRecorder"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::All]
    }

    async fn handle(
        &self,
        event: &AlephEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Get session ID from context
        let session_id = ctx.get_session_id().await;

        // Handle special session events
        match event {
            AlephEvent::SessionCreated(info) => {
                // Create new session record
                if let Err(e) = self.create_session_with_options(
                    &info.id,
                    &info.model,
                    info.parent_id.as_deref(),
                    &info.agent_id,
                ) {
                    tracing::error!(error = %e, "Failed to create session record");
                }
                return Ok(vec![]);
            }
            AlephEvent::LoopContinue(_) => {
                // Update session iteration count
                if let Some(ref sid) = session_id {
                    if let Err(e) = self.update_session(sid) {
                        tracing::error!(error = %e, "Failed to update session");
                    }
                }
                return Ok(vec![]);
            }
            AlephEvent::SessionUpdated(diff) => {
                // Apply session diff
                if let Err(e) = self.update_session_full(
                    &diff.session_id,
                    diff.status.as_deref(),
                    diff.iteration_count,
                    diff.total_tokens,
                ) {
                    tracing::error!(error = %e, "Failed to apply session diff");
                }
                return Ok(vec![]);
            }
            _ => {}
        }

        // Convert event to session part
        if let Some(part) = Self::event_to_part(event) {
            if let Some(ref sid) = session_id {
                if let Err(e) = self.append_part(sid, &part) {
                    tracing::error!(
                        error = %e,
                        event_type = ?event.event_type(),
                        "Failed to persist session part"
                    );
                }
            }
        }

        // SessionRecorder doesn't publish any events
        Ok(vec![])
    }
}

// ============================================================================
// Event Conversion Methods
// ============================================================================

impl SessionRecorder {
    /// Convert an AlephEvent to a SessionPart
    ///
    /// Returns None for events that don't map to session parts.
    pub fn event_to_part(event: &AlephEvent) -> Option<SessionPart> {
        match event {
            AlephEvent::InputReceived(input) => Some(Self::input_to_part(input)),
            AlephEvent::ToolCallCompleted(result) => Some(Self::tool_result_to_part(result)),
            AlephEvent::ToolCallFailed(error) => Some(Self::tool_error_to_part(error)),
            AlephEvent::AiResponseGenerated(response) => Some(Self::ai_response_to_part(response)),
            AlephEvent::PlanCreated(plan) => Some(Self::plan_to_part(plan)),
            // Events that don't map to session parts
            AlephEvent::PlanRequested(_)
            | AlephEvent::ToolCallRequested(_)
            | AlephEvent::ToolCallStarted(_)
            | AlephEvent::ToolCallRetrying(_)
            | AlephEvent::LoopContinue(_)
            | AlephEvent::LoopStop(_)
            | AlephEvent::SessionCreated(_)
            | AlephEvent::SessionUpdated(_)
            | AlephEvent::SessionResumed(_)
            | AlephEvent::SessionCompacted(_)
            | AlephEvent::SubAgentStarted(_)
            | AlephEvent::SubAgentCompleted(_)
            | AlephEvent::UserQuestionAsked(_)
            | AlephEvent::UserResponseReceived(_)
            // Permission system events (handled separately)
            | AlephEvent::PermissionAsked(_)
            | AlephEvent::PermissionReplied { .. }
            // Question system events (handled separately)
            | AlephEvent::QuestionAsked(_)
            | AlephEvent::QuestionReplied { .. }
            | AlephEvent::QuestionRejected { .. }
            // Part update events are meta-events, not session parts themselves
            | AlephEvent::PartAdded(_)
            | AlephEvent::PartUpdated(_)
            | AlephEvent::PartRemoved(_)
            // Team events don't map to session parts
            | AlephEvent::TeamCreated { .. }
            | AlephEvent::TeamMemberAdded { .. }
            | AlephEvent::TeamMemberRemoved { .. }
            | AlephEvent::TeamTaskAssigned { .. }
            | AlephEvent::TeamTaskUpdated { .. }
            | AlephEvent::TeamTaskCompleted { .. }
            | AlephEvent::TeamDisbanded { .. }
            | AlephEvent::TeamMessageSent(_) => None,
        }
    }

    /// Convert InputEvent to UserInputPart
    fn input_to_part(input: &InputEvent) -> SessionPart {
        SessionPart::UserInput(UserInputPart {
            text: input.text.clone(),
            context: input
                .context
                .as_ref()
                .map(|ctx| serde_json::to_string(ctx).unwrap_or_default()),
            timestamp: input.timestamp,
        })
    }

    /// Convert ToolCallResult to ToolCallPart
    fn tool_result_to_part(result: &ToolCallResult) -> SessionPart {
        SessionPart::ToolCall(ToolCallPart {
            id: result.call_id.clone(),
            tool_name: result.tool.clone(),
            input: result.input.clone(),
            status: ToolCallStatus::Completed,
            output: Some(result.output.clone()),
            error: None,
            started_at: result.started_at,
            completed_at: Some(result.completed_at),
        })
    }

    /// Convert ToolCallError to ToolCallPart
    fn tool_error_to_part(error: &ToolCallError) -> SessionPart {
        SessionPart::ToolCall(ToolCallPart {
            id: error.call_id.clone(),
            tool_name: error.tool.clone(),
            input: serde_json::Value::Null,
            status: ToolCallStatus::Failed,
            output: None,
            error: Some(error.error.clone()),
            started_at: chrono::Utc::now().timestamp(),
            completed_at: Some(chrono::Utc::now().timestamp()),
        })
    }

    /// Convert AiResponse to AiResponsePart
    fn ai_response_to_part(response: &AiResponse) -> SessionPart {
        SessionPart::AiResponse(AiResponsePart {
            content: response.content.clone(),
            reasoning: response.reasoning.clone(),
            timestamp: response.timestamp,
        })
    }

    /// Convert TaskPlan to PlanPart
    fn plan_to_part(plan: &TaskPlan) -> SessionPart {
        SessionPart::PlanCreated(PlanPart {
            plan_id: plan.id.clone(),
            steps: plan
                .steps
                .iter()
                .map(|s| PlanStep {
                    step_id: s.id.clone(),
                    description: s.description.clone(),
                    status: StepStatus::Pending,
                    dependencies: s.depends_on.clone(),
                })
                .collect(),
            requires_confirmation: false, // Default to false for now
            created_at: chrono::Utc::now().timestamp_millis(),
        })
    }
}
