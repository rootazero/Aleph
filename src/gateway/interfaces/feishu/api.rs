use crate::sync_primitives::Arc;
use reqwest::multipart;

use super::auth::TokenManager;
use super::types::{
    BotInfo, BotInfoResponse, CardCreateResponse, ReactionResponse, SendMessageResponse,
    UploadImageResponse, UserInfoResponse, WsEndpointResponse,
};

// Hard cap with no marker: Feishu length-checks this field, so the budget is
// the provider's, not ours to spend on an ellipsis.
use crate::utils::text_format::truncate_chars;

#[derive(Debug)]
pub enum FeishuSendError {
    RateLimited { retry_after_secs: u64 },
    Other(String),
}

impl From<String> for FeishuSendError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

/// Feishu HTTP API client. Wraps a `TokenManager` for auth.
pub struct FeishuApi {
    auth: Arc<TokenManager>,
    base_url: String,
    http: reqwest::Client,
    bot_open_id: tokio::sync::RwLock<Option<String>>,
}

impl FeishuApi {
    pub fn new(auth: Arc<TokenManager>, base_url: &str, http: reqwest::Client) -> Self {
        Self {
            auth,
            base_url: base_url.to_string(),
            http,
            bot_open_id: tokio::sync::RwLock::new(None),
        }
    }

    pub async fn bot_open_id(&self) -> Option<String> {
        self.bot_open_id.read().await.clone()
    }

    // ── Bot Info ──

    pub async fn get_bot_info(&self) -> Result<BotInfo, String> {
        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/bot/v3/info", self.base_url);

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Bot info request failed: {e}"))?;

        let info: BotInfoResponse = resp
            .json()
            .await
            .map_err(|e| format!("Bot info parse failed: {e}"))?;

        if info.code != 0 {
            return Err(format!(
                "Bot info error: code={}, msg={}",
                info.code,
                info.msg.unwrap_or_default()
            ));
        }

        let bot = info
            .bot
            .ok_or_else(|| "No bot info in response".to_string())?;

        if let Some(ref oid) = bot.open_id {
            *self.bot_open_id.write().await = Some(oid.clone());
        }

        Ok(bot)
    }

    // ── WebSocket Endpoint ──

    pub async fn get_ws_endpoint(&self) -> Result<String, String> {
        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/callback/ws/endpoint", self.base_url);

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| format!("WS endpoint request failed: {e}"))?;

        let ws_resp: WsEndpointResponse = resp
            .json()
            .await
            .map_err(|e| format!("WS endpoint parse failed: {e}"))?;

        if ws_resp.code != 0 {
            return Err(format!(
                "WS endpoint error: code={}, msg={}",
                ws_resp.code, ws_resp.msg
            ));
        }

        let data = ws_resp
            .data
            .ok_or_else(|| "No data in WS endpoint response".to_string())?;
        Ok(data.url)
    }

    // ── Send Messages ──

    pub async fn send_text(
        &self,
        chat_id: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<String, FeishuSendError> {
        if let Some(msg_id) = reply_to {
            return self
                .reply_message(
                    msg_id,
                    "text",
                    &serde_json::json!({"text": text}).to_string(),
                )
                .await;
        }

        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
            self.base_url
        );

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "text",
            "content": serde_json::json!({"text": text}).to_string(),
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send text failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    pub async fn send_image(
        &self,
        chat_id: &str,
        image_key: &str,
        reply_to: Option<&str>,
    ) -> Result<String, FeishuSendError> {
        if let Some(msg_id) = reply_to {
            return self
                .reply_message(
                    msg_id,
                    "image",
                    &serde_json::json!({"image_key": image_key}).to_string(),
                )
                .await;
        }

        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
            self.base_url
        );

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "image",
            "content": serde_json::json!({"image_key": image_key}).to_string(),
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send image failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    async fn reply_message(
        &self,
        message_id: &str,
        msg_type: &str,
        content: &str,
    ) -> Result<String, FeishuSendError> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reply",
            self.base_url, message_id
        );

        let body = serde_json::json!({
            "msg_type": msg_type,
            "content": content,
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Reply message failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    async fn parse_send_response(
        &self,
        resp: reqwest::Response,
    ) -> Result<String, FeishuSendError> {
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5);
            return Err(FeishuSendError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        let send_resp: SendMessageResponse = resp
            .json()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send response parse failed: {e}")))?;

        if send_resp.code != 0 {
            return Err(FeishuSendError::Other(format!(
                "Send error: code={}, msg={}",
                send_resp.code, send_resp.msg
            )));
        }

        let msg_id = send_resp
            .data
            .and_then(|d| d.message_id)
            .unwrap_or_default();

        Ok(msg_id)
    }

    // ── Media ──

    pub async fn upload_image(&self, data: Vec<u8>, filename: &str) -> Result<String, String> {
        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/im/v1/images", self.base_url);

        let part = multipart::Part::bytes(data)
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")
            .map_err(|e| format!("Multipart error: {e}"))?;

        let form = multipart::Form::new()
            .text("image_type", "message")
            .part("image", part);

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Upload image failed: {e}"))?;

        let upload_resp: UploadImageResponse = resp
            .json()
            .await
            .map_err(|e| format!("Upload response parse failed: {e}"))?;

        if upload_resp.code != 0 {
            return Err(format!(
                "Upload error: code={}, msg={}",
                upload_resp.code, upload_resp.msg
            ));
        }

        upload_resp
            .data
            .and_then(|d| d.image_key)
            .ok_or_else(|| "No image_key in upload response".to_string())
    }

    // ── Card Kit (Streaming Cards) ──

    /// Create a streaming card and return the `card_id`.
    pub async fn create_streaming_card(&self, initial_text: &str) -> Result<String, String> {
        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/cardkit/v1/cards", self.base_url);

        let card_body = serde_json::json!({
            "schema": "2.0",
            "config": {
                "streaming_mode": true,
                "summary": { "content": "[Generating...]" },
                "streaming_config": {
                    "print_frequency_ms": { "default": 50 },
                    "print_step": { "default": 1 }
                }
            },
            "body": {
                "elements": [
                    { "tag": "markdown", "content": initial_text, "element_id": "content" }
                ]
            }
        });

        let body = serde_json::json!({
            "type": "card_json",
            "data": card_body.to_string(),
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Create streaming card failed: {e}"))?;

        let card_resp: CardCreateResponse = resp
            .json()
            .await
            .map_err(|e| format!("Card create response parse failed: {e}"))?;

        if card_resp.code != 0 {
            return Err(format!(
                "Card create error: code={}, msg={}",
                card_resp.code, card_resp.msg
            ));
        }

        card_resp
            .data
            .and_then(|d| d.card_id)
            .ok_or_else(|| "No card_id in response".to_string())
    }

    /// Send a card message (streaming or static) to a chat.
    pub async fn send_card_message(
        &self,
        chat_id: &str,
        card_id: &str,
        reply_to: Option<&str>,
    ) -> Result<String, FeishuSendError> {
        let content = serde_json::json!({
            "type": "card",
            "data": { "card_id": card_id }
        });

        if let Some(msg_id) = reply_to {
            return self
                .reply_message(msg_id, "interactive", &content.to_string())
                .await;
        }

        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
            self.base_url
        );

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "interactive",
            "content": content.to_string(),
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send card message failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    /// Update a streaming card's content element.
    pub async fn update_streaming_card(
        &self,
        card_id: &str,
        content: &str,
        sequence: u32,
    ) -> Result<(), String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/cardkit/v1/cards/{}/elements/content/content",
            self.base_url, card_id
        );

        let body = serde_json::json!({
            "content": content,
            "sequence": sequence,
            "uuid": format!("s_{}_{}", card_id, sequence),
        });

        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Update streaming card failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Update streaming card HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// Close a streaming card (disable `streaming_mode`).
    pub async fn close_streaming_card(
        &self,
        card_id: &str,
        summary: &str,
        sequence: u32,
    ) -> Result<(), String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/cardkit/v1/cards/{}/settings",
            self.base_url, card_id
        );

        let settings = serde_json::json!({
            "config": {
                "streaming_mode": false,
                "summary": {
                    "content": truncate_chars(summary, 50)
                }
            }
        });

        let body = serde_json::json!({
            "settings": settings.to_string(),
            "sequence": sequence,
            "uuid": format!("c_{}_{}", card_id, sequence),
        });

        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Close streaming card failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Close streaming card HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// Send a static markdown card (non-streaming).
    pub async fn send_card(
        &self,
        chat_id: &str,
        markdown_text: &str,
        reply_to: Option<&str>,
    ) -> Result<String, FeishuSendError> {
        let card = serde_json::json!({
            "schema": "2.0",
            "config": { "wide_screen_mode": true },
            "body": {
                "elements": [
                    { "tag": "markdown", "content": markdown_text }
                ]
            }
        });

        if let Some(msg_id) = reply_to {
            return self
                .reply_message(msg_id, "interactive", &card.to_string())
                .await;
        }

        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
            self.base_url
        );

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "interactive",
            "content": card.to_string(),
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send card failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    // ── Reactions ──

    /// Add an emoji reaction to a message. Returns `reaction_id`.
    pub async fn add_reaction(&self, message_id: &str, emoji_type: &str) -> Result<String, String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reactions",
            self.base_url, message_id
        );

        let body = serde_json::json!({
            "reaction_type": { "emoji_type": emoji_type }
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Add reaction failed: {e}"))?;

        let reaction_resp: ReactionResponse = resp
            .json()
            .await
            .map_err(|e| format!("Reaction response parse failed: {e}"))?;

        if reaction_resp.code != 0 {
            return Err(format!(
                "Reaction error: code={}, msg={}",
                reaction_resp.code, reaction_resp.msg
            ));
        }

        reaction_resp
            .data
            .and_then(|d| d.reaction_id)
            .ok_or_else(|| "No reaction_id in response".to_string())
    }

    /// Remove an emoji reaction from a message.
    pub async fn remove_reaction(&self, message_id: &str, reaction_id: &str) -> Result<(), String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reactions/{}",
            self.base_url, message_id, reaction_id
        );

        let resp = self
            .http
            .delete(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Remove reaction failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Remove reaction HTTP {}", resp.status()));
        }
        Ok(())
    }

    // ── User Info ──

    /// Fetch user info by `open_id`. Returns name if available.
    pub async fn get_user_info(&self, open_id: &str) -> Result<Option<String>, String> {
        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/contact/v3/users/{}", self.base_url, open_id);

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .query(&[("user_id_type", "open_id")])
            .send()
            .await
            .map_err(|e| format!("User info request failed: {e}"))?;

        let user_resp: UserInfoResponse = resp
            .json()
            .await
            .map_err(|e| format!("User info parse failed: {e}"))?;

        if user_resp.code != 0 {
            tracing::debug!(
                "User info error for {}: code={}, msg={}",
                open_id,
                user_resp.code,
                user_resp.msg
            );
            return Ok(None);
        }

        let user = user_resp
            .data
            .and_then(|d| d.user)
            .ok_or_else(|| "No user data".to_string())?;
        Ok(user.name.or(user.english_name))
    }
}
