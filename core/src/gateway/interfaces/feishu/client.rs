use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use reqwest::multipart;

use super::config::FeishuConfig;
use super::types::*;

const TOKEN_REFRESH_MARGIN_SECS: u64 = 300;
const DEFAULT_TOKEN_EXPIRY_SECS: u64 = 7200;

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

pub struct FeishuClient {
    app_id: String,
    app_secret: String,
    base_url: String,
    http: reqwest::Client,
    token: Arc<RwLock<TokenState>>,
    bot_open_id: Arc<RwLock<Option<String>>>,
}

pub(super) struct TokenState {
    pub(super) access_token: String,
    pub(super) expires_at: Instant,
}

impl TokenState {
    fn needs_refresh(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

impl FeishuClient {
    pub fn new(config: &FeishuConfig) -> Self {
        let http = reqwest::Client::new();
        Self {
            app_id: config.app_id.clone(),
            app_secret: config.app_secret.clone(),
            base_url: config.base_url(),
            http,
            token: Arc::new(RwLock::new(TokenState {
                access_token: String::new(),
                expires_at: Instant::now(),
            })),
            bot_open_id: Arc::new(RwLock::new(None)),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn bot_open_id(&self) -> Option<String> {
        self.bot_open_id.read().await.clone()
    }

    // ── Token Management ──

    pub async fn refresh_token(&self) -> Result<(), String> {
        let url = format!("{}/open-apis/auth/v3/app_access_token/internal", self.base_url);
        let body = serde_json::json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret,
        });

        let resp = self.http.post(&url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Token request failed: {e}"))?;

        let token_resp: TokenResponse = resp.json().await
            .map_err(|e| format!("Token response parse failed: {e}"))?;

        if token_resp.code != 0 {
            return Err(format!("Token error: code={}, msg={}", token_resp.code, token_resp.msg));
        }

        let access_token = token_resp.app_access_token
            .ok_or_else(|| "No access_token in response".to_string())?;
        let expire = token_resp.expire.unwrap_or(DEFAULT_TOKEN_EXPIRY_SECS);

        let expires_at = Instant::now() + Duration::from_secs(expire.saturating_sub(TOKEN_REFRESH_MARGIN_SECS));

        let mut state = self.token.write().await;
        state.access_token = access_token;
        state.expires_at = expires_at;

        tracing::debug!("Feishu token refreshed, expires in {}s", expire);
        Ok(())
    }

    async fn get_token(&self) -> Result<String, String> {
        {
            let state = self.token.read().await;
            if !state.needs_refresh() {
                return Ok(state.access_token.clone());
            }
        }
        self.refresh_token().await?;
        let state = self.token.read().await;
        Ok(state.access_token.clone())
    }

    pub fn spawn_token_refresh(&self, shutdown: tokio::sync::watch::Receiver<bool>) {
        let app_id = self.app_id.clone();
        let app_secret = self.app_secret.clone();
        let base_url = self.base_url.clone();
        let http = self.http.clone();
        let token = self.token.clone();

        tokio::spawn(async move {
            let mut shutdown = shutdown;
            loop {
                let sleep_duration = {
                    let state = token.read().await;
                    let now = Instant::now();
                    if state.expires_at > now {
                        state.expires_at.duration_since(now)
                    } else {
                        Duration::from_secs(60)
                    }
                };

                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration) => {}
                    _ = shutdown.changed() => {
                        tracing::debug!("Token refresh task shutting down");
                        return;
                    }
                }

                let url = format!("{}/open-apis/auth/v3/app_access_token/internal", base_url);
                let body = serde_json::json!({
                    "app_id": app_id,
                    "app_secret": app_secret,
                });

                match http.post(&url)
                    .header("Content-Type", "application/json; charset=utf-8")
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        match resp.json::<TokenResponse>().await {
                            Ok(tr) if tr.code == 0 => {
                                if let Some(at) = tr.app_access_token {
                                    let expire = tr.expire.unwrap_or(DEFAULT_TOKEN_EXPIRY_SECS);
                                    let mut state = token.write().await;
                                    state.access_token = at;
                                    state.expires_at = Instant::now() + Duration::from_secs(
                                        expire.saturating_sub(TOKEN_REFRESH_MARGIN_SECS)
                                    );
                                    tracing::debug!("Feishu token refreshed (background)");
                                }
                            }
                            Ok(tr) => tracing::warn!("Token refresh failed: code={}, msg={}", tr.code, tr.msg),
                            Err(e) => tracing::warn!("Token refresh parse error: {e}"),
                        }
                    }
                    Err(e) => tracing::warn!("Token refresh request error: {e}"),
                }
            }
        });
    }

    // ── Bot Info ──

    pub async fn get_bot_info(&self) -> Result<BotInfo, String> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/bot/v3/info", self.base_url);

        let resp = self.http.get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Bot info request failed: {e}"))?;

        let info: BotInfoResponse = resp.json().await
            .map_err(|e| format!("Bot info parse failed: {e}"))?;

        if info.code != 0 {
            return Err(format!("Bot info error: code={}, msg={}", info.code, info.msg.unwrap_or_default()));
        }

        let bot = info.bot.ok_or_else(|| "No bot info in response".to_string())?;

        if let Some(ref oid) = bot.open_id {
            *self.bot_open_id.write().await = Some(oid.clone());
        }

        Ok(bot)
    }

    // ── WebSocket Endpoint ──

    pub(super) fn ws_reconnect_handle(&self) -> (reqwest::Client, String, Arc<RwLock<TokenState>>) {
        (self.http.clone(), self.base_url.clone(), self.token.clone())
    }

    pub async fn get_ws_endpoint(&self) -> Result<String, String> {
        let token = self.get_token().await?;
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
            return Err(format!("WS endpoint error: code={}, msg={}", ws_resp.code, ws_resp.msg));
        }

        let data = ws_resp.data.ok_or_else(|| "No data in WS endpoint response".to_string())?;
        Ok(data.url)
    }

    // ── Send Messages ──

    pub async fn send_text(&self, chat_id: &str, text: &str, reply_to: Option<&str>) -> Result<String, FeishuSendError> {
        if let Some(msg_id) = reply_to {
            return self.reply_message(msg_id, "text", &serde_json::json!({"text": text}).to_string()).await;
        }

        let token = self.get_token().await?;
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

    pub async fn send_image(&self, chat_id: &str, image_key: &str, reply_to: Option<&str>) -> Result<String, FeishuSendError> {
        if let Some(msg_id) = reply_to {
            return self.reply_message(msg_id, "image", &serde_json::json!({"image_key": image_key}).to_string()).await;
        }

        let token = self.get_token().await?;
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

    async fn reply_message(&self, message_id: &str, msg_type: &str, content: &str) -> Result<String, FeishuSendError> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages/{}/reply", self.base_url, message_id);

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

    async fn parse_send_response(&self, resp: reqwest::Response) -> Result<String, FeishuSendError> {
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5);
            return Err(FeishuSendError::RateLimited { retry_after_secs: retry_after });
        }

        let send_resp: SendMessageResponse = resp.json().await
            .map_err(|e| FeishuSendError::Other(format!("Send response parse failed: {e}")))?;

        if send_resp.code != 0 {
            return Err(FeishuSendError::Other(format!("Send error: code={}, msg={}", send_resp.code, send_resp.msg)));
        }

        let msg_id = send_resp.data
            .and_then(|d| d.message_id)
            .unwrap_or_default();

        Ok(msg_id)
    }

    // ── Media ──

    pub async fn upload_image(&self, data: Vec<u8>, filename: &str) -> Result<String, String> {
        let token = self.get_token().await?;
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
            return Err(format!("Upload error: code={}, msg={}", upload_resp.code, upload_resp.msg));
        }

        upload_resp.data
            .and_then(|d| d.image_key)
            .ok_or_else(|| "No image_key in upload response".to_string())
    }

    pub async fn download_media(&self, message_id: &str, file_key: &str) -> Result<Vec<u8>, String> {
        let token = self.get_token().await?;
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

        resp.bytes().await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Download media read failed: {e}"))
    }

    // ── Card Kit (Streaming Cards) ──

    /// Create a streaming card and return the card_id.
    pub async fn create_streaming_card(&self, initial_text: &str) -> Result<String, String> {
        let token = self.get_token().await?;
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

        let card_resp: super::types::CardCreateResponse = resp.json().await
            .map_err(|e| format!("Card create response parse failed: {e}"))?;

        if card_resp.code != 0 {
            return Err(format!("Card create error: code={}, msg={}", card_resp.code, card_resp.msg));
        }

        card_resp.data
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

        let token = self.get_token().await?;
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

    /// Update a streaming card's content element.
    pub async fn update_streaming_card(
        &self,
        card_id: &str,
        content: &str,
        sequence: u32,
    ) -> Result<(), String> {
        let token = self.get_token().await?;
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

    /// Close a streaming card (disable streaming_mode).
    pub async fn close_streaming_card(
        &self,
        card_id: &str,
        summary: &str,
        sequence: u32,
    ) -> Result<(), String> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/cardkit/v1/cards/{}/settings", self.base_url, card_id);

        let settings = serde_json::json!({
            "config": {
                "streaming_mode": false,
                "summary": {
                    "content": summary.get(..50).unwrap_or(summary)
                }
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

        let token = self.get_token().await?;
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

    // ── Reactions ──

    /// Add an emoji reaction to a message. Returns reaction_id.
    pub async fn add_reaction(&self, message_id: &str, emoji_type: &str) -> Result<String, String> {
        let token = self.get_token().await?;
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

        let reaction_resp: super::types::ReactionResponse = resp.json().await
            .map_err(|e| format!("Reaction response parse failed: {e}"))?;

        if reaction_resp.code != 0 {
            return Err(format!("Reaction error: code={}, msg={}", reaction_resp.code, reaction_resp.msg));
        }

        reaction_resp.data
            .and_then(|d| d.reaction_id)
            .ok_or_else(|| "No reaction_id in response".to_string())
    }

    /// Remove an emoji reaction from a message.
    pub async fn remove_reaction(&self, message_id: &str, reaction_id: &str) -> Result<(), String> {
        let token = self.get_token().await?;
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
}
