use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct WsPayload {
    pub op: i32,
    #[serde(rename = "d")]
    pub d: Option<serde_json::Value>,
    pub s: Option<u64>,
    pub t: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct C2CMessageEvent {
    pub id: String,
    pub content: String,
    pub timestamp: String,
    pub author: C2CAuthor,
    #[serde(default)]
    pub attachments: Vec<QQAttachment>,
    pub message_scene: Option<MessageScene>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct C2CAuthor {
    pub id: String,
    pub user_openid: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroupMessageEvent {
    pub id: String,
    pub content: String,
    pub timestamp: String,
    pub group_id: String,
    pub group_openid: String,
    pub author: GroupAuthor,
    #[serde(default)]
    pub attachments: Vec<QQAttachment>,
    pub message_scene: Option<MessageScene>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroupAuthor {
    pub id: String,
    pub member_openid: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QQAttachment {
    pub content_type: String,
    pub filename: Option<String>,
    pub url: String,
    pub size: Option<u64>,
    pub height: Option<u32>,
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessageScene {
    pub source: String,
    #[serde(default)]
    pub ext: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum QQEvent {
    C2CMessage(C2CMessageEvent),
    GroupAtMessage(GroupMessageEvent),
    HeartbeatAck,
    Reconnect { url: String },
    Unknown { t: String, d: serde_json::Value },
}

#[derive(Debug, Clone, Serialize)]
pub struct SendMessagePayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
    pub msg_type: u8,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_seq: Option<u16>,
}
