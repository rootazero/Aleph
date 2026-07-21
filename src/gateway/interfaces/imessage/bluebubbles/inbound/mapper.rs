//! Map a BlueBubbles webhook/message JSON object to a transport-neutral struct.

use crate::gateway::interfaces::imessage::reaction;

// `updated-message` is intentionally absent: the channel no longer subscribes
// to it (edits reuse the original GUID and get deduped, so they were inert) —
// see `api::register_webhook`. `message` stays as a defensive alias for servers
// that emit the bare type.
const MESSAGE_EVENTS: &[&str] = &["new-message", "message"];

/// A BlueBubbles message reduced to the fields Aleph needs.
#[derive(Debug, Clone)]
pub struct MappedMessage {
    pub guid: String,
    pub chat_guid: String,
    pub sender: String,
    pub text: String,
    pub is_group: bool,
    pub is_from_me: bool,
    /// True for any tapback record (add *or* remove).
    pub is_tapback: bool,
    /// Display emoji when this is a surface-able *add* tapback (removes stay
    /// `None`); drives whether the reaction is routed and how it renders.
    pub reaction_emoji: Option<String>,
    pub reply_to: Option<String>,
    /// (attachment_guid, mime_type) pairs to download.
    pub attachment_guids: Vec<(String, String)>,
}

impl MappedMessage {
    /// Whether this record should be routed to the inbound pipeline. Drops our
    /// own echoes, GUID-less records, and *remove* tapbacks (a tapback with no
    /// surface-able emoji), while letting normal messages and *add* tapbacks
    /// through. Group mention-gating is enforced downstream in the central
    /// permission layer, not here.
    #[must_use]
    pub fn is_routable(&self) -> bool {
        !self.is_from_me
            && !self.guid.is_empty()
            && (!self.is_tapback || self.reaction_emoji.is_some())
    }
}

fn first_str<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()))
}

/// Returns `None` for non-message events or unparseable payloads.
pub fn map_webhook_record(payload: &serde_json::Value) -> Option<MappedMessage> {
    let event = first_str(payload, &["type", "event"]).unwrap_or("");
    if !event.is_empty() && !MESSAGE_EVENTS.contains(&event) {
        return None;
    }
    let data = payload
        .get("data")
        .and_then(|d| {
            if d.is_object() {
                Some(d.clone())
            } else {
                d.as_array()
                    .and_then(|a| a.iter().find(|x| x.is_object()).cloned())
            }
        })
        .unwrap_or_else(|| payload.clone());

    let guid = first_str(&data, &["guid", "messageGuid", "id"])?.to_string();
    let text = first_str(&data, &["text", "message", "body"])
        .unwrap_or("")
        .to_string();
    let is_from_me = data
        .get("isFromMe")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // `associatedMessageType` arrives as either an integer (2000) or a
    // BlueBubbles string ("love"); the shared reaction module handles both.
    let assoc = data.get("associatedMessageType");
    let is_tapback = assoc.is_some_and(reaction::is_reaction_type);
    let reaction_emoji = assoc
        .and_then(reaction::reaction_emoji)
        .map(str::to_string);

    let chat_guid = first_str(&data, &["chatGuid", "chat_guid"])
        .map(str::to_string)
        .or_else(|| {
            data.get("chats")
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|c| c.get("guid").and_then(|g| g.as_str()))
                .map(str::to_string)
        })
        .unwrap_or_default();

    let sender = data
        .get("handle")
        .and_then(|h| h.get("address"))
        .and_then(|a| a.as_str())
        .or_else(|| first_str(&data, &["sender", "from", "address"]))
        .unwrap_or(&chat_guid)
        .to_string();

    let is_group = data
        .get("isGroup")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || chat_guid.contains(";+;");

    let reply_to =
        first_str(&data, &["threadOriginatorGuid", "associatedMessageGuid"]).map(str::to_string);

    let attachment_guids = data
        .get("attachments")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|att| {
                    let g = att.get("guid").and_then(|g| g.as_str())?.to_string();
                    let mime = att
                        .get("mimeType")
                        .and_then(|m| m.as_str())
                        .unwrap_or("application/octet-stream")
                        .to_string();
                    Some((g, mime))
                })
                .collect()
        })
        .unwrap_or_default();

    Some(MappedMessage {
        guid,
        chat_guid,
        sender,
        text,
        is_group,
        is_from_me,
        is_tapback,
        reaction_emoji,
        reply_to,
        attachment_guids,
    })
}

/// Build an `InboundMessage` from a mapped record (attachments already downloaded).
///
/// An *add* tapback is rendered like the Telegram/Matrix reaction convention —
/// `text = "Reacted with: <emoji>"` plus a [`MessageMeta::Reaction`] carrying the
/// emoji — so it surfaces as a context signal against the reacted-to message
/// (`reply_to`), and the model decides whether it warrants a reply.
pub fn to_inbound(
    m: &MappedMessage,
    attachments: Vec<crate::gateway::channel::Attachment>,
) -> crate::gateway::channel::InboundMessage {
    use crate::gateway::channel::{
        ChannelId, ConversationId, InboundMessage, MessageId, MessageMeta, UserId,
    };
    let (text, metadata) = match &m.reaction_emoji {
        Some(emoji) => (
            format!("Reacted with: {emoji}"),
            vec![MessageMeta::Reaction {
                emojis: vec![emoji.clone()],
            }],
        ),
        None => (m.text.clone(), vec![]),
    };
    InboundMessage {
        id: MessageId::new(&m.guid),
        channel_id: ChannelId::new("imessage"),
        conversation_id: ConversationId::new(&m.chat_guid),
        sender_id: UserId::new(&m.sender),
        sender_name: None,
        text,
        attachments,
        timestamp: chrono::Utc::now(),
        reply_to: m.reply_to.as_deref().map(MessageId::new),
        is_group: m.is_group,
        raw: None,
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(extra: serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "type": "new-message",
            "data": {
                "guid": "msg-1",
                "text": "hello",
                "isFromMe": false,
                "chatGuid": "iMessage;-;+15551234567",
                "handle": { "address": "+15551234567" }
            }
        });
        base["data"]
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        base
    }

    #[test]
    fn maps_basic_dm() {
        let m = map_webhook_record(&record(serde_json::json!({}))).unwrap();
        assert_eq!(m.guid, "msg-1");
        assert_eq!(m.text, "hello");
        assert_eq!(m.sender, "+15551234567");
        assert_eq!(m.chat_guid, "iMessage;-;+15551234567");
        assert!(!m.is_group);
        assert!(!m.is_from_me);
        assert!(!m.is_tapback);
    }

    #[test]
    fn skips_from_me() {
        let m = map_webhook_record(&record(serde_json::json!({ "isFromMe": true }))).unwrap();
        assert!(m.is_from_me);
    }

    #[test]
    fn flags_tapback() {
        let m = map_webhook_record(&record(
            serde_json::json!({ "associatedMessageType": 2000 }),
        ))
        .unwrap();
        assert!(m.is_tapback);
    }

    #[test]
    fn add_tapback_surfaces_as_reaction() {
        // Integer form (2000 = love) and the reacted-to message reference.
        let m = map_webhook_record(&record(serde_json::json!({
            "associatedMessageType": 2000,
            "associatedMessageGuid": "target-guid",
        })))
        .unwrap();
        assert!(m.is_tapback);
        assert_eq!(m.reaction_emoji.as_deref(), Some("❤️"));
        assert!(m.is_routable(), "add tapbacks route to the pipeline");
        assert_eq!(m.reply_to.as_deref(), Some("target-guid"));

        let inbound = to_inbound(&m, vec![]);
        assert_eq!(inbound.text, "Reacted with: ❤️");
        assert_eq!(inbound.reply_to.as_ref().map(|r| r.as_str()), Some("target-guid"));
        match &inbound.metadata[..] {
            [crate::gateway::channel::MessageMeta::Reaction { emojis }] => {
                assert_eq!(emojis, &vec!["❤️".to_string()]);
            }
            other => panic!("expected Reaction metadata, got {other:?}"),
        }
    }

    #[test]
    fn add_tapback_string_form_surfaces() {
        // BlueBubbles also emits the type as a string.
        let m = map_webhook_record(&record(
            serde_json::json!({ "associatedMessageType": "laugh" }),
        ))
        .unwrap();
        assert!(m.is_tapback);
        assert_eq!(m.reaction_emoji.as_deref(), Some("😂"));
        assert!(m.is_routable());
    }

    #[test]
    fn remove_tapback_is_not_routable() {
        // Remove reactions (3000–3005 / "-love") are recorded as tapbacks but
        // carry no emoji, so they are dropped rather than surfaced.
        for assoc in [serde_json::json!(3000), serde_json::json!("-love")] {
            let m = map_webhook_record(&record(
                serde_json::json!({ "associatedMessageType": assoc }),
            ))
            .unwrap();
            assert!(m.is_tapback);
            assert_eq!(m.reaction_emoji, None);
            assert!(!m.is_routable(), "remove tapbacks are dropped: {assoc:?}");
        }
    }

    #[test]
    fn normal_message_is_routable_without_reaction() {
        let m = map_webhook_record(&record(serde_json::json!({}))).unwrap();
        assert!(!m.is_tapback);
        assert_eq!(m.reaction_emoji, None);
        assert!(m.is_routable());
    }

    #[test]
    fn own_echo_is_not_routable() {
        let m = map_webhook_record(&record(serde_json::json!({ "isFromMe": true }))).unwrap();
        assert!(!m.is_routable());
    }

    #[test]
    fn detects_group_by_guid() {
        let m = map_webhook_record(&record(serde_json::json!({
            "chatGuid": "iMessage;+;chat123", "isGroup": true
        })))
        .unwrap();
        assert!(m.is_group);
    }

    #[test]
    fn ignores_non_message_events() {
        let p = serde_json::json!({ "type": "typing-indicator", "data": {} });
        assert!(map_webhook_record(&p).is_none());
    }
}
