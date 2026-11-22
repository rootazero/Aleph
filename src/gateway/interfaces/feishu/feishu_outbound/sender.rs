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
            api.send_text(chat_id, &message.text, reply_to)
                .await
                .map_err(|error| {
                    // The image is already delivered: never expose a whole-message retry.
                    ChannelError::Internal(format!(
                        "Partial delivery: image {msg_id} sent, caption failed: {error:?}; do not resend the image"
                    ))
                })?;
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

    #[tokio::test]
    async fn caption_failure_reports_sent_image_without_retrying_it() {
        use super::*;
        use crate::gateway::interfaces::feishu::auth::TokenManager;
        use wiremock::{
            matchers::{body_partial_json, method, path},
            Mock, MockServer, ResponseTemplate,
        };

        for status in [429, 403] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/open-apis/auth/v3/app_access_token/internal"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "code": 0, "msg": "ok", "app_access_token": "test-token", "expire": 7200
                })))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/open-apis/im/v1/images"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "code": 0, "msg": "ok", "data": {"image_key": "image-key"}
                })))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/open-apis/im/v1/messages"))
                .and(body_partial_json(serde_json::json!({"msg_type": "image"})))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "code": 0, "msg": "ok", "data": {"message_id": "sent-image-id"}
                })))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/open-apis/im/v1/messages"))
                .and(body_partial_json(serde_json::json!({"msg_type": "text"})))
                .respond_with(
                    ResponseTemplate::new(status).set_body_json(serde_json::json!({
                        "code": 999, "msg": "caption denied"
                    })),
                )
                .expect(1)
                .mount(&server)
                .await;
            let http = reqwest::Client::new();
            let auth = Arc::new(TokenManager::new(
                "app",
                "secret",
                &server.uri(),
                http.clone(),
            ));
            let api = Arc::new(FeishuApi::new(auth, &server.uri(), http));
            let mut message = OutboundMessage::text("chat", "caption");
            message
                .attachments
                .push(crate::gateway::channel::Attachment {
                    id: "image".into(),
                    mime_type: "image/png".into(),
                    filename: None,
                    size: None,
                    url: None,
                    path: None,
                    data: Some(vec![1]),
                    is_voice_note: false,
                    is_video_note: false,
                });
            let config: FeishuConfig = serde_json::from_value(serde_json::json!({
                "app_id": "app", "app_secret": "secret"
            }))
            .unwrap();
            let error = FeishuSender::send_message(&api, message, &config)
                .await
                .unwrap_err();
            assert!(
                matches!(&error, ChannelError::Internal(_)),
                "must not retry a delivered image: {error}"
            );
            let detail = error.to_string();
            assert!(detail.contains("sent-image-id"));
            assert!(detail.contains("caption"));
            server.verify().await;
        }
    }

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
