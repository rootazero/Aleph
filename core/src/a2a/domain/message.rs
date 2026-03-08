use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Map;

/// A2A message role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum A2ARole {
    User,
    Agent,
}

/// A2A message — the communication payload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2AMessage {
    pub message_id: String,
    pub role: A2ARole,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
}

/// Content part — text, file, or structured data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Part {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, serde_json::Value>>,
    },
    File {
        file: FileContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, serde_json::Value>>,
    },
    Data {
        data: Map<String, serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Map<String, serde_json::Value>>,
    },
}

/// File content — either inline bytes (base64) or a URI reference
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<String>, // Base64 encoded
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// Artifact — output produced by an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub artifact_id: String,
    pub kind: String,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, serde_json::Value>>,
}

impl A2AMessage {
    /// Create a simple text message
    pub fn text(role: A2ARole, text: impl Into<String>) -> Self {
        Self {
            message_id: uuid::Uuid::new_v4().to_string(),
            role,
            parts: vec![Part::Text {
                text: text.into(),
                metadata: None,
            }],
            session_id: None,
            timestamp: Some(Utc::now()),
            metadata: None,
        }
    }

    /// Extract all text parts concatenated with newlines
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_serde_roundtrip() {
        for role in [A2ARole::User, A2ARole::Agent] {
            let json = serde_json::to_string(&role).unwrap();
            let back: A2ARole = serde_json::from_str(&json).unwrap();
            assert_eq!(role, back);
        }
    }

    #[test]
    fn role_json_values() {
        assert_eq!(serde_json::to_string(&A2ARole::User).unwrap(), "\"user\"");
        assert_eq!(serde_json::to_string(&A2ARole::Agent).unwrap(), "\"agent\"");
    }

    #[test]
    fn text_constructor() {
        let msg = A2AMessage::text(A2ARole::User, "Hello, agent!");
        assert_eq!(msg.role, A2ARole::User);
        assert_eq!(msg.parts.len(), 1);
        assert!(msg.timestamp.is_some());
        assert!(!msg.message_id.is_empty());

        match &msg.parts[0] {
            Part::Text { text, metadata } => {
                assert_eq!(text, "Hello, agent!");
                assert!(metadata.is_none());
            }
            _ => panic!("Expected Text part"),
        }
    }

    #[test]
    fn text_content_single_part() {
        let msg = A2AMessage::text(A2ARole::Agent, "Response text");
        assert_eq!(msg.text_content(), "Response text");
    }

    #[test]
    fn text_content_multiple_parts() {
        let msg = A2AMessage {
            message_id: "test".to_string(),
            role: A2ARole::Agent,
            parts: vec![
                Part::Text {
                    text: "Line 1".to_string(),
                    metadata: None,
                },
                Part::File {
                    file: FileContent {
                        name: Some("test.txt".to_string()),
                        mime_type: None,
                        bytes: None,
                        uri: None,
                    },
                    metadata: None,
                },
                Part::Text {
                    text: "Line 2".to_string(),
                    metadata: None,
                },
            ],
            session_id: None,
            timestamp: None,
            metadata: None,
        };
        assert_eq!(msg.text_content(), "Line 1\nLine 2");
    }

    #[test]
    fn text_content_no_text_parts() {
        let msg = A2AMessage {
            message_id: "test".to_string(),
            role: A2ARole::Agent,
            parts: vec![Part::Data {
                data: serde_json::Map::new(),
                metadata: None,
            }],
            session_id: None,
            timestamp: None,
            metadata: None,
        };
        assert_eq!(msg.text_content(), "");
    }

    #[test]
    fn message_serde_roundtrip() {
        let msg = A2AMessage::text(A2ARole::User, "Test message");
        let json = serde_json::to_string(&msg).unwrap();
        let back: A2AMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.message_id, msg.message_id);
        assert_eq!(back.role, msg.role);
        assert_eq!(back.text_content(), "Test message");
    }

    #[test]
    fn part_tagged_enum_serde() {
        let text_part = Part::Text {
            text: "hello".to_string(),
            metadata: None,
        };
        let json = serde_json::to_value(&text_part).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");

        let file_part = Part::File {
            file: FileContent {
                name: Some("doc.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                bytes: None,
                uri: Some("https://example.com/doc.pdf".to_string()),
            },
            metadata: None,
        };
        let json = serde_json::to_value(&file_part).unwrap();
        assert_eq!(json["type"], "file");
        assert_eq!(json["file"]["name"], "doc.pdf");
        assert_eq!(json["file"]["mimeType"], "application/pdf");
    }

    #[test]
    fn artifact_serde_roundtrip() {
        let artifact = Artifact {
            artifact_id: "art-1".to_string(),
            kind: "code".to_string(),
            parts: vec![Part::Text {
                text: "fn main() {}".to_string(),
                metadata: None,
            }],
            metadata: None,
        };
        let json = serde_json::to_string(&artifact).unwrap();
        let back: Artifact = serde_json::from_str(&json).unwrap();
        assert_eq!(back.artifact_id, "art-1");
        assert_eq!(back.kind, "code");
        assert_eq!(back.parts.len(), 1);
    }

    #[test]
    fn message_camelcase_fields() {
        let msg = A2AMessage::text(A2ARole::User, "test");
        let json = serde_json::to_value(&msg).unwrap();
        assert!(json.get("messageId").is_some());
        assert!(json.get("role").is_some());
        assert!(json.get("parts").is_some());
    }
}
