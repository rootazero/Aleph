use crate::gateway::channel::ChannelResult;
use crate::gateway::event_emitter::StreamEvent;
use crate::gateway::interfaces::telegram::config_v2::StatusReactionConfig;
use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Manages status reactions on the inbound message based on stream events.
pub struct StatusReactionController {
    delivery: TelegramDelivery,
    config: StatusReactionConfig,
    current_reaction: Arc<Mutex<Option<String>>>,
}

impl StatusReactionController {
    pub fn new(delivery: TelegramDelivery, config: StatusReactionConfig) -> Self {
        Self {
            delivery,
            config,
            current_reaction: Arc::new(Mutex::new(None)),
        }
    }

    /// Handle a stream event and update the reaction accordingly.
    pub async fn handle_event(
        &self,
        event: &StreamEvent,
        message_id: i64,
    ) -> ChannelResult<()> {
        let target_emoji = match event {
            StreamEvent::ResponseChunk { .. } => {
                self.config.processing.clone()
            }
            StreamEvent::ToolStart { .. } => {
                self.config.tool_active.clone()
            }
            StreamEvent::ToolEnd { .. } => {
                self.config.processing.clone()
            }
            StreamEvent::RunComplete { .. } => {
                self.config.complete.clone()
            }
            StreamEvent::RunError { .. } => {
                Some("👎".to_string())
            }
            _ => None,
        };

        if let Some(emoji) = target_emoji {
            let mut current = self.current_reaction.lock().await;
            if current.as_ref() != Some(&emoji) {
                self.delivery.set_reaction(message_id, &emoji).await?;
                *current = Some(emoji);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::telegram::config_resolver::ResolvedConfig;
    use crate::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;

    #[tokio::test]
    async fn test_reaction_state_transitions() {
        let delivery = TelegramDelivery::new(
            teloxide::Bot::new("test"),
            ResolvedConfig {
                account_id: "test".to_string(),
                bot_token: "test".to_string(),
                bot_username: None,
                default_agent: None,
                dm_policy: Default::default(),
                group_policy: Default::default(),
                send_typing: false,
                allowed_users: vec![],
                allowed_groups: vec![],
                streaming: Default::default(),
                error_policy: Default::default(),
                max_retries: 0,
            },
            Arc::new(ErrorCooldown::new()),
            "123",
        );
        let config = StatusReactionConfig {
            processing: Some("👀".to_string()),
            tool_active: Some("🔧".to_string()),
            complete: Some("👍".to_string()),
        };
        let controller = StatusReactionController::new(delivery, config);

        controller
            .handle_event(
                &StreamEvent::ResponseChunk {
                    run_id: "r1".to_string(),
                    seq: 1,
                    delta: "hi".to_string(),
                    full_text: "hi".to_string(),
                    content: "hi".to_string(),
                    chunk_index: 0,
                    is_final: false,
                    is_intermediate: false,
                },
                100,
            )
            .await
            .unwrap();

        assert_eq!(
            *controller.current_reaction.lock().await,
            Some("👀".to_string())
        );
    }
}
