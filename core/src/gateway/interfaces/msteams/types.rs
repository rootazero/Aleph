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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_data: Option<serde_json::Value>,
}

impl Activity {
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

pub fn build_ai_generated_entity() -> serde_json::Value {
    serde_json::json!({
        "type": "https://schema.org/Message",
        "@type": "Message",
        "@id": "",
        "additionalType": ["AIGeneratedContent"]
    })
}

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
        LazyLock::new(|| regex::Regex::new(r"<at>[^<]*</at>\s*").unwrap());
    RE.replace_all(text, "").trim().to_string()
}

const STATUS_TEXTS: &[&str] = &[
    "Thinking...",
    "Working on that...",
    "Checking the details...",
    "Putting an answer together...",
];

pub fn pick_status_text() -> &'static str {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize;
    STATUS_TEXTS[seed % STATUS_TEXTS.len()]
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
