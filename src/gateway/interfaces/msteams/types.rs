//! Bot Framework Activity Types
//!
//! Minimal structural types for the Bot Framework v3 protocol.

use serde::{Deserialize, Serialize};

/// Bot Framework Activity (inbound and outbound)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[serde(rename = "type")]
    pub activity_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<ChannelAccount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationAccount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<ChannelAccount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<ActivityAttachment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members_added: Option<Vec<ChannelAccount>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members_removed: Option<Vec<ChannelAccount>>,
    #[serde(rename = "reactionsAdded", skip_serializing_if = "Option::is_none")]
    pub reactions_added: Option<Vec<MessageReaction>>,
    #[serde(rename = "reactionsRemoved", skip_serializing_if = "Option::is_none")]
    pub reactions_removed: Option<Vec<MessageReaction>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_data: Option<serde_json::Value>,
}

impl Activity {
    #[must_use]
    pub fn text_message(text: &str) -> Self {
        Self {
            activity_type: "message".into(),
            text: Some(text.into()),
            text_format: Some("markdown".into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelAccount {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "aadObjectId", skip_serializing_if = "Option::is_none")]
    pub aad_object_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationAccount {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "conversationType", skip_serializing_if = "Option::is_none")]
    pub conversation_type: Option<String>,
    #[serde(rename = "tenantId", skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(rename = "isGroup", skip_serializing_if = "Option::is_none")]
    pub is_group: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageReaction {
    #[serde(rename = "type")]
    pub reaction_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityAttachment {
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActivityResponse {
    pub id: String,
}

pub fn inject_ai_entity(activity: &mut Activity) {
    let entity = build_ai_generated_entity();
    match activity.entities {
        Some(ref mut entities) => entities.push(entity),
        None => activity.entities = Some(vec![entity]),
    }
}

#[must_use]
pub fn build_ai_generated_entity() -> serde_json::Value {
    serde_json::json!({
        "type": "https://schema.org/Message",
        "@type": "Message",
        "@id": "",
        "additionalType": ["AIGeneratedContent"]
    })
}

#[must_use]
pub fn build_stream_info_entity(
    stream_id: Option<&str>,
    stream_type: &str,
    sequence: u32,
) -> serde_json::Value {
    let mut entity = serde_json::json!({
        "type": "streaminfo",
        "streamType": stream_type,
        "streamSequence": sequence,
    });
    if let Some(id) = stream_id {
        entity["streamId"] = serde_json::Value::String(id.into());
    }
    entity
}

#[must_use]
pub fn build_welcome_card(bot_name: &str, prompt_starters: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "AdaptiveCard",
        "version": "1.5",
        "body": [
            {
                "type": "TextBlock",
                "text": format!("Hi! I'm {}.", bot_name),
                "weight": "bolder",
                "size": "medium"
            },
            {
                "type": "TextBlock",
                "text": "I can help you with questions, tasks, and more. Here are some things to try:",
                "wrap": true
            }
        ],
        "actions": prompt_starters.iter().map(|label| {
            serde_json::json!({
                "type": "Action.Submit",
                "title": label,
                "data": { "msteams": { "type": "imBack", "value": label } }
            })
        }).collect::<Vec<_>>()
    })
}

pub fn strip_mentions(text: &str) -> String {
    use std::sync::LazyLock;
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<at>[^<]*</at>\s*").expect("mention regex is valid"));
    RE.replace_all(text, "").trim().to_string()
}

use std::sync::LazyLock;
static QUOTE_SENDER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"<strong[^>]*itemprop=["']mri["'][^>]*>(.*?)</strong>"#)
        .expect("quote sender regex is valid")
});
static QUOTE_BODY_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"<p[^>]*itemprop=["']copy["'][^>]*>(.*?)</p>"#)
        .expect("quote body regex is valid")
});

const STATUS_TEXTS: &[&str] = &[
    "Thinking...",
    "Working on that...",
    "Checking the details...",
    "Putting an answer together...",
];

#[must_use]
pub fn pick_status_text() -> &'static str {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize;
    STATUS_TEXTS[seed % STATUS_TEXTS.len()]
}

/// Quote info extracted from a Teams HTML reply attachment.
#[derive(Debug, Clone)]
pub struct QuoteInfo {
    pub sender: String,
    pub body: String,
}

/// Decode common HTML entities to plain text.
fn decode_html_entities(html: &str) -> String {
    html.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&") // must be last to prevent double-decoding
}

/// Strip HTML tags, preserving text content.
fn html_to_plain_text(html: &str) -> String {
    // Remove `<...>` tag spans (preserving the text between them), then decode
    // entities and collapse whitespace. The previous implementation replaced
    // every alphanumeric/space char with a space — i.e. it erased exactly the
    // readable content and kept only punctuation/markup, so quoted-reply bodies
    // came back garbage/empty.
    let mut stripped = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => stripped.push(c),
            _ => {}
        }
    }
    decode_html_entities(&stripped)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract quote info from MS Teams HTML reply attachments.
///
/// Teams wraps quoted content in a blockquote with itemtype="http://schema.skype.com/Reply".
/// The sender is in `<strong itemprop="mri">` and body in `<p itemprop="copy">`.
pub fn extract_quote_info(attachments: &[ActivityAttachment]) -> Option<QuoteInfo> {
    for att in attachments {
        // Skip attachments without content instead of aborting the whole scan.
        let Some(content) = att.content.as_ref() else {
            continue;
        };

        // Content might be a JSON object with text/body fields
        let html = if let Some(obj) = content.as_object() {
            obj.get("text")
                .or_else(|| obj.get("body"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
        } else if let Some(s) = content.as_str() {
            s
        } else {
            continue;
        };

        if !html.contains("schema.skype.com/Reply") {
            continue;
        }

        // Extract sender from <strong itemprop="mri">
        let sender = QUOTE_SENDER_RE
            .captures(html)
            .and_then(|c| c.get(1))
            .map_or_else(|| "unknown".to_string(), |m| html_to_plain_text(m.as_str()));

        // Extract body from <p itemprop="copy">
        let body = QUOTE_BODY_RE
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| html_to_plain_text(m.as_str()))
            .unwrap_or_default();

        if !body.is_empty() {
            return Some(QuoteInfo { sender, body });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activity_text_message() {
        let a = Activity::text_message("hello");
        assert_eq!(a.activity_type, "message");
        assert_eq!(a.text.as_deref(), Some("hello"));
        assert_eq!(a.text_format.as_deref(), Some("markdown"));
    }

    #[test]
    fn test_deserialize_message_activity() {
        let json = r#"{
            "type": "message",
            "id": "1234",
            "serviceUrl": "https://smba.trafficmanager.net/amer/",
            "channelId": "msteams",
            "from": {"id": "user-aad-id", "name": "John", "aadObjectId": "aad-123"},
            "conversation": {"id": "19:conv@thread.v2", "conversationType": "personal"},
            "recipient": {"id": "bot-id", "name": "Aleph"},
            "text": "Hello bot"
        }"#;
        let activity: Activity = serde_json::from_str(json).unwrap();
        assert_eq!(activity.activity_type, "message");
        assert_eq!(activity.text.as_deref(), Some("Hello bot"));
        assert_eq!(
            activity.from.as_ref().unwrap().aad_object_id.as_deref(),
            Some("aad-123")
        );
        assert_eq!(
            activity
                .conversation
                .as_ref()
                .unwrap()
                .conversation_type
                .as_deref(),
            Some("personal")
        );
    }

    #[test]
    fn test_deserialize_conversation_update() {
        let json = r#"{
            "type": "conversationUpdate",
            "membersAdded": [{"id": "bot-id", "name": "Aleph"}],
            "conversation": {"id": "19:conv@thread.v2"},
            "recipient": {"id": "bot-id"}
        }"#;
        let activity: Activity = serde_json::from_str(json).unwrap();
        assert_eq!(activity.activity_type, "conversationUpdate");
        assert_eq!(activity.members_added.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_deserialize_minimal_activity() {
        let json = r#"{"type": "typing"}"#;
        let activity: Activity = serde_json::from_str(json).unwrap();
        assert_eq!(activity.activity_type, "typing");
        assert!(activity.text.is_none());
    }

    #[test]
    fn test_build_stream_info_entity() {
        let e = build_stream_info_entity(None, "informative", 0);
        assert_eq!(e["type"], "streaminfo");
        assert_eq!(e["streamType"], "informative");
        assert!(e.get("streamId").is_none());

        let e2 = build_stream_info_entity(Some("stream-1"), "streaming", 3);
        assert_eq!(e2["streamId"], "stream-1");
        assert_eq!(e2["streamSequence"], 3);
    }

    #[test]
    fn test_build_ai_generated_entity() {
        let e = build_ai_generated_entity();
        assert_eq!(e["type"], "https://schema.org/Message");
        assert_eq!(e["additionalType"][0], "AIGeneratedContent");
    }

    #[test]
    fn test_build_welcome_card() {
        let card = build_welcome_card("Aleph", &["What can you do?", "Help me"]);
        assert_eq!(card["type"], "AdaptiveCard");
        assert_eq!(card["version"], "1.5");
        assert_eq!(card["actions"].as_array().unwrap().len(), 2);
        assert_eq!(card["actions"][0]["title"], "What can you do?");
    }

    #[test]
    fn test_inject_ai_entity() {
        let mut a = Activity::text_message("test");
        assert!(a.entities.is_none());
        inject_ai_entity(&mut a);
        assert_eq!(a.entities.as_ref().unwrap().len(), 1);
        inject_ai_entity(&mut a);
        assert_eq!(a.entities.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_strip_mentions() {
        assert_eq!(strip_mentions("<at>Aleph</at> hello world"), "hello world");
        assert_eq!(strip_mentions("hello world"), "hello world");
        assert_eq!(strip_mentions("<at>Bot</at>  <at>User</at> hi"), "hi");
        assert_eq!(strip_mentions(""), "");
    }

    #[test]
    fn test_pick_status_text() {
        let text = pick_status_text();
        assert!(STATUS_TEXTS.contains(&text));
    }
}
