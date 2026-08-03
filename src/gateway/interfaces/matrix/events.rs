use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, MessageMeta, UserId,
};
use crate::gateway::interfaces::matrix::config::MatrixConfig;
use chrono::Utc;

/// Convert a Matrix timeline event JSON into an `InboundMessage`.
///
/// Returns `None` for events that should be ignored (own messages, filtered rooms,
/// unsupported types, policy violations, duplicates).
pub fn convert_timeline_event(
    event: &serde_json::Value,
    room_id: &str,
    channel_id: &ChannelId,
    own_user_id: &str,
    config: Option<&MatrixConfig>,
) -> Option<InboundMessage> {
    let event_type = event["type"].as_str()?;

    match event_type {
        "m.room.message" => convert_room_message(event, room_id, channel_id, own_user_id, config),
        "m.reaction" => convert_reaction(event, room_id, channel_id, own_user_id, config),
        "m.room.encrypted" => {
            tracing::warn!("Skipping encrypted Matrix event (E2EE not enabled in Phase 1)");
            None
        }
        _ => None,
    }
}

fn convert_room_message(
    event: &serde_json::Value,
    room_id: &str,
    channel_id: &ChannelId,
    own_user_id: &str,
    config: Option<&MatrixConfig>,
) -> Option<InboundMessage> {
    let sender = event["sender"].as_str()?;

    if sender == own_user_id {
        return None;
    }

    if let Some(cfg) = config {
        if !cfg.is_user_allowed_in_room(sender, room_id) {
            return None;
        }
    }

    let content = &event["content"];
    let msgtype = content["msgtype"].as_str().unwrap_or("");

    let (text, attachments, metadata) = match msgtype {
        "m.text" | "m.notice" => {
            let body = content["body"].as_str().unwrap_or("");
            (body.to_string(), Vec::new(), Vec::new())
        }
        "m.image" | "m.audio" | "m.video" | "m.file" => {
            let body = content["body"].as_str().unwrap_or("attachment");
            let url = content["url"].as_str().map(|s| s.to_string());
            let mime = guess_mime_from_msgtype(msgtype, content["info"]["mimetype"].as_str());
            let filename = content["body"].as_str().map(|s| s.to_string());
            let attachment = Attachment {
                id: url
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                mime_type: mime,
                filename,
                size: content["info"]["size"].as_u64(),
                url,
                path: None,
                data: None,
            };
            (body.to_string(), vec![attachment], Vec::new())
        }
        "m.location" => {
            let body = content["body"].as_str().unwrap_or("");
            let geo_uri = content["geo_uri"].as_str().unwrap_or("");
            let text = if geo_uri.is_empty() {
                body.to_string()
            } else {
                format!("{body} (location: {geo_uri})")
            };
            (text, Vec::new(), Vec::new())
        }
        _ => {
            let body = content["body"].as_str().unwrap_or("");
            (body.to_string(), Vec::new(), Vec::new())
        }
    };

    if text.is_empty() && attachments.is_empty() {
        return None;
    }

    if let Some(cfg) = config {
        if !cfg.check_mention(&text, own_user_id, room_id) {
            return None;
        }
    }

    let event_id = event["event_id"].as_str().unwrap_or("").to_string();

    let timestamp = event["origin_server_ts"]
        .as_i64()
        .and_then(|ms| {
            chrono::DateTime::from_timestamp(ms / 1000, ((ms % 1000) * 1_000_000) as u32)
        })
        .unwrap_or_else(Utc::now);

    let relates_to = content["m.relates_to"].as_object();
    let reply_to = relates_to
        .and_then(|r| r.get("m.in_reply_to"))
        .and_then(|ir| ir.get("event_id"))
        .and_then(|eid| eid.as_str())
        .map(|id| MessageId::new(id.to_string()));

    Some(InboundMessage {
        id: MessageId::new(event_id),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(room_id.to_string()),
        sender_id: UserId::new(sender.to_string()),
        sender_name: Some(sender.to_string()),
        text,
        attachments,
        timestamp,
        reply_to,
        is_group: true,
        raw: Some(event.clone()),
        metadata,
    })
}

fn convert_reaction(
    event: &serde_json::Value,
    room_id: &str,
    channel_id: &ChannelId,
    own_user_id: &str,
    config: Option<&MatrixConfig>,
) -> Option<InboundMessage> {
    let sender = event["sender"].as_str()?;

    if sender == own_user_id {
        return None;
    }

    if let Some(cfg) = config {
        if !cfg.is_user_allowed_in_room(sender, room_id) {
            return None;
        }
    }

    let content = &event["content"];
    let relates_to = content["m.relates_to"].as_object()?;
    let rel_type = relates_to["rel_type"].as_str()?;
    if rel_type != "m.annotation" {
        return None;
    }

    let target_event_id = relates_to["event_id"].as_str()?.to_string();
    let emoji = relates_to["key"].as_str()?.to_string();

    let event_id = event["event_id"].as_str().unwrap_or("").to_string();
    let timestamp = event["origin_server_ts"]
        .as_i64()
        .and_then(|ms| {
            chrono::DateTime::from_timestamp(ms / 1000, ((ms % 1000) * 1_000_000) as u32)
        })
        .unwrap_or_else(Utc::now);

    Some(InboundMessage {
        id: MessageId::new(event_id),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(room_id.to_string()),
        sender_id: UserId::new(sender.to_string()),
        sender_name: Some(sender.to_string()),
        text: emoji.clone(),
        attachments: Vec::new(),
        timestamp,
        reply_to: Some(MessageId::new(target_event_id)),
        is_group: true,
        raw: Some(event.clone()),
        metadata: vec![MessageMeta::Reaction {
            emojis: vec![emoji],
        }],
    })
}

fn guess_mime_from_msgtype(msgtype: &str, info_mime: Option<&str>) -> String {
    if let Some(m) = info_mime {
        return m.to_string();
    }
    match msgtype {
        "m.image" => "image/jpeg".to_string(),
        "m.audio" => "audio/mpeg".to_string(),
        "m.video" => "video/mp4".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_basic_message() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$event123",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello from Matrix!"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert_eq!(msg.channel_id.as_str(), "matrix");
        assert_eq!(msg.conversation_id.as_str(), "!room1:matrix.org");
        assert_eq!(msg.sender_id.as_str(), "@user:matrix.org");
        assert_eq!(msg.text, "Hello from Matrix!");
        assert_eq!(msg.id.as_str(), "$event123");
        assert!(msg.is_group);
    }

    #[test]
    fn test_convert_filters_own_messages() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@bot:matrix.org",
            "event_id": "$event456",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "My own message"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_skips_non_message_events() {
        let event = serde_json::json!({
            "type": "m.room.member",
            "sender": "@user:matrix.org",
            "event_id": "$event789",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "membership": "join"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_with_reply_to() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$reply_event",
            "origin_server_ts": 1700000001000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "This is a reply",
                "m.relates_to": {
                    "m.in_reply_to": {
                        "event_id": "$original_event"
                    }
                }
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "$original_event");
    }

    #[test]
    fn test_convert_reaction_event() {
        let event = serde_json::json!({
            "type": "m.reaction",
            "sender": "@user:matrix.org",
            "event_id": "$react1",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "m.relates_to": {
                    "rel_type": "m.annotation",
                    "event_id": "$target1",
                    "key": "👍"
                }
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert_eq!(msg.text, "👍");
        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "$target1");
        let has_reaction = msg.metadata.iter().any(
            |m| matches!(m, MessageMeta::Reaction { emojis } if *emojis == vec!["👍".to_string()]),
        );
        assert!(has_reaction);
    }

    #[test]
    fn test_convert_image_attachment() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$img1",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.image",
                "body": "photo.jpg",
                "url": "mxc://example.com/abc123",
                "info": {
                    "mimetype": "image/jpeg",
                    "size": 12345
                }
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert_eq!(msg.attachments.len(), 1);
        let att = &msg.attachments[0];
        assert_eq!(att.mime_type, "image/jpeg");
        assert_eq!(att.url.as_deref(), Some("mxc://example.com/abc123"));
        assert_eq!(att.size, Some(12345));
    }

    #[test]
    fn test_convert_location() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$loc1",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.location",
                "body": "Office",
                "geo_uri": "geo:37.7749,-122.4194"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert!(msg.text.contains("Office"));
        assert!(msg.text.contains("geo:37.7749,-122.4194"));
    }

    #[test]
    fn test_convert_policy_user_allowlist() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@stranger:matrix.org",
            "event_id": "$event_blocked",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello from stranger!"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let config = MatrixConfig {
            allowed_users: vec!["@allowed:matrix.org".to_string()],
            ..Default::default()
        };

        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            Some(&config),
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_policy_mention_gating() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$event_no_mention",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello without mention"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let config = MatrixConfig {
            mention_gating: true,
            ..Default::default()
        };

        let msg = convert_timeline_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            Some(&config),
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_per_room_mention_override() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$e_room_gate",
            "origin_server_ts": 1700000000000_i64,
            "content": { "msgtype": "m.text", "body": "Hello without mention" }
        });
        let channel_id = ChannelId::new("matrix");

        // Global gating OFF, but the strict room demands a mention.
        let mut config = MatrixConfig::default();
        config.rooms.insert(
            "!strict:matrix.org".to_string(),
            crate::gateway::interfaces::matrix::config::MatrixRoomPolicy {
                require_mention: Some(true),
                allow_bots: None,
                users: None,
            },
        );

        // Unmentioned message is gated out only in the strict room.
        assert!(convert_timeline_event(
            &event,
            "!strict:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            Some(&config),
        )
        .is_none());
        assert!(convert_timeline_event(
            &event,
            "!other:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            Some(&config),
        )
        .is_some());
    }
}
