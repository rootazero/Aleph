use crate::gateway::channel::{ChannelError, ChannelResult};
use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
use crate::sync_primitives::Arc;
use tokio::sync::Mutex;

use super::lane_tracker::{LaneDeliveryTracker, LaneId};

/// Handle to a single lane for writing stream chunks.
#[derive(Clone)]
pub struct LaneHandle {
    lane_id: LaneId,
    tracker: Arc<Mutex<LaneDeliveryTracker>>,
    delivery: TelegramDelivery,
}

impl LaneHandle {
    pub const fn new(
        lane_id: LaneId,
        tracker: Arc<Mutex<LaneDeliveryTracker>>,
        delivery: TelegramDelivery,
    ) -> Self {
        Self {
            lane_id,
            tracker,
            delivery,
        }
    }

    /// Append a text chunk to this lane.
    /// If no preview message exists yet, sends a new message.
    /// Otherwise edits the existing preview message.
    pub async fn write_chunk(&self, text: &str) -> ChannelResult<()> {
        let mut tracker = self.tracker.lock().await;
        let state = tracker
            .get_mut(self.lane_id)
            .ok_or_else(|| ChannelError::Internal("lane not initialized in tracker".to_string()))?;

        let message_id = if let Some(id) = state.preview_message_id {
            id
        } else {
            let id = self.delivery.send_text_message(text).await?;
            state.preview_message_id = Some(id);
            state.is_streaming = true;
            return Ok(());
        };

        drop(tracker);
        self.delivery.edit_text_message(message_id, text).await?;
        Ok(())
    }

    /// Finalize the lane with final text.
    /// Returns the Telegram message ID of the finalized message.
    pub async fn finalize(&self, final_text: &str) -> ChannelResult<i64> {
        let mut tracker = self.tracker.lock().await;
        let state = tracker
            .get_mut(self.lane_id)
            .ok_or_else(|| ChannelError::Internal("lane not initialized in tracker".to_string()))?;

        let message_id = if let Some(id) = state.preview_message_id {
            id
        } else {
            let id = self.delivery.send_text_message(final_text).await?;
            state.final_message_id = Some(id);
            state.is_streaming = false;
            return Ok(id);
        };

        drop(tracker);
        self.delivery
            .edit_text_message(message_id, final_text)
            .await?;

        let mut tracker = self.tracker.lock().await;
        let state = tracker
            .get_mut(self.lane_id)
            .ok_or_else(|| ChannelError::Internal("lane not initialized in tracker".to_string()))?;
        state.final_message_id = Some(message_id);
        state.is_streaming = false;
        Ok(message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
    use crate::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;
    use crate::sync_primitives::Arc;

    #[tokio::test]
    async fn test_lane_handle_creation() {
        let tracker = Arc::new(Mutex::new(LaneDeliveryTracker::new(123, None)));
        let delivery = TelegramDelivery::new(
            teloxide::Bot::new("test"),
            crate::gateway::interfaces::telegram::config_resolver::ResolvedConfig {
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
                html_fallback: true,
                link_preview:
                    crate::gateway::interfaces::telegram::config_v2::LinkPreviewMode::Enabled,
            },
            Arc::new(ErrorCooldown::new()),
            "123",
        );
        let handle = LaneHandle::new(LaneId::Answer, tracker, delivery);
        assert!(matches!(handle.lane_id, LaneId::Answer));
    }
}
