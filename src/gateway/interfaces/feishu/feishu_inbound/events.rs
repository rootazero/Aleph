use crate::gateway::interfaces::feishu::types::{
    ChatType, FeishuEvent, Mention, MessageEventPayload, TextContent, WsEventEnvelope,
};
use serde::Deserialize;

/// Parse a raw WebSocket text frame into a `FeishuEvent`.
///
/// Returns `Ok(None)` for ping/pong frames (handled separately).
/// Returns `Ok(Some(event))` for business events.
/// Returns `Err` if the JSON is malformed.
pub fn parse_ws_frame(raw: &str) -> Result<Option<FeishuEvent>, String> {
    let envelope: WsEventEnvelope =
        serde_json::from_str(raw).map_err(|e| format!("Failed to parse WS frame: {e}"))?;

    // Handle ping/pong frames (no header)
    if let Some(frame_type) = &envelope.frame_type {
        match frame_type.as_str() {
            "ping" | "pong" => return Ok(None),
            _ => {}
        }
    }

    let header = match &envelope.header {
        Some(h) => h,
        None => return Ok(Some(FeishuEvent::Unknown("no header".to_string()))),
    };

    let event_type = header.event_type.as_deref().unwrap_or("");

    match event_type {
        "im.message.receive_v1" => parse_message_event(&envelope),
        "card.action.trigger" => parse_card_action(&envelope),
        "im.chat.member.bot.added_v1" => parse_bot_added_event(&envelope),
        "im.chat.member.bot.deleted_v1" => parse_bot_removed_event(&envelope),
        "im.message.reaction.created_v1" => parse_reaction_created(&envelope),
        "im.message.reaction.deleted_v1" => parse_reaction_deleted(&envelope),
        "application.bot.menu_v6" => parse_bot_menu_event(&envelope),
        "drive.notice.comment_add_v1" => parse_drive_comment_event(&envelope),
        other => Ok(Some(FeishuEvent::Unknown(other.to_string()))),
    }
}

fn parse_message_event(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => {
            return Ok(Some(FeishuEvent::Unknown(
                "message event without body".to_string(),
            )))
        }
    };

    let payload: MessageEventPayload = serde_json::from_value(event_value.clone())
        .map_err(|e| format!("Failed to parse message event: {e}"))?;

    let message = payload
        .message
        .as_ref()
        .ok_or_else(|| "Missing message field".to_string())?;

    let sender_id = payload
        .sender
        .as_ref()
        .and_then(|s| s.sender_id.as_ref())
        .and_then(|sid| sid.open_id.clone())
        .unwrap_or_default();

    let chat_type = match message.chat_type.as_deref() {
        Some("p2p") => ChatType::P2p,
        _ => ChatType::Group,
    };

    let mentions = message
        .mentions
        .as_ref()
        .map(|ms| {
            ms.iter()
                .map(|m| Mention {
                    key: m.key.clone().unwrap_or_default(),
                    id: m
                        .id
                        .as_ref()
                        .and_then(|mid| mid.open_id.clone())
                        .unwrap_or_default(),
                    name: m.name.clone().unwrap_or_default(),
                    is_bot: false, // Will be determined by comparing with bot's open_id
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Some(FeishuEvent::MessageReceive {
        message_id: message.message_id.clone().unwrap_or_default(),
        chat_id: message.chat_id.clone().unwrap_or_default(),
        chat_type,
        sender_id,
        sender_name: None, // Feishu doesn't include sender name in event
        message_type: message.message_type.clone().unwrap_or_default(),
        content: message.content.clone().unwrap_or_default(),
        mentions,
        parent_id: message.parent_id.clone(),
        root_id: message.root_id.clone(),
    }))
}

fn parse_card_action(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => {
            return Ok(Some(FeishuEvent::Unknown(
                "card action without body".to_string(),
            )))
        }
    };

    let operator_id = event_value
        .pointer("/operator/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let chat_id = event_value
        .pointer("/context/open_chat_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let message_id = event_value
        .pointer("/context/open_message_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let action_value = event_value
        .pointer("/action/value")
        .map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        })
        .unwrap_or_default();

    if action_value.is_empty() {
        return Ok(Some(FeishuEvent::Unknown(
            "card action with empty value".to_string(),
        )));
    }

    Ok(Some(FeishuEvent::CardAction {
        chat_id,
        sender_id: operator_id,
        action_value,
        message_id,
    }))
}

fn parse_bot_added_event(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => {
            return Ok(Some(FeishuEvent::Unknown(
                "bot added event without body".to_string(),
            )))
        }
    };

    let chat_id = event_value
        .pointer("/chat_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let operator_id = event_value
        .pointer("/operator_id/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let operator_name = event_value
        .pointer("/operator_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if chat_id.is_empty() {
        return Ok(Some(FeishuEvent::Unknown(
            "bot added event without chat_id".to_string(),
        )));
    }

    Ok(Some(FeishuEvent::BotAdded {
        chat_id,
        operator_id,
        operator_name,
    }))
}

fn parse_bot_removed_event(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => {
            return Ok(Some(FeishuEvent::Unknown(
                "bot removed event without body".to_string(),
            )))
        }
    };

    let chat_id = event_value
        .pointer("/chat_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let operator_id = event_value
        .pointer("/operator_id/open_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if chat_id.is_empty() {
        return Ok(Some(FeishuEvent::Unknown(
            "bot removed event without chat_id".to_string(),
        )));
    }

    Ok(Some(FeishuEvent::BotRemoved {
        chat_id,
        operator_id,
    }))
}

fn parse_reaction_created(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => {
            return Ok(Some(FeishuEvent::Unknown(
                "reaction created event without body".to_string(),
            )))
        }
    };

    let message_id = event_value
        .pointer("/message_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let chat_id = event_value
        .pointer("/chat_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let emoji = event_value
        .pointer("/reaction_type/emoji_type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let operator_id = event_value
        .pointer("/user_id/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    if message_id.is_empty() || emoji.is_empty() {
        return Ok(Some(FeishuEvent::Unknown(
            "reaction created event missing message_id or emoji".to_string(),
        )));
    }

    Ok(Some(FeishuEvent::ReactionCreated {
        message_id,
        chat_id,
        emoji,
        operator_id,
    }))
}

fn parse_reaction_deleted(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => {
            return Ok(Some(FeishuEvent::Unknown(
                "reaction deleted event without body".to_string(),
            )))
        }
    };

    let message_id = event_value
        .pointer("/message_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let chat_id = event_value
        .pointer("/chat_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let emoji = event_value
        .pointer("/reaction_type/emoji_type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let operator_id = event_value
        .pointer("/user_id/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let reaction_id = event_value
        .pointer("/reaction_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if message_id.is_empty() || emoji.is_empty() {
        return Ok(Some(FeishuEvent::Unknown(
            "reaction deleted event missing message_id or emoji".to_string(),
        )));
    }

    Ok(Some(FeishuEvent::ReactionDeleted {
        message_id,
        chat_id,
        emoji,
        operator_id,
        reaction_id,
    }))
}

fn parse_bot_menu_event(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => {
            return Ok(Some(FeishuEvent::Unknown(
                "bot menu event without body".to_string(),
            )))
        }
    };

    let event_key = event_value
        .pointer("/event_key")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let operator_id = event_value
        .pointer("/operator/operator_id/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let operator_name = event_value
        .pointer("/operator/operator_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Timestamp can be string or number in JSON
    let timestamp = event_value
        .pointer("/timestamp")
        .and_then(|v| {
            if let Some(n) = v.as_i64() {
                Some(n)
            } else if let Some(s) = v.as_str() {
                s.parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    if event_key.is_empty() || operator_id.is_empty() {
        return Ok(Some(FeishuEvent::Unknown(
            "bot menu event missing event_key or operator_id".to_string(),
        )));
    }

    Ok(Some(FeishuEvent::BotMenu {
        event_key,
        operator_id,
        operator_name,
        timestamp,
    }))
}

fn parse_drive_comment_event(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => {
            return Ok(Some(FeishuEvent::Unknown(
                "drive comment event without body".to_string(),
            )))
        }
    };

    let event_id = envelope
        .header
        .as_ref()
        .and_then(|h| h.event_id.as_ref())
        .cloned()
        .unwrap_or_default();

    let comment_id = event_value
        .pointer("/comment_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let reply_id = event_value
        .pointer("/reply_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let file_type = event_value
        .pointer("/notice_meta/file_type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let file_token = event_value
        .pointer("/notice_meta/file_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let from_user_id = event_value
        .pointer("/notice_meta/from_user_id/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let mentioned = event_value
        .pointer("/is_mentioned")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let content = event_value
        .pointer("/comment/text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if comment_id.is_empty() || file_token.is_empty() {
        return Ok(Some(FeishuEvent::Unknown(
            "drive comment event missing comment_id or file_token".to_string(),
        )));
    }

    Ok(Some(FeishuEvent::DriveComment {
        event_id,
        comment_id,
        reply_id,
        file_type,
        file_token,
        from_user_id,
        mentioned,
        content,
    }))
}

/// Extract text from a Feishu message content JSON string.
///
/// For "text" type: parses `{"text": "..."}` and returns the text.
/// Removes bot mention placeholders from the text.
#[must_use]
pub fn extract_text_content(content: &str, mentions: &[Mention]) -> Option<String> {
    let parsed: TextContent = serde_json::from_str(content).ok()?;
    let mut text = parsed.text?;

    // Remove bot mention placeholders (e.g., "@_user_1 ")
    for mention in mentions {
        if mention.is_bot {
            // Remove the placeholder and any trailing space
            text = text.replace(&format!("{} ", mention.key), "");
            text = text.replace(&mention.key, "");
        }
    }

    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Mark mentions that refer to the bot, given the bot's `open_id`.
pub fn mark_bot_mentions(mentions: &mut [Mention], bot_open_id: &str) {
    for mention in mentions.iter_mut() {
        if mention.id == bot_open_id {
            mention.is_bot = true;
        }
    }
}

/// Flatten a Feishu `post` (rich-text) message body into readable plain text.
///
/// Handles both the direct `{title, content}` shape and locale-wrapped shapes
/// (`{post:{...}}` or `{zh_cn:{...}, en_us:{...}}`). Falls back to a placeholder
/// when the payload is not a recognizable post.
#[must_use]
pub fn parse_post_content(content: &str) -> String {
    const FALLBACK: &str = "[Rich text message]";
    let parsed: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return FALLBACK.to_string(),
    };
    let Some((title, paragraphs)) = resolve_post_payload(&parsed) else {
        return FALLBACK.to_string();
    };

    let mut blocks: Vec<String> = Vec::new();
    let title = title.trim();
    if !title.is_empty() {
        blocks.push(title.to_string());
    }

    let mut lines: Vec<String> = Vec::new();
    for paragraph in paragraphs {
        if let Some(elements) = paragraph.as_array() {
            let mut line = String::new();
            for element in elements {
                line.push_str(&render_post_element(element));
            }
            lines.push(line);
        }
    }
    let body = lines.join("\n");
    let body = body.trim();
    if !body.is_empty() {
        blocks.push(body.to_string());
    }

    let out = blocks.join("\n\n").trim().to_string();
    if out.is_empty() {
        FALLBACK.to_string()
    } else {
        out
    }
}

/// Resolve a post payload `(title, content_paragraphs)` from any supported shape.
fn resolve_post_payload(v: &serde_json::Value) -> Option<(String, &Vec<serde_json::Value>)> {
    if let Some(content) = v.get("content").and_then(|c| c.as_array()) {
        let title = v
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        return Some((title, content));
    }
    if let Some(post) = v.get("post") {
        if let Some(found) = resolve_post_payload(post) {
            return Some(found);
        }
    }
    if let Some(obj) = v.as_object() {
        for val in obj.values() {
            if let Some(content) = val.get("content").and_then(|c| c.as_array()) {
                let title = val
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                return Some((title, content));
            }
        }
    }
    None
}

/// Render a single post element node into plain text.
fn render_post_element(element: &serde_json::Value) -> String {
    let tag = element.get("tag").and_then(|t| t.as_str()).unwrap_or("");
    let field = |key: &str| element.get(key).and_then(|v| v.as_str()).unwrap_or("");
    match tag {
        "text" | "md" | "lark_md" => field("text").to_string(),
        "a" => {
            let text = field("text");
            let href = field("href");
            match (text.is_empty(), href.is_empty()) {
                (false, false) => format!("{text} ({href})"),
                (true, false) => href.to_string(),
                _ => text.to_string(),
            }
        }
        "at" => {
            let name = [field("user_name"), field("user_id"), field("open_id")]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or("");
            if name.is_empty() {
                String::new()
            } else {
                format!("@{name}")
            }
        }
        "img" => "[image]".to_string(),
        "media" => "[media]".to_string(),
        "emotion" => field("emoji").to_string(),
        "code_block" | "pre" => field("text").to_string(),
        "hr" => "---".to_string(),
        _ => field("text").to_string(),
    }
}

#[cfg(test)]
mod post_content_tests {
    use super::parse_post_content;

    #[test]
    fn direct_post_with_title() {
        let c = r#"{"title":"Daily","content":[[{"tag":"text","text":"line one"}],[{"tag":"text","text":"line two"}]]}"#;
        assert_eq!(parse_post_content(c), "Daily\n\nline one\nline two");
    }

    #[test]
    fn locale_wrapped_post() {
        let c = r#"{"zh_cn":{"title":"标题","content":[[{"tag":"text","text":"正文"}]]}}"#;
        assert_eq!(parse_post_content(c), "标题\n\n正文");
    }

    #[test]
    fn renders_links_and_mentions() {
        let c = r#"{"title":"","content":[[{"tag":"text","text":"see "},{"tag":"a","text":"docs","href":"https://x"},{"tag":"text","text":" and "},{"tag":"at","user_name":"Bob"}]]}"#;
        assert_eq!(parse_post_content(c), "see docs (https://x) and @Bob");
    }

    #[test]
    fn fallback_on_invalid() {
        assert_eq!(parse_post_content("not json"), "[Rich text message]");
        assert_eq!(parse_post_content("{}"), "[Rich text message]");
    }
}

#[must_use]
pub fn parse_merge_forward_content(content: &str) -> String {
    #[derive(Debug, Deserialize)]
    struct MergeForwardItem {
        msg_type: Option<String>,
        body: Option<MergeForwardBody>,
        upper_message_id: Option<String>,
        create_time: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct MergeForwardBody {
        content: Option<String>,
    }

    let items: Vec<MergeForwardItem> = match serde_json::from_str(content) {
        Ok(items) => items,
        Err(_) => return "[Merged and Forwarded Message - parse error]".to_string(),
    };

    let sub_messages: Vec<&MergeForwardItem> = items
        .iter()
        .filter(|item| item.upper_message_id.is_some())
        .collect();

    if sub_messages.is_empty() {
        return "[Merged and Forwarded Message - no sub-messages]".to_string();
    }

    let mut sorted: Vec<&&MergeForwardItem> = sub_messages.iter().collect();
    sorted.sort_by_key(|item| {
        item.create_time
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or(0)
    });

    let max_messages = 50;
    let mut lines = vec!["[Merged and Forwarded Messages]".to_string()];

    for item in sorted.iter().take(max_messages) {
        let item_text = match (item.msg_type.as_deref(), &item.body) {
            (Some("text"), Some(body)) => {
                if let Some(c) = &body.content {
                    if let Ok(tc) = serde_json::from_str::<TextContent>(c) {
                        tc.text.unwrap_or_else(|| "[Text]".to_string())
                    } else {
                        "[Text]".to_string()
                    }
                } else {
                    "[Text]".to_string()
                }
            }
            (Some("post"), Some(body)) => {
                if let Some(c) = &body.content {
                    c.lines().take(3).collect::<Vec<_>>().join(" ")
                } else {
                    "[Post]".to_string()
                }
            }
            (Some("image"), _) => "[Image]".to_string(),
            (Some("file"), Some(body)) => {
                let name = if let Some(c) = &body.content {
                    match serde_json::from_str::<serde_json::Value>(c) {
                        Ok(v) => v
                            .get("file_name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        Err(_) => "unknown".to_string(),
                    }
                } else {
                    "unknown".to_string()
                };
                format!("[File: {name}]")
            }
            (Some("audio"), _) => "[Audio]".to_string(),
            (Some("video"), _) => "[Video]".to_string(),
            (Some("sticker"), _) => "[Sticker]".to_string(),
            (Some("merge_forward"), _) => "[Nested Merged Forward]".to_string(),
            (Some(msg_type), _) => format!("[{msg_type}]"),
            _ => "[Unknown content]".to_string(),
        };
        lines.push(format!("- {item_text}"));
    }

    if sorted.len() > max_messages {
        lines.push(format!(
            "... and {} more messages",
            sorted.len() - max_messages
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ping_frame() {
        let raw = r#"{"type": "ping"}"#;
        let result = parse_ws_frame(raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_pong_frame() {
        let raw = r#"{"type": "pong"}"#;
        let result = parse_ws_frame(raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_unknown_event() {
        let raw = r#"{
            "header": {"event_id": "e1", "event_type": "im.chat.created_v1", "token": "t"},
            "event": {}
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::Unknown(t) => assert_eq!(t, "im.chat.created_v1"),
            _ => panic!("Expected Unknown event"),
        }
    }

    #[test]
    fn test_parse_message_receive_p2p() {
        let raw = r#"{
            "header": {
                "event_id": "evt_123",
                "event_type": "im.message.receive_v1",
                "token": "tok",
                "create_time": "1700000000"
            },
            "event": {
                "sender": {
                    "sender_id": {"open_id": "ou_user1"},
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "msg_001",
                    "chat_id": "oc_chat1",
                    "chat_type": "p2p",
                    "message_type": "text",
                    "content": "{\"text\":\"Hello\"}",
                    "mentions": null,
                    "parent_id": null
                }
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::MessageReceive {
                message_id,
                chat_id,
                chat_type,
                sender_id,
                message_type,
                content,
                mentions,
                parent_id,
                ..
            } => {
                assert_eq!(message_id, "msg_001");
                assert_eq!(chat_id, "oc_chat1");
                assert_eq!(chat_type, ChatType::P2p);
                assert_eq!(sender_id, "ou_user1");
                assert_eq!(message_type, "text");
                assert_eq!(content, "{\"text\":\"Hello\"}");
                assert!(mentions.is_empty());
                assert!(parent_id.is_none());
            }
            _ => panic!("Expected MessageReceive"),
        }
    }

    #[test]
    fn test_parse_message_receive_group_with_mention() {
        let raw = r#"{
            "header": {
                "event_id": "evt_456",
                "event_type": "im.message.receive_v1",
                "token": "tok"
            },
            "event": {
                "sender": {
                    "sender_id": {"open_id": "ou_user2"},
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "msg_002",
                    "chat_id": "oc_group1",
                    "chat_type": "group",
                    "message_type": "text",
                    "content": "{\"text\":\"@_user_1 What is Rust?\"}",
                    "mentions": [
                        {"key": "@_user_1", "id": {"open_id": "ou_bot"}, "name": "Aleph"}
                    ],
                    "parent_id": "msg_001"
                }
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::MessageReceive {
                chat_type,
                mentions,
                parent_id,
                ..
            } => {
                assert_eq!(chat_type, ChatType::Group);
                assert_eq!(mentions.len(), 1);
                assert_eq!(mentions[0].key, "@_user_1");
                assert_eq!(mentions[0].id, "ou_bot");
                assert_eq!(mentions[0].name, "Aleph");
                assert_eq!(parent_id, Some("msg_001".to_string()));
            }
            _ => panic!("Expected MessageReceive"),
        }
    }

    #[test]
    fn test_extract_text_content_plain() {
        let content = r#"{"text": "Hello world"}"#;
        let result = extract_text_content(content, &[]);
        assert_eq!(result, Some("Hello world".to_string()));
    }

    #[test]
    fn test_extract_text_content_with_bot_mention() {
        let content = r#"{"text": "@_user_1 What is Rust?"}"#;
        let mentions = vec![Mention {
            key: "@_user_1".into(),
            id: "ou_bot".into(),
            name: "Aleph".into(),
            is_bot: true,
        }];
        let result = extract_text_content(content, &mentions);
        assert_eq!(result, Some("What is Rust?".to_string()));
    }

    #[test]
    fn test_extract_text_content_only_mention() {
        let content = r#"{"text": "@_user_1"}"#;
        let mentions = vec![Mention {
            key: "@_user_1".into(),
            id: "ou_bot".into(),
            name: "Aleph".into(),
            is_bot: true,
        }];
        let result = extract_text_content(content, &mentions);
        assert!(result.is_none());
    }

    #[test]
    fn test_mark_bot_mentions() {
        let mut mentions = vec![
            Mention {
                key: "@_user_1".into(),
                id: "ou_bot".into(),
                name: "Bot".into(),
                is_bot: false,
            },
            Mention {
                key: "@_user_2".into(),
                id: "ou_human".into(),
                name: "Human".into(),
                is_bot: false,
            },
        ];
        mark_bot_mentions(&mut mentions, "ou_bot");
        assert!(mentions[0].is_bot);
        assert!(!mentions[1].is_bot);
    }

    #[test]
    fn test_parse_card_action() {
        let raw = r#"{
            "header": {"event_id": "e1", "event_type": "card.action.trigger", "token": "t"},
            "event": {
                "operator": {"open_id": "ou_user1"},
                "action": {"value": "start_conversation"},
                "context": {"open_chat_id": "oc_chat1", "open_message_id": "msg_card1"}
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::CardAction {
                chat_id,
                sender_id,
                action_value,
                message_id,
            } => {
                assert_eq!(chat_id, "oc_chat1");
                assert_eq!(sender_id, "ou_user1");
                assert_eq!(action_value, "start_conversation");
                assert_eq!(message_id, "msg_card1");
            }
            _ => panic!("Expected CardAction"),
        }
    }

    #[test]
    fn test_parse_invalid_json() {
        let raw = "not json at all";
        let result = parse_ws_frame(raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_bot_added_event() {
        let raw = r#"{
            "header": {"event_id": "e1", "event_type": "im.chat.member.bot.added_v1", "token": "t"},
            "event": {
                "chat_id": "oc_chat1",
                "operator_id": {"open_id": "ou_user1"},
                "operator_name": "TestUser"
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::BotAdded {
                chat_id,
                operator_id,
                operator_name,
            } => {
                assert_eq!(chat_id, "oc_chat1");
                assert_eq!(operator_id, "ou_user1");
                assert_eq!(operator_name, Some("TestUser".to_string()));
            }
            _ => panic!("Expected BotAdded event"),
        }
    }

    #[test]
    fn test_parse_bot_removed_event() {
        let raw = r#"{
            "header": {"event_id": "e2", "event_type": "im.chat.member.bot.deleted_v1", "token": "t"},
            "event": {
                "chat_id": "oc_chat1",
                "operator_id": {"open_id": "ou_user2"}
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::BotRemoved {
                chat_id,
                operator_id,
            } => {
                assert_eq!(chat_id, "oc_chat1");
                assert_eq!(operator_id, Some("ou_user2".to_string()));
            }
            _ => panic!("Expected BotRemoved event"),
        }
    }

    #[test]
    fn test_parse_reaction_created_event() {
        let raw = r#"{
            "header": {"event_id": "e3", "event_type": "im.message.reaction.created_v1", "token": "t"},
            "event": {
                "message_id": "msg_001",
                "chat_id": "oc_chat1",
                "reaction_type": {"emoji_type": " thumbs_up"},
                "user_id": {"open_id": "ou_user1"}
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::ReactionCreated {
                message_id,
                chat_id,
                emoji,
                operator_id,
            } => {
                assert_eq!(message_id, "msg_001");
                assert_eq!(chat_id, Some("oc_chat1".to_string()));
                assert_eq!(emoji, " thumbs_up");
                assert_eq!(operator_id, "ou_user1");
            }
            _ => panic!("Expected ReactionCreated event"),
        }
    }

    #[test]
    fn test_parse_reaction_deleted_event() {
        let raw = r#"{
            "header": {"event_id": "e4", "event_type": "im.message.reaction.deleted_v1", "token": "t"},
            "event": {
                "message_id": "msg_001",
                "chat_id": "oc_chat1",
                "reaction_type": {"emoji_type": " thumbs_up"},
                "user_id": {"open_id": "ou_user1"},
                "reaction_id": "reaction_123"
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::ReactionDeleted {
                message_id,
                chat_id,
                emoji,
                operator_id,
                reaction_id,
            } => {
                assert_eq!(message_id, "msg_001");
                assert_eq!(chat_id, Some("oc_chat1".to_string()));
                assert_eq!(emoji, " thumbs_up");
                assert_eq!(operator_id, "ou_user1");
                assert_eq!(reaction_id, Some("reaction_123".to_string()));
            }
            _ => panic!("Expected ReactionDeleted event"),
        }
    }

    #[test]
    fn test_parse_bot_menu_event() {
        let raw = r#"{
            "header": {"event_id": "e5", "event_type": "application.bot.menu_v6", "token": "t"},
            "event": {
                "event_key": "menu_settings",
                "timestamp": "1700000000000",
                "operator": {
                    "operator_name": "TestUser",
                    "operator_id": {"open_id": "ou_user1", "user_id": "user_1", "union_id": "union_1"}
                }
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::BotMenu {
                event_key,
                operator_id,
                operator_name,
                timestamp,
            } => {
                assert_eq!(event_key, "menu_settings");
                assert_eq!(operator_id, "ou_user1");
                assert_eq!(operator_name, Some("TestUser".to_string()));
                assert_eq!(timestamp, 1700000000000);
            }
            _ => panic!("Expected BotMenu event"),
        }
    }

    #[test]
    fn test_parse_drive_comment_event() {
        let raw = r#"{
            "header": {"event_id": "evt_123", "event_type": "drive.notice.comment_add_v1", "token": "t"},
            "event": {
                "event_id": "evt_123",
                "comment_id": "comment_001",
                "reply_id": "reply_001",
                "notice_meta": {
                    "notice_type": "add_comment",
                    "file_type": "docx",
                    "file_token": "file_token_abc",
                    "from_user_id": {"open_id": "ou_user1"}
                },
                "is_mentioned": true,
                "comment": {"text": "This is a comment"}
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::DriveComment {
                event_id,
                comment_id,
                reply_id,
                file_type,
                file_token,
                from_user_id,
                mentioned,
                content,
            } => {
                assert_eq!(event_id, "evt_123");
                assert_eq!(comment_id, "comment_001");
                assert_eq!(reply_id, Some("reply_001".to_string()));
                assert_eq!(file_type, "docx");
                assert_eq!(file_token, "file_token_abc");
                assert_eq!(from_user_id, "ou_user1");
                assert!(mentioned);
                assert_eq!(content, Some("This is a comment".to_string()));
            }
            _ => panic!("Expected DriveComment event"),
        }
    }
}
