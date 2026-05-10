use super::*;

pub struct SlackMessageOps;

impl SlackMessageOps {
    /// Validate bot token via `auth.test` and return the bot user ID.
    pub async fn validate_bot_token(
        client: &reqwest::Client,
        bot_token: &str,
        api_base: Option<&str>,
    ) -> Result<String, ChannelError> {
        let base = api_base.unwrap_or(SLACK_API_BASE);
        let resp: serde_json::Value = client
            .post(format!("{base}/auth.test"))
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
        Self::send_message_with_base(client, bot_token, channel, text, thread_ts, None).await
    }

    pub async fn send_message_with_base(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
        api_base: Option<&str>,
    ) -> Result<SendResult, ChannelError> {
        let base = api_base.unwrap_or(SLACK_API_BASE);
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
                .post(format!("{base}/chat.postMessage"))
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
        Self::post_typing_with_base(client, bot_token, channel, None).await
    }

    pub async fn post_typing_with_base(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        api_base: Option<&str>,
    ) -> ChannelResult<()> {
        let base = api_base.unwrap_or(SLACK_API_BASE);
        let body = serde_json::json!({
            "channel": channel,
        });

        let resp: serde_json::Value = client
            .post(format!("{base}/chat.postTyping"))
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
        Self::upload_file_with_base(
            client, bot_token, channel, file_data, filename, title, mime_type, caption, thread_ts,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upload_file_with_base(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        file_data: &[u8],
        filename: &str,
        title: Option<&str>,
        mime_type: Option<&str>,
        caption: Option<&str>,
        thread_ts: Option<&str>,
        api_base: Option<&str>,
    ) -> ChannelResult<String> {
        let length = file_data.len() as u64;
        let base = api_base.unwrap_or(SLACK_API_BASE);

        // Step 1: Get presigned upload URL
        let url_resp: serde_json::Value = client
            .post(format!("{base}/files.getUploadURLExternal"))
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
            .post(format!("{base}/files.completeUploadExternal"))
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
    pub(crate) fn normalize_emoji_to_slack(emoji: &str) -> String {
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
        Self::add_reaction_with_base(client, bot_token, channel, timestamp, emoji, None).await
    }

    pub async fn add_reaction_with_base(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        timestamp: &str,
        emoji: &str,
        api_base: Option<&str>,
    ) -> ChannelResult<()> {
        let slack_emoji = Self::normalize_emoji_to_slack(emoji);
        let base = api_base.unwrap_or(SLACK_API_BASE);

        let body = serde_json::json!({
            "channel": channel,
            "timestamp": timestamp,
            "name": slack_emoji,
        });

        let resp: serde_json::Value = client
            .post(format!("{base}/reactions.add"))
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
}
