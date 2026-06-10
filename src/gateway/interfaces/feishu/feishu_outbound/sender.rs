use crate::sync_primitives::Arc;

use chrono::Utc;

use crate::gateway::channel::{
    ChannelError, ChannelResult, MessageId, OutboundMessage, SendResult,
};
use crate::gateway::interfaces::feishu::api::{FeishuApi, FeishuSendError};
use crate::gateway::interfaces::feishu::config::FeishuConfig;
use crate::gateway::interfaces::feishu::feishu_outbound::media::MediaHelper;

#[must_use]
pub fn should_use_card(text: &str, render_mode: &str) -> bool {
    match render_mode {
        "card" => true,
        "raw" => false,
        _ => {
            text.len() > 200
                || text.contains("```")
                || text.contains("|---|")
                || text.contains("|:--")
        }
    }
}

fn map_send_error(e: FeishuSendError) -> ChannelError {
    match e {
        FeishuSendError::RateLimited { retry_after_secs } => {
            ChannelError::RateLimited { retry_after_secs }
        }
        FeishuSendError::Other(msg) => ChannelError::SendFailed(msg),
    }
}

pub struct FeishuSender;

impl FeishuSender {
    pub async fn send_message(
        api: &Arc<FeishuApi>,
        message: OutboundMessage,
        config: &FeishuConfig,
    ) -> ChannelResult<SendResult> {
        let chat_id = message.conversation_id.as_str();
        let reply_to = message.reply_to.as_ref().map(|id| id.as_str());

        let has_image = message
            .attachments
            .iter()
            .any(|a| a.mime_type.starts_with("image/"));

        let msg_id = if has_image {
            if let Some(attachment) = message
                .attachments
                .iter()
                .find(|a| a.mime_type.starts_with("image/"))
            {
                let image_data = attachment.data.clone().ok_or_else(|| {
                    ChannelError::SendFailed("Image attachment has no data".to_string())
                })?;
                let filename = attachment.filename.as_deref().unwrap_or("image.png");
                let image_key = MediaHelper::upload_image(api, image_data, filename).await?;
                api.send_image(chat_id, &image_key, reply_to)
                    .await
                    .map_err(map_send_error)?
            } else {
                return Err(ChannelError::SendFailed(
                    "Image attachment not found".to_string(),
                ));
            }
        } else {
            if message.text.is_empty() {
                return Err(ChannelError::SendFailed("Empty message".to_string()));
            }
            if should_use_card(&message.text, &config.render_mode) {
                api.send_card(chat_id, &message.text, reply_to)
                    .await
                    .map_err(map_send_error)?
            } else {
                api.send_text(chat_id, &message.text, reply_to)
                    .await
                    .map_err(map_send_error)?
            }
        };

        if has_image && !message.text.is_empty() {
            let _ = api.send_text(chat_id, &message.text, reply_to).await;
        }

        Ok(SendResult {
            message_id: MessageId::new(msg_id),
            timestamp: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::should_use_card;

    #[test]
    fn test_should_use_card_auto_plain() {
        assert!(!should_use_card("Hello world", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_code_block() {
        assert!(should_use_card("```rust\nfn main() {}\n```", "auto"));
    }

    #[test]
    fn test_should_use_card_forced() {
        assert!(should_use_card("Hi", "card"));
    }
}
