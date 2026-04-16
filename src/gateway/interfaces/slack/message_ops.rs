//! Slack API Operations
//!
//! Low-level functions for interacting with the Slack Web API and Socket Mode.
//! These are separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ChannelResult, ConversationId, InboundMessage, InboundMessageSender,
    MessageId, MessageMeta, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use crate::sync_primitives::Arc;
use super::directory::UserDirectory;
use chrono::Utc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::config::SlackConfig;

const SLACK_API_BASE: &str = "https://slack.com/api";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Slack message length limit (characters).
pub(crate) const SLACK_MSG_LIMIT: usize = 3000;

/// Debounce entry for coalescing rapid messages.
struct DebounceEntry {
    messages: Vec<InboundMessage>,
    deadline: tokio::time::Instant,
}

/// Debouncer for coalescing rapid messages from the same sender in the same channel.
struct SlackDebouncer {
    entries: std::collections::HashMap<String, DebounceEntry>,
    debounce_ms: u64,
    inbound_tx: InboundMessageSender,
}

impl SlackDebouncer {
    fn new(debounce_ms: u64, inbound_tx: InboundMessageSender) -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            debounce_ms,
            inbound_tx,
        }
    }

    fn build_key(msg: &InboundMessage, thread_ts: Option<&str>) -> String {
        let base = format!(
            "{}:{}",
            msg.conversation_id.as_str(),
            msg.sender_id.as_str()
        );
        match thread_ts {
            Some(ts) => format!("{}:{}", base, ts),
            None => base,
        }
    }

    async fn enqueue(&mut self, msg: InboundMessage, thread_ts: Option<&str>) -> bool {
        if self.debounce_ms == 0 {
            return self.send_immediate(msg).await;
        }

        let key = Self::build_key(&msg, thread_ts);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(self.debounce_ms);

        if let Some(entry) = self.entries.get_mut(&key) {
            entry.messages.push(msg);
            entry.deadline = deadline;
        } else {
            self.entries.insert(
                key,
                DebounceEntry {
                    messages: vec![msg],
                    deadline,
                },
            );
        }
        false
    }

    async fn flush_expired(&mut self) {
        let now = tokio::time::Instant::now();
        let keys_to_flush: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.deadline <= now)
            .map(|(k, _)| k.clone())
            .collect();

        for key in keys_to_flush {
            if let Some(entry) = self.entries.remove(&key) {
                self.flush_entry(entry).await;
            }
        }
    }

    async fn send_immediate(&self, msg: InboundMessage) -> bool {
        self.inbound_tx.send(msg).is_ok()
    }

    async fn flush_entry(&self, entry: DebounceEntry) {
        if entry.messages.is_empty() {
            return;
        }

        let combined = if entry.messages.len() == 1 {
            entry.messages.into_iter().next().unwrap()
        } else {
            let last = entry.messages.last().unwrap();
            let combined_text = entry
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n");

            InboundMessage {
                id: last.id.clone(),
                channel_id: last.channel_id.clone(),
                conversation_id: last.conversation_id.clone(),
                sender_id: last.sender_id.clone(),
                sender_name: last.sender_name.clone(),
                text: combined_text,
                attachments: last.attachments.clone(),
                timestamp: last.timestamp,
                reply_to: last.reply_to.clone(),
                is_group: last.is_group,
                raw: last.raw.clone(),
                metadata: last.metadata.clone(),
            }
        };

        let _ = self.inbound_tx.send(combined);
    }
}

/// Slack message operations helper.
///
/// Provides methods for sending messages and interacting with the Slack REST API.
pub struct SlackMessageOps;

impl SlackMessageOps {
    /// Validate bot token via `auth.test` and return the bot user ID.
    pub async fn validate_bot_token(
        client: &reqwest::Client,
        bot_token: &str,
    ) -> Result<String, ChannelError> {
        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/auth.test"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .send()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("auth.test request failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                ChannelError::AuthFailed(format!("auth.test response parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown error");
            return Err(ChannelError::AuthFailed(format!(
                "Slack auth.test failed: {err}"
            )));
        }

        let user_id = resp["user_id"].as_str().unwrap_or("unknown").to_string();
        Ok(user_id)
    }

    /// Get Socket Mode WebSocket URL via `apps.connections.open`.
    pub async fn get_socket_mode_url(
        client: &reqwest::Client,
        app_token: &str,
    ) -> Result<String, ChannelError> {
        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/apps.connections.open"))
            .header("Authorization", format!("Bearer {app_token}"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("apps.connections.open request failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("apps.connections.open response parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown error");
            return Err(ChannelError::Internal(format!(
                "Slack apps.connections.open failed: {err}"
            )));
        }

        resp["url"].as_str().map(String::from).ok_or_else(|| {
            ChannelError::Internal("Missing 'url' in connections.open response".to_string())
        })
    }

    /// Send a message via `chat.postMessage`.
    ///
    /// Automatically splits long messages and formats using SlackMrkdwn.
    pub async fn send_message(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<SendResult, ChannelError> {
        // Format text for Slack mrkdwn
        let formatted = MessageFormatter::format(text, MarkupFormat::SlackMrkdwn);
        let chunks = MessageFormatter::split(&formatted, SLACK_MSG_LIMIT);

        let mut last_result = None;

        for chunk in &chunks {
            let mut body = serde_json::json!({
                "channel": channel,
                "text": chunk,
            });

            if let Some(ts) = thread_ts {
                body["thread_ts"] = serde_json::Value::String(ts.to_string());
            }

            let resp: serde_json::Value = client
                .post(format!("{SLACK_API_BASE}/chat.postMessage"))
                .header("Authorization", format!("Bearer {bot_token}"))
                .json(&body)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(format!("chat.postMessage failed: {e}")))?
                .json()
                .await
                .map_err(|e| {
                    ChannelError::SendFailed(format!("chat.postMessage response parse failed: {e}"))
                })?;

            if resp["ok"].as_bool() != Some(true) {
                let err = resp["error"].as_str().unwrap_or("unknown");
                return Err(ChannelError::SendFailed(format!(
                    "Slack chat.postMessage failed: {err}"
                )));
            }

            let msg_ts = resp["ts"].as_str().unwrap_or("0").to_string();

            last_result = Some(SendResult {
                message_id: MessageId::new(msg_ts),
                timestamp: Utc::now(),
            });
        }

        last_result.ok_or_else(|| ChannelError::SendFailed("No message chunks to send".to_string()))
    }

    /// Send a typing indicator to a Slack channel.
    ///
    /// Slack's `chat.postTyping` broadcasts typing status to all channel members.
    /// The indicator typically times out after ~5 seconds if not refreshed.
    pub async fn post_typing(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
    ) -> ChannelResult<()> {
        let body = serde_json::json!({
            "channel": channel,
        });

        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/chat.postTyping"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("chat.postTyping failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!("chat.postTyping response parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown");
            tracing::debug!("Slack chat.postTyping failed: {err} (non-fatal)");
        }

        Ok(())
    }

    /// Upload a file to Slack using the 3-step upload flow.
    ///
    /// Step 1: `files.getUploadURLExternal` - get a presigned URL
    /// Step 2: POST the file content directly to the presigned URL
    /// Step 3: `files.completeUploadExternal` - finalize and share to channel
    ///
    /// This approach (rather than `files.upload`) is more reliable and works
    /// even when `files:write` scope is granted without `chat:write`.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_file(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        file_data: &[u8],
        filename: &str,
        title: Option<&str>,
        mime_type: Option<&str>,
        caption: Option<&str>,
        thread_ts: Option<&str>,
    ) -> ChannelResult<String> {
        let length = file_data.len() as u64;

        // Step 1: Get presigned upload URL
        let url_resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/files.getUploadURLExternal"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&serde_json::json!({
                "filename": filename,
                "length": length,
                "pretty打印": false,
            }))
            .send()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!("files.getUploadURLExternal failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!(
                    "files.getUploadURLExternal response parse failed: {e}"
                ))
            })?;

        if url_resp["ok"].as_bool() != Some(true) {
            return Err(ChannelError::SendFailed(format!(
                "files.getUploadURLExternal failed: {}",
                url_resp["error"].as_str().unwrap_or("unknown")
            )));
        }

        let upload_url = url_resp["upload_url"].as_str().ok_or_else(|| {
            ChannelError::SendFailed("Missing upload_url in response".to_string())
        })?;
        let file_id = url_resp["file_id"]
            .as_str()
            .ok_or_else(|| ChannelError::SendFailed("Missing file_id in response".to_string()))?;

        // Step 2: Upload file content directly to presigned URL
        let req = reqwest::Client::new().post(upload_url).header(
            "Content-Type",
            mime_type.unwrap_or("application/octet-stream"),
        );

        // For AWS S3-style presigned URLs, the body goes directly
        let upload_resp = req.body(file_data.to_vec()).send().await.map_err(|e| {
            ChannelError::SendFailed(format!("File upload to presigned URL failed: {e}"))
        })?;

        if !upload_resp.status().is_success() {
            return Err(ChannelError::SendFailed(format!(
                "File upload failed with status: {}",
                upload_resp.status()
            )));
        }

        // Step 3: Complete the upload and share to channel
        let mut complete_body = serde_json::json!({
            "files": [{
                "id": file_id,
                "title": title.unwrap_or(filename),
            }],
            "channel_id": channel,
        });

        if let Some(ts) = thread_ts {
            complete_body["thread_ts"] = serde_json::Value::String(ts.to_string());
        }

        if let Some(cap) = caption {
            complete_body["initial_comment"] = serde_json::Value::String(cap.to_string());
        }

        let complete_resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/files.completeUploadExternal"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&complete_body)
            .send()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!("files.completeUploadExternal failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!(
                    "files.completeUploadExternal response parse failed: {e}"
                ))
            })?;

        if complete_resp["ok"].as_bool() != Some(true) {
            return Err(ChannelError::SendFailed(format!(
                "files.completeUploadExternal failed: {}",
                complete_resp["error"].as_str().unwrap_or("unknown")
            )));
        }

        tracing::debug!("Slack file upload completed: {file_id}");
        Ok(file_id.to_string())
    }

    /// Normalize Unicode emoji to Slack emoji name.
    fn normalize_emoji_to_slack(emoji: &str) -> String {
        let base: String = emoji
            .chars()
            .filter(|c| {
                !matches!(c, '\u{FE0F}' | '\u{FE4D}' | '\u{FE4E}' | '\u{FE4F}')
                    && !matches!(c, '\u{1F3FB}'..='\u{1F3FF}')
                    && *c != '\u{200D}'
                    && !matches!(c, '\u{2640}'..='\u{2695}')
            })
            .collect();

        match base.as_str() {
            "👍" | "👍🏻" | "👍🏼" | "👍🏽" | "👍🏾" | "👍🏿" => {
                "thumbsup".to_string()
            }
            "👎" | "👎🏻" | "👎🏼" | "👎🏾" | "👎🏿" => "thumbsdown".to_string(),
            "❤️" | "❤" => "heart".to_string(),
            "😂" | "🤣" => "joy".to_string(),
            "😢" | "😭" => "cry".to_string(),
            "😮" | "😮‍💨" => "astonished".to_string(),
            "😒" => "disappointed".to_string(),
            "🙄" => "eyes_on_eyes".to_string(),
            "😴" | "😪" => "sleepy".to_string(),
            "🤔" => "thinking".to_string(),
            "👏" => "clap".to_string(),
            "😎" => "sunglasses".to_string(),
            "🤷" | "🤷‍♀️" | "🤷‍♂️" => "shrug".to_string(),
            "✅" => "white_check_mark".to_string(),
            "❌" => "x".to_string(),
            "🔥" => "fire".to_string(),
            "💯" => "100".to_string(),
            "✨" => "sparkles".to_string(),
            "🎉" => "tada".to_string(),
            "🚀" => "rocket".to_string(),
            "💪" => "muscle".to_string(),
            "🙏" | "🙏🏻" | "🙏🏼" | "🙏🏽" | "🙏🏾" | "🙏🏿" => {
                "pray".to_string()
            }
            "😊" => "blush".to_string(),
            "❤️‍🔥" => "heart_on_fire".to_string(),
            "👋" | "👋🏻" | "👋🏼" | "👋🏽" | "👋🏾" | "👋🏿" => {
                "wave".to_string()
            }
            "🙌" | "🙌🏻" | "🙌🏼" | "🙌🏽" | "🙌🏾" | "🙌🏿" => {
                "raised_hands".to_string()
            }
            "🎯" => "dart".to_string(),
            "💬" => "speech_balloon".to_string(),
            "💀" => "skull".to_string(),
            "☠️" => "skull_and_crossbones".to_string(),
            "🤡" => "clown".to_string(),
            "🌈" => "rainbow".to_string(),
            "💥" => "boom".to_string(),
            "⭐" | "🌟" => "star".to_string(),
            "💫" => "dizzy".to_string(),
            "👀" => "eyes".to_string(),
            "🔵" => "large_blue_circle".to_string(),
            "🔴" => "red_circle".to_string(),
            "🟢" => "green_circle".to_string(),
            "🟡" => "yellow_circle".to_string(),
            "⬛" => "black_large_square".to_string(),
            "⬜" => "white_large_square".to_string(),
            "🟧" => "orange_square".to_string(),
            "🟦" => "blue_square".to_string(),
            "🟪" => "purple_square".to_string(),
            "🟫" => "brown_square".to_string(),
            _ => {
                if emoji.starts_with(':') && emoji.ends_with(':') {
                    emoji.trim_matches(':').to_string()
                } else {
                    let cleaned = base.replace([' ', '-', '_'], "");
                    if cleaned.is_empty() {
                        return "emoji".to_string();
                    }
                    cleaned
                        .chars()
                        .filter(|c| c.is_alphanumeric())
                        .take(30)
                        .collect::<String>()
                        .to_lowercase()
                }
            }
        }
    }

    pub async fn add_reaction(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        timestamp: &str,
        emoji: &str,
    ) -> ChannelResult<()> {
        let slack_emoji = Self::normalize_emoji_to_slack(emoji);

        let body = serde_json::json!({
            "channel": channel,
            "timestamp": timestamp,
            "name": slack_emoji,
        });

        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/reactions.add"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("reactions.add request failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!("reactions.add response parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown");
            return Err(ChannelError::SendFailed(format!(
                "Slack reactions.add failed: {err}"
            )));
        }

        Ok(())
    }

    pub async fn remove_reaction(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        timestamp: &str,
        emoji: &str,
    ) -> ChannelResult<()> {
        let slack_emoji = Self::normalize_emoji_to_slack(emoji);

        let body = serde_json::json!({
            "channel": channel,
            "timestamp": timestamp,
            "name": slack_emoji,
        });

        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/reactions.remove"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("reactions.remove request failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!("reactions.remove response parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown");
            return Err(ChannelError::SendFailed(format!(
                "Slack reactions.remove failed: {err}"
            )));
        }

        Ok(())
    }

    pub async fn update_message(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        timestamp: &str,
        new_text: &str,
    ) -> ChannelResult<()> {
        let formatted = MessageFormatter::format(new_text, MarkupFormat::SlackMrkdwn);

        let body = serde_json::json!({
            "channel": channel,
            "ts": timestamp,
            "text": formatted,
        });

        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/chat.update"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("chat.update request failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!("chat.update response parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown");
            return Err(ChannelError::SendFailed(format!(
                "Slack chat.update failed: {err}"
            )));
        }

        Ok(())
    }

    pub async fn delete_message(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        timestamp: &str,
    ) -> ChannelResult<()> {
        let body = serde_json::json!({
            "channel": channel,
            "ts": timestamp,
        });

        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/chat.delete"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("chat.delete request failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!("chat.delete response parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown");
            return Err(ChannelError::SendFailed(format!(
                "Slack chat.delete failed: {err}"
            )));
        }

        Ok(())
    }

    /// Convert a Slack `app_mention` event to an `InboundMessage`.
    ///
    /// App mentions are similar to messages but include mention metadata.
    pub fn convert_app_mention_to_inbound(
        event: &serde_json::Value,
        channel_id: &ChannelId,
        bot_user_id: &str,
        config: &SlackConfig,
    ) -> Option<InboundMessage> {
        let user_id = event["user"].as_str()?;
        if user_id == bot_user_id {
            return None;
        }

        if !config.is_user_allowed(user_id) {
            tracing::debug!(
                "Slack: user {} not in user_allowlist, filtering mention",
                user_id
            );
            return None;
        }

        let slack_channel = event["channel"].as_str()?;
        if !config.is_channel_allowed(slack_channel) {
            return None;
        }

        let text = event["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }

        let normalized_text = MessageFormatter::normalize(text, MarkupFormat::SlackMrkdwn);

        let ts = event["ts"].as_str().unwrap_or("0");
        let timestamp = ts
            .split('.')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|epoch| chrono::DateTime::from_timestamp(epoch, 0))
            .unwrap_or_else(Utc::now);

        Some(InboundMessage {
            id: MessageId::new(ts.to_string()),
            channel_id: channel_id.clone(),
            conversation_id: ConversationId::new(slack_channel.to_string()),
            sender_id: UserId::new(user_id.to_string()),
            sender_name: Some(user_id.to_string()),
            text: normalized_text,
            attachments: Vec::new(),
            timestamp,
            reply_to: None,
            is_group: true,
            raw: Some(event.clone()),
            metadata: vec![MessageMeta::AppMention],
        })
    }

    /// Convert a Slack event payload to an `InboundMessage`.
    ///
    /// Returns `None` if the event should be ignored (bot's own message,
    /// filtered channel, non-message event, etc.).
    pub fn convert_event_to_inbound(
        event: &serde_json::Value,
        channel_id: &ChannelId,
        bot_user_id: &str,
        config: &SlackConfig,
    ) -> Option<InboundMessage> {
        let event_type = event["type"].as_str()?;
        if event_type != "message" {
            return None;
        }

        // Handle message_changed subtype: extract inner message
        let subtype = event["subtype"].as_str();
        let (msg_data, _is_edit) = match subtype {
            Some("message_changed") => match event.get("message") {
                Some(inner) => (inner, true),
                None => return None,
            },
            Some(_) => return None, // Skip other subtypes (joins, leaves, etc.)
            None => (event, false),
        };

        // Filter out bot messages
        if msg_data.get("bot_id").is_some() {
            return None;
        }

        let user_id = msg_data["user"]
            .as_str()
            .or_else(|| event["user"].as_str())?;

        // Filter out bot's own messages
        if user_id == bot_user_id {
            return None;
        }

        // Filter by user allowlist
        if !config.is_user_allowed(user_id) {
            tracing::debug!(
                "Slack: user {} not in user_allowlist, filtering",
                user_id
            );
            return None;
        }

        let slack_channel = event["channel"].as_str()?;

        // Filter by allowed channels
        if !config.is_channel_allowed(slack_channel) {
            return None;
        }

        // Check DM permission (DMs start with "D")
        let is_dm = slack_channel.starts_with('D');
        if is_dm && !config.dm_allowed {
            return None;
        }

        let text = msg_data["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }

        // Normalize Slack mrkdwn to standard Markdown
        let normalized_text = MessageFormatter::normalize(text, MarkupFormat::SlackMrkdwn);

        let ts = msg_data["ts"]
            .as_str()
            .or_else(|| event["ts"].as_str())
            .unwrap_or("0");

        // Parse timestamp (Slack uses epoch.microseconds format)
        let timestamp = ts
            .split('.')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|epoch| chrono::DateTime::from_timestamp(epoch, 0))
            .unwrap_or_else(Utc::now);

        // Extract thread_ts for reply threading
        let reply_to = event["thread_ts"]
            .as_str()
            .map(|ts| MessageId::new(ts.to_string()));

        Some(InboundMessage {
            id: MessageId::new(ts.to_string()),
            channel_id: channel_id.clone(),
            conversation_id: ConversationId::new(slack_channel.to_string()),
            sender_id: UserId::new(user_id.to_string()),
            sender_name: Some(user_id.to_string()), // Slack user IDs as display name
            text: normalized_text,
            attachments: Vec::new(), // TODO: extract Slack file attachments
            timestamp,
            reply_to,
            is_group: !is_dm,
            raw: Some(event.clone()),
            metadata: vec![],
        })
    }

    /// Run the Socket Mode WebSocket loop with reconnection and exponential backoff.
    ///
    /// This function runs indefinitely until a shutdown signal is received.
    /// It handles:
    /// - Getting a fresh WebSocket URL via `apps.connections.open`
    /// - Connecting with tokio-tungstenite
    /// - Processing events in a loop with `tokio::select!`
    /// - ACK-ing `events_api` envelopes
    /// - Reconnecting with exponential backoff on disconnect
    pub async fn run_socket_mode_loop(
        client: reqwest::Client,
        app_token: String,
        bot_user_id: Arc<RwLock<Option<String>>>,
        channel_id: ChannelId,
        config: SlackConfig,
        inbound_tx: InboundMessageSender,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
        user_directory: Option<Arc<UserDirectory>>,
    ) {
        use futures_util::{SinkExt, StreamExt};

        let mut backoff = INITIAL_BACKOFF;
        let mut debouncer = SlackDebouncer::new(config.debounce_ms, inbound_tx);

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            // Get a fresh WebSocket URL
            let ws_url = match Self::get_socket_mode_url(&client, &app_token).await {
                Ok(url) => url,
                Err(e) => {
                    tracing::warn!(
                        "Slack: failed to get WebSocket URL: {e}, retrying in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            tracing::info!("Connecting to Slack Socket Mode...");

            let ws_result = tokio_tungstenite::connect_async(&ws_url).await;
            let ws_stream = match ws_result {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!(
                        "Slack WebSocket connection failed: {e}, retrying in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            // Reset backoff on successful connection
            backoff = INITIAL_BACKOFF;
            tracing::info!("Slack Socket Mode connected");

            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            let mut flush_interval = tokio::time::interval(Duration::from_millis(50));

            let should_reconnect = 'inner: loop {
                tokio::select! {
                    _ = flush_interval.tick() => {
                        debouncer.flush_expired().await;
                        continue;
                    }
                    msg = ws_rx.next() => {
                        debouncer.flush_expired().await;

                        let msg = match msg {
                            Some(Ok(m)) => m,
                            Some(Err(e)) => {
                                tracing::warn!("Slack WebSocket error: {e}");
                                break 'inner true;
                            }
                            None => {
                                tracing::info!("Slack WebSocket closed");
                                break 'inner true;
                            }
                        };

                        let text = match msg {
                            tokio_tungstenite::tungstenite::Message::Text(t) => t,
                            tokio_tungstenite::tungstenite::Message::Close(_) => {
                                tracing::info!("Slack Socket Mode closed by server");
                                break 'inner true;
                            }
                            _ => continue,
                        };

                        let payload: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("Slack: failed to parse message: {e}");
                                continue;
                            }
                        };

                        let envelope_type = payload["type"].as_str().unwrap_or("");

                        match envelope_type {
                            "hello" => {
                                tracing::debug!("Slack Socket Mode hello received");
                            }

                            "events_api" => {
                                let envelope_id = payload["envelope_id"].as_str().unwrap_or("");
                                if !envelope_id.is_empty() {
                                    let ack = serde_json::json!({ "envelope_id": envelope_id });
                                    if let Err(e) = ws_tx
                                        .send(tokio_tungstenite::tungstenite::Message::Text(
                                            serde_json::to_string(&ack).unwrap().into(),
                                        ))
                                        .await
                                    {
                                        tracing::error!("Slack: failed to send ack: {e}");
                                        break 'inner true;
                                    }
                                }

                                let event = &payload["payload"]["event"];
                                let bot_id_guard = bot_user_id.read().await;
                                let bot_id_str = bot_id_guard.as_deref().unwrap_or("");
                                let event_type = event["type"].as_str().unwrap_or("");

                                let inbound = match event_type {
                                    "message" => Self::convert_event_to_inbound(
                                        event,
                                        &channel_id,
                                        bot_id_str,
                                        &config,
                                    ),
                                    "app_mention" => Self::convert_app_mention_to_inbound(
                                        event,
                                        &channel_id,
                                        bot_id_str,
                                        &config,
                                    ),
                                    _ => None,
                                };

                                if let Some(inbound) = inbound {
                                    let resolved_inbound = if config.resolve_user_names {
                                        if let Some(ref dir) = user_directory {
                                            if let Some(name) =
                                                dir.resolve(inbound.sender_id.as_str()).await
                                            {
                                                InboundMessage {
                                                    sender_name: Some(name),
                                                    ..inbound
                                                }
                                            } else {
                                                inbound
                                            }
                                        } else {
                                            inbound
                                        }
                                    } else {
                                        inbound
                                    };

                                    tracing::debug!(
                                        "Slack {} from {}: {}",
                                        event_type,
                                        resolved_inbound.sender_id.as_str(),
                                        &resolved_inbound.text[..resolved_inbound.text.len().min(50)]
                                    );
                                    let reply_to = resolved_inbound.reply_to.clone();
                                    let thread_ts = reply_to.as_ref().map(|id| id.as_str());
                                    if debouncer.enqueue(resolved_inbound, thread_ts).await {
                                        tracing::error!("Slack: inbound channel closed");
                                        return;
                                    }
                                }
                            }

                            "disconnect" => {
                                let reason = payload["reason"].as_str().unwrap_or("unknown");
                                tracing::info!("Slack disconnect request: {reason}");
                                break 'inner true;
                            }

                            _ => {
                                tracing::debug!("Slack envelope type: {envelope_type}");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            let _ = ws_tx.close().await;
                            return;
                        }
                    }
                };
            };

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            tracing::warn!("Slack: reconnecting in {backoff:?}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }

        tracing::info!("Slack Socket Mode loop stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_basic_message() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello agent!",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        assert_eq!(msg.channel_id.as_str(), "slack");
        assert_eq!(msg.conversation_id.as_str(), "C789");
        assert_eq!(msg.sender_id.as_str(), "U456");
        assert_eq!(msg.text, "Hello agent!");
        assert!(msg.is_group);
    }

    #[test]
    fn test_convert_filters_bot_messages() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Bot message",
            "ts": "1700000000.000100",
            "bot_id": "B999"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_filters_own_user() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "My message",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "U456", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_channel_filter() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");

        // Not in allowed channels
        let config = SlackConfig {
            allowed_channels: vec!["C111".to_string(), "C222".to_string()],
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());

        // In allowed channels
        let config = SlackConfig {
            allowed_channels: vec!["C789".to_string()],
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
    }

    #[test]
    fn test_convert_skips_other_subtypes() {
        let event = serde_json::json!({
            "type": "message",
            "subtype": "channel_join",
            "user": "U456",
            "channel": "C789",
            "text": "joined",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_message_changed() {
        let event = serde_json::json!({
            "type": "message",
            "subtype": "message_changed",
            "channel": "C789",
            "message": {
                "user": "U456",
                "text": "Edited message text",
                "ts": "1700000000.000100"
            },
            "ts": "1700000001.000200"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        assert_eq!(msg.conversation_id.as_str(), "C789");
        assert_eq!(msg.text, "Edited message text");
    }

    #[test]
    fn test_convert_non_message_event() {
        let event = serde_json::json!({
            "type": "reaction_added",
            "user": "U456",
            "reaction": "thumbsup"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_empty_text() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_dm_message() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "D12345",
            "text": "Private message",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");

        // DMs allowed
        let config = SlackConfig {
            dm_allowed: true,
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
        assert!(!msg.unwrap().is_group);

        // DMs not allowed
        let config = SlackConfig {
            dm_allowed: false,
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_thread_reply() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Thread reply",
            "ts": "1700000002.000300",
            "thread_ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "1700000000.000100");
    }

    #[test]
    fn test_convert_normalizes_mrkdwn() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "*bold text*",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        // Slack *bold* normalizes to Markdown **bold**
        assert_eq!(msg.text, "**bold text**");
    }

    #[test]
    fn test_convert_timestamp_parsing() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        assert_eq!(msg.timestamp.timestamp(), 1700000000);
    }

    #[test]
    fn test_normalize_emoji_to_slack() {
        fn normalize(emoji: &str) -> String {
            SlackMessageOps::normalize_emoji_to_slack(emoji)
        }

        assert_eq!(normalize("👍"), "thumbsup");
        assert_eq!(normalize("👎"), "thumbsdown");
        assert_eq!(normalize("❤️"), "heart");
        assert_eq!(normalize("😂"), "joy");
        assert_eq!(normalize("😢"), "cry");
        assert_eq!(normalize("😮"), "astonished");
        assert_eq!(normalize("😒"), "disappointed");
        assert_eq!(normalize("😴"), "sleepy");
        assert_eq!(normalize("🤔"), "thinking");
        assert_eq!(normalize("👏"), "clap");
        assert_eq!(normalize("🙄"), "eyes_on_eyes");
        assert_eq!(normalize("😎"), "sunglasses");
        assert_eq!(normalize("🤷"), "shrug");
        assert_eq!(normalize("✅"), "white_check_mark");
        assert_eq!(normalize("❌"), "x");
        assert_eq!(normalize("🔥"), "fire");
        assert_eq!(normalize("💯"), "100");
        assert_eq!(normalize("✨"), "sparkles");
        assert_eq!(normalize("🎉"), "tada");
        assert_eq!(normalize("🚀"), "rocket");
        assert_eq!(normalize("💪"), "muscle");
        assert_eq!(normalize("🙏"), "pray");
        assert_eq!(normalize("😊"), "blush");
        assert_eq!(normalize("🤷‍♀️"), "shrug");
        assert_eq!(normalize("🤷‍♂️"), "shrug");
        assert_eq!(normalize("👀"), "eyes");
        assert_eq!(normalize("🙌"), "raised_hands");
        assert_eq!(normalize("👍🏻"), "thumbsup");
        assert_eq!(normalize("👍🏿"), "thumbsup");
        assert_eq!(normalize(":thumbsup:"), "thumbsup");
        assert_eq!(normalize("unknown_emoji_xyz"), "unknownemojixyz");
        assert_eq!(normalize("💫"), "dizzy");
    }

    #[test]
    fn test_convert_app_mention_basic() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U456",
            "channel": "C789",
            "text": "<@B123> Hello bot!",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config)
                .unwrap();

        assert_eq!(msg.channel_id.as_str(), "slack");
        assert_eq!(msg.conversation_id.as_str(), "C789");
        assert_eq!(msg.sender_id.as_str(), "U456");
        assert!(msg.text.contains("Hello bot"));
        assert!(msg.is_group);
        assert!(msg
            .metadata
            .iter()
            .any(|m| matches!(m, MessageMeta::AppMention)));
    }

    #[test]
    fn test_convert_app_mention_filters_own_message() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "B123",
            "channel": "C789",
            "text": "<@B123> self mention",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_app_mention_channel_filter() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U456",
            "channel": "C789",
            "text": "<@B123> Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");

        let config = SlackConfig {
            allowed_channels: vec!["C111".to_string()],
            ..Default::default()
        };
        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_filters_user_not_in_allowlist() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };

        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_allows_user_in_allowlist() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U123",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };

        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
    }

    #[test]
    fn test_convert_allowlist_empty_allows_all() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();

        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
    }

    #[test]
    fn test_convert_mention_filters_user_not_in_allowlist() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U456",
            "channel": "C789",
            "text": "<@B123> Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };

        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_mention_allows_user_in_allowlist() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U123",
            "channel": "C789",
            "text": "<@B123> Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };

        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
    }
}
