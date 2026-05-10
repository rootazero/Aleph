use super::*;

impl SlackMessageOps {
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
            tracing::debug!("Slack: user {} not in user_allowlist, filtering", user_id);
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

}
