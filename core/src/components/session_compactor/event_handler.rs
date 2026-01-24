//! EventHandler trait implementation for SessionCompactor

use async_trait::async_trait;

use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError,
};

use super::compactor::SessionCompactor;

#[async_trait]
impl EventHandler for SessionCompactor {
    fn name(&self) -> &'static str {
        "SessionCompactor"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::ToolCallCompleted, EventType::LoopContinue]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Check if auto-compaction is enabled
        if !self.config().auto_compact {
            return Ok(vec![]);
        }

        match event {
            AetherEvent::LoopContinue(loop_state) => {
                // Check if we need compaction based on token count
                let limit = self.token_tracker().get_model_limit(&loop_state.model);

                if loop_state.total_tokens >= limit.compaction_threshold() {
                    // Log that compaction would be needed
                    // (Full implementation would get session from context and compact)
                    tracing::info!(
                        "Session {} would need compaction: {} tokens exceeds threshold {}",
                        loop_state.session_id,
                        loop_state.total_tokens,
                        limit.compaction_threshold()
                    );
                    // Return a placeholder - in full impl, would return SessionCompacted event
                    // Example (pseudo-code):
                    // let session = ctx.get_session(&loop_state.session_id).await;
                    // if let Some(compaction_info) = self.check_and_compact(&mut session).await {
                    //     return Ok(vec![AetherEvent::SessionCompacted(compaction_info)]);
                    // }
                }
                Ok(vec![])
            }
            AetherEvent::ToolCallCompleted(result) => {
                // Log pruning trigger
                if self.config().prune_enabled {
                    tracing::debug!(
                        "Tool {} completed, pruning check would trigger",
                        result.tool
                    );
                }
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
}
