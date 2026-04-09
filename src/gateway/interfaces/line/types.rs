//! LINE Channel Event Types
//!
//! Deserializable from LINE webhook payload.

use serde::{Deserialize, Serialize};

/// LINE event types received via webhook.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LineEvent {
    Message {
        reply_token: String,
        source: LineSource,
        message: LineMessage,
    },
    Follow {
        reply_token: String,
        source: LineSource,
    },
    #[serde(rename = "unfollow")]
    Unfollow {
        source: LineSource,
    },
    Join {
        reply_token: String,
        source: LineSource,
    },
    Leave {
        source: LineSource,
    },
    Postback {
        reply_token: String,
        source: LineSource,
        postback: PostbackData,
    },
    Beacon {
        reply_token: String,
        source: LineSource,
        beacon: BeaconData,
    },
    AccountLink {
        reply_token: String,
        source: LineSource,
        link: LinkData,
    },
}

/// Source of a LINE event (user, group, or room).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineSource {
    #[serde(rename = "type")]
    pub source_type: LineSourceType,
    pub user_id: Option<String>,
    pub group_id: Option<String>,
    pub room_id: Option<String>,
}

/// Type of LINE source.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LineSourceType {
    User,
    Group,
    Room,
}

/// A LINE message within an event.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LineMessage {
    Text(TextMessage),
    Image(ImageMessage),
    Video(VideoMessage),
    Audio(AudioMessage),
    File(FileMessage),
    Location(LocationMessage),
    Sticker(StickerMessage),
    #[serde(other)]
    Unknown,
}

impl LineMessage {
    pub fn id(&self) -> &str {
        match self {
            LineMessage::Text(m) => &m.id,
            LineMessage::Image(m) => &m.id,
            LineMessage::Video(m) => &m.id,
            LineMessage::Audio(m) => &m.id,
            LineMessage::File(m) => &m.id,
            LineMessage::Location(m) => &m.id,
            LineMessage::Sticker(m) => &m.id,
            LineMessage::Unknown => "",
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            LineMessage::Text(m) => Some(&m.text),
            _ => None,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self, LineMessage::Image(_))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextMessage {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageMessage {
    pub id: String,
    #[serde(default)]
    pub content_provider: ContentProvider,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoMessage {
    pub id: String,
    pub duration: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AudioMessage {
    pub id: String,
    pub duration: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileMessage {
    pub id: String,
    pub file_name: String,
    pub file_size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocationMessage {
    pub id: String,
    pub title: String,
    pub address: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StickerMessage {
    pub id: String,
    pub package_id: String,
    pub sticker_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ContentProvider {
    #[serde(rename = "type", default)]
    pub provider_type: String,
    #[serde(default)]
    pub original_content_url: Option<String>,
    #[serde(default)]
    pub preview_image_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostbackData {
    pub data: String,
    #[serde(default)]
    pub params: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BeaconData {
    #[serde(rename = "type")]
    pub type_: String,
    pub hwid: String,
    #[serde(default)]
    pub device_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinkData {
    pub result: String,
    pub nonce: String,
}

/// Reply token wrapper for outbound message construction.
#[derive(Debug, Clone)]
pub struct LineReplyToken(String);

impl LineReplyToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for LineReplyToken {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_message_event() {
        let json = r#"{
            "type": "message",
            "replyToken": "nHuyWiB4yP5Q5a5ga5...",
            "source": {
                "type": "user",
                "userId": "U4af4980629..."
            },
            "message": {
                "type": "text",
                "id": "12345",
                "text": "Hello"
            }
        }"#;
        let event: LineEvent = serde_json::from_str(json).unwrap();
        match event {
            LineEvent::Message {
                reply_token,
                source,
                message,
            } => {
                assert_eq!(source.user_id.as_deref(), Some("U4af4980629..."));
                assert_eq!(source.source_type, LineSourceType::User);
                assert_eq!(message.id(), "12345");
            }
            _ => panic!("expected Message event"),
        }
    }

    #[test]
    fn test_deserialize_group_event() {
        let json = r#"{
            "type": "message",
            "replyToken": "nHuyWiB4yP5Q5a5ga5...",
            "source": {
                "type": "group",
                "groupId": "C1234567890..."
            },
            "message": {
                "type": "text",
                "id": "12345",
                "text": "Hello from group"
            }
        }"#;
        let event: LineEvent = serde_json::from_str(json).unwrap();
        match event {
            LineEvent::Message { source, .. } => {
                assert_eq!(source.source_type, LineSourceType::Group);
                assert_eq!(source.group_id.as_deref(), Some("C1234567890..."));
            }
            _ => panic!("expected Message event"),
        }
    }

    #[test]
    fn test_deserialize_follow_event() {
        let json = r#"{
            "type": "follow",
            "replyToken": "nHuyWiB4yP5Q5a5ga5...",
            "source": {"type": "user", "userId": "U4af4980629..."}
        }"#;
        let event: LineEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, LineEvent::Follow { .. }));
    }

    #[test]
    fn test_deserialize_postback_event() {
        let json = r#"{
            "type": "postback",
            "replyToken": "nHuyWiB4yP5Q5a5ga5...",
            "source": {"type": "user", "userId": "U4af4980629..."},
            "postback": {"data": "action=buy&item=123"}
        }"#;
        let event: LineEvent = serde_json::from_str(json).unwrap();
        match event {
            LineEvent::Postback { postback, .. } => {
                assert_eq!(postback.data, "action=buy&item=123");
            }
            _ => panic!("expected Postback event"),
        }
    }

    #[test]
    fn test_line_source_user() {
        let json = r#"{"type": "user", "userId": "U123"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, LineSourceType::User);
        assert_eq!(source.user_id.as_deref(), Some("U123"));
        assert!(source.group_id.is_none());
        assert!(source.room_id.is_none());
    }

    #[test]
    fn test_line_source_group() {
        let json = r#"{"type": "group", "groupId": "G123", "userId": "U456"}"#;
        let source: LineSource = serde_json::from_str(json).unwrap();
        assert_eq!(source.source_type, LineSourceType::Group);
        assert_eq!(source.user_id.as_deref(), Some("U456"));
        assert_eq!(source.group_id.as_deref(), Some("G123"));
    }

    #[test]
    fn test_line_message_text() {
        let json = r#"{"type": "text", "id": "123", "text": "hello"}"#;
        let msg: LineMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id(), "123");
        assert_eq!(msg.text().unwrap(), "hello");
    }

    #[test]
    fn test_line_message_image() {
        let json = r#"{"type": "image", "id": "456", "contentProvider": {"type": "line"}}"#;
        let msg: LineMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id(), "456");
        assert!(msg.is_image());
    }
}
