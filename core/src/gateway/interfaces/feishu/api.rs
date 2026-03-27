use std::sync::Arc;
use tokio::sync::RwLock;
use reqwest::multipart;

use super::auth::TokenManager;
use super::types::*;

// ── Send Error ──

#[derive(Debug)]
pub enum FeishuSendError {
    RateLimited { retry_after_secs: u64 },
    Other(String),
}

impl From<String> for FeishuSendError {
    fn from(s: String) -> Self {
        FeishuSendError::Other(s)
    }
}

// ── FeishuApi ──

/// Thin HTTP client for all Feishu/Lark REST API calls.
/// Authentication is delegated to the shared `TokenManager`.
pub struct FeishuApi {
    auth: Arc<TokenManager>,
    http: reqwest::Client,
    base_url: String,
    bot_open_id: Arc<RwLock<Option<String>>>,
}

impl FeishuApi {
    pub fn new(auth: Arc<TokenManager>, base_url: &str, http: reqwest::Client) -> Self {
        Self {
            auth,
            http,
            base_url: base_url.to_string(),
            bot_open_id: Arc::new(RwLock::new(None)),
        }
    }

    // ── Bot Info ──

    /// Fetch bot info and cache the bot's open_id for mention detection.
    pub async fn get_bot_info(&self) -> Result<BotInfo, String> {
        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/bot/v3/info", self.base_url);

        let resp = self.http.get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Bot info request failed: {e}"))?;

        let info: BotInfoResponse = resp.json().await
            .map_err(|e| format!("Bot info parse failed: {e}"))?;

        if info.code != 0 {
            return Err(format!(
                "Bot info error: code={}, msg={}",
                info.code,
                info.msg.unwrap_or_default()
            ));
        }

        let bot = info.bot.ok_or_else(|| "No bot info in response".to_string())?;

        if let Some(ref oid) = bot.open_id {
            *self.bot_open_id.write().await = Some(oid.clone());
        }

        Ok(bot)
    }

    /// Returns the cached bot open_id (populated after `get_bot_info`).
    pub async fn bot_open_id(&self) -> Option<String> {
        self.bot_open_id.read().await.clone()
    }

    // ── WebSocket Endpoint ──

    /// Request a new WebSocket endpoint URL from the Feishu API.
    pub async fn get_ws_endpoint(&self) -> Result<String, String> {
        self.refresh_ws_endpoint().await
    }

    /// Fetch a fresh WebSocket endpoint URL — used for both initial connect and reconnect.
    pub async fn refresh_ws_endpoint(&self) -> Result<String, String> {
        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/callback/ws/endpoint", self.base_url);

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| format!("WS endpoint request failed: {e}"))?;

        let ws_resp: WsEndpointResponse = resp.json().await
            .map_err(|e| format!("WS endpoint parse failed: {e}"))?;

        if ws_resp.code != 0 {
            return Err(format!(
                "WS endpoint error: code={}, msg={}",
                ws_resp.code, ws_resp.msg
            ));
        }

        let data = ws_resp.data
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
            return self.reply_message(msg_id, "text", &serde_json::json!({"text": text}).to_string()).await;
        }

        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", self.base_url);

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "text",
            "content": serde_json::json!({"text": text}).to_string(),
        });

        let resp = self.http.post(&url)
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
            return self.reply_message(
                msg_id,
                "image",
                &serde_json::json!({"image_key": image_key}).to_string(),
            ).await;
        }

        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", self.base_url);

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "image",
            "content": serde_json::json!({"image_key": image_key}).to_string(),
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send image failed: {e}")))?;

        self.parse_send_response(resp).await
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
            return self.reply_message(msg_id, "interactive", &card.to_string()).await;
        }

        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", self.base_url);

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "interactive",
            "content": card.to_string(),
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send card failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    /// Reply to an existing message with arbitrary content. Made `pub` so message_ops.rs can use it.
    pub async fn reply_message(
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

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Reply message failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    /// Inspect the HTTP response from a send/reply call and extract the message_id.
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
            return Err(FeishuSendError::RateLimited { retry_after_secs: retry_after });
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

    // ── Card Kit (Streaming Cards) ──

    /// Create a streaming card and return the card_id.
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

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Create streaming card failed: {e}"))?;

        let card_resp: CardCreateResponse = resp.json().await
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
            return self.reply_message(msg_id, "interactive", &content.to_string()).await;
        }

        let token = self.auth.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", self.base_url);

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "interactive",
            "content": content.to_string(),
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send card message failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    /// Update a streaming card's content element in place.
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

        let resp = self.http.put(&url)
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

    /// Close a streaming card by disabling streaming_mode and setting the summary.
    ///
    /// The summary is safely truncated to 50 Unicode characters (not bytes).
    pub async fn close_streaming_card(
        &self,
        card_id: &str,
        text: &str,
        sequence: u32,
    ) -> Result<(), String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/cardkit/v1/cards/{}/settings",
            self.base_url, card_id
        );

        // UTF-8-safe truncation to 50 Unicode scalar values
        let summary = text
            .char_indices()
            .nth(50)
            .map(|(idx, _)| &text[..idx])
            .unwrap_or(text);

        let settings = serde_json::json!({
            "config": {
                "streaming_mode": false,
                "summary": { "content": summary }
            }
        });

        let body = serde_json::json!({
            "settings": settings.to_string(),
            "sequence": sequence,
            "uuid": format!("c_{}_{}", card_id, sequence),
        });

        let resp = self.http.patch(&url)
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

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Upload image failed: {e}"))?;

        let upload_resp: UploadImageResponse = resp.json().await
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

    pub async fn download_media(
        &self,
        message_id: &str,
        file_key: &str,
    ) -> Result<Vec<u8>, String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/resources/{}?type=image",
            self.base_url, message_id, file_key
        );

        let resp = self.http.get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Download media failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Download media HTTP {}", resp.status()));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Download media read failed: {e}"))
    }

    // ── Reactions ──

    /// Add an emoji reaction to a message. Returns the reaction_id.
    pub async fn add_reaction(
        &self,
        message_id: &str,
        emoji_type: &str,
    ) -> Result<String, String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reactions",
            self.base_url, message_id
        );

        let body = serde_json::json!({
            "reaction_type": { "emoji_type": emoji_type }
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Add reaction failed: {e}"))?;

        let reaction_resp: ReactionResponse = resp.json().await
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
    pub async fn remove_reaction(
        &self,
        message_id: &str,
        reaction_id: &str,
    ) -> Result<(), String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reactions/{}",
            self.base_url, message_id, reaction_id
        );

        let resp = self.http.delete(&url)
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

    /// Fetch the display name for a user by their open_id.
    ///
    /// Returns `None` if the user record does not have a name field.
    pub async fn get_user_info(&self, open_id: &str) -> Result<Option<String>, String> {
        let token = self.auth.get_token().await?;
        let url = format!(
            "{}/open-apis/contact/v3/users/{}?user_id_type=open_id",
            self.base_url, open_id
        );

        let resp = self.http.get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Get user info request failed: {e}"))?;

        let body: serde_json::Value = resp.json().await
            .map_err(|e| format!("Get user info parse failed: {e}"))?;

        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = body.get("msg").and_then(|v| v.as_str()).unwrap_or("unknown error");
            return Err(format!("Get user info error: code={code}, msg={msg}"));
        }

        let name = body
            .pointer("/data/user/name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_close_streaming_card_utf8_truncation() {
        // Verify the truncation logic without network I/O — just the char_indices pattern.
        let text = "你好世界这是一段超过五十个字符的中文文本用于测试截断逻辑是否正确处理Unicode字符而不是按字节截断";
        let summary = text
            .char_indices()
            .nth(50)
            .map(|(idx, _)| &text[..idx])
            .unwrap_or(text);
        // Each Chinese character is 3 bytes; byte-slicing at 50 would panic or give wrong result.
        assert_eq!(summary.chars().count(), 50);
    }

    #[test]
    fn test_close_streaming_card_short_text() {
        let text = "短文本";
        let summary = text
            .char_indices()
            .nth(50)
            .map(|(idx, _)| &text[..idx])
            .unwrap_or(text);
        // Text shorter than 50 chars returns as-is.
        assert_eq!(summary, text);
    }
}
