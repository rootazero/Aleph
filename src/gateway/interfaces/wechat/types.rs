//! WeChat Channel iLink API Types
//!
//! Deserializable from iLink Bot API responses.

use serde::{Deserialize, Serialize};

/// iLink API constants
pub const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
pub const WEIXIN_CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
pub const ILINK_APP_ID: &str = "bot";
pub const CHANNEL_VERSION: &str = "2.2.0";
pub const ILINK_APP_CLIENT_VERSION: u32 = (2 << 16) | (2 << 8);

/// Message item types
pub const ITEM_TEXT: u32 = 1;
pub const ITEM_IMAGE: u32 = 2;
pub const ITEM_VOICE: u32 = 3;
pub const ITEM_FILE: u32 = 4;
pub const ITEM_VIDEO: u32 = 5;

/// Media types
pub const MEDIA_IMAGE: u32 = 1;
pub const MEDIA_VIDEO: u32 = 2;
pub const MEDIA_FILE: u32 = 3;
pub const MEDIA_VOICE: u32 = 4;

/// Message type constants
pub const MSG_TYPE_USER: u32 = 1;
pub const MSG_TYPE_BOT: u32 = 2;
pub const MSG_STATE_FINISH: u32 = 2;

/// Typing constants
pub const TYPING_START: u32 = 1;
pub const TYPING_STOP: u32 = 2;

/// API Endpoints
pub const EP_GET_UPDATES: &str = "ilink/bot/getupdates";
pub const EP_SEND_MESSAGE: &str = "ilink/bot/sendmessage";
pub const EP_SEND_TYPING: &str = "ilink/bot/sendtyping";
pub const EP_GET_CONFIG: &str = "ilink/bot/getconfig";
pub const EP_GET_UPLOAD_URL: &str = "ilink/bot/getuploadurl";
pub const EP_GET_BOT_QR: &str = "ilink/bot/get_bot_qrcode";
pub const EP_GET_QR_STATUS: &str = "ilink/bot/get_qrcode_status";

/// Long poll timeout in milliseconds
pub const LONG_POLL_TIMEOUT_MS: u64 = 35_000;
pub const API_TIMEOUT_MS: u64 = 15_000;

/// QR timeout in milliseconds
pub const QR_TIMEOUT_MS: u64 = 120_000;

/// Config timeout in milliseconds
pub const CONFIG_TIMEOUT_MS: u64 = 10_000;

/// GetUpdates response from iLink API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GetUpdatesResponse {
    pub ret: i32,
    pub errcode: Option<i32>,
    pub errmsg: Option<String>,
    pub msgs: Vec<Message>,
    #[serde(rename = "get_updates_buf")]
    pub get_updates_buf: Option<String>,
    #[serde(rename = "longpolling_timeout_ms")]
    pub longpolling_timeout_ms: Option<u64>,
}

/// A message received from iLink API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    #[serde(rename = "msg_id")]
    pub msg_id: String,
    #[serde(rename = "from_user_id")]
    pub from_user_id: String,
    #[serde(rename = "to_user_id")]
    pub to_user_id: String,
    #[serde(rename = "room_id")]
    pub room_id: Option<String>,
    #[serde(rename = "chat_room_id")]
    pub chat_room_id: Option<String>,
    #[serde(rename = "msg_type")]
    pub msg_type: u32,
    #[serde(rename = "item_list")]
    pub item_list: Vec<MessageItem>,
    #[serde(rename = "context_token")]
    pub context_token: Option<String>,
}

/// A message item (text, image, voice, etc.)
#[derive(Debug, Clone, Serialize)]
#[serde(into = "MessageItemHelper")]
pub enum MessageItem {
    Text(TextItem),
    Image(ImageItem),
    Voice(VoiceItem),
    File(FileItem),
    Video(VideoItem),
}

impl<'de> Deserialize<'de> for MessageItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let helper = MessageItemHelper::deserialize(deserializer)?;
        match helper.type_num {
            1 => Ok(MessageItem::Text(helper.text_item.ok_or_else(|| {
                serde::de::Error::custom("missing text_item for Text variant")
            })?)),
            2 => Ok(MessageItem::Image(helper.image_item.ok_or_else(|| {
                serde::de::Error::custom("missing image_item for Image variant")
            })?)),
            3 => Ok(MessageItem::Voice(helper.voice_item.ok_or_else(|| {
                serde::de::Error::custom("missing voice_item for Voice variant")
            })?)),
            4 => Ok(MessageItem::File(helper.file_item.ok_or_else(|| {
                serde::de::Error::custom("missing file_item for File variant")
            })?)),
            5 => Ok(MessageItem::Video(helper.video_item.ok_or_else(|| {
                serde::de::Error::custom("missing video_item for Video variant")
            })?)),
            _ => Err(serde::de::Error::custom(format!(
                "unknown message item type: {}",
                helper.type_num
            ))),
        }
    }
}

/// Helper for deserializing MessageItem from iLink API (integer type field)
#[derive(Debug, Clone, Deserialize, Serialize)]
struct MessageItemHelper {
    #[serde(rename = "type")]
    type_num: u32,
    #[serde(rename = "text_item", skip_serializing_if = "Option::is_none")]
    text_item: Option<TextItem>,
    #[serde(rename = "image_item", skip_serializing_if = "Option::is_none")]
    image_item: Option<ImageItem>,
    #[serde(rename = "voice_item", skip_serializing_if = "Option::is_none")]
    voice_item: Option<VoiceItem>,
    #[serde(rename = "file_item", skip_serializing_if = "Option::is_none")]
    file_item: Option<FileItem>,
    #[serde(rename = "video_item", skip_serializing_if = "Option::is_none")]
    video_item: Option<VideoItem>,
}

impl From<MessageItem> for MessageItemHelper {
    fn from(item: MessageItem) -> Self {
        match item {
            MessageItem::Text(t) => Self {
                type_num: 1,
                text_item: Some(t),
                image_item: None,
                voice_item: None,
                file_item: None,
                video_item: None,
            },
            MessageItem::Image(i) => Self {
                type_num: 2,
                text_item: None,
                image_item: Some(i),
                voice_item: None,
                file_item: None,
                video_item: None,
            },
            MessageItem::Voice(v) => Self {
                type_num: 3,
                text_item: None,
                image_item: None,
                voice_item: Some(v),
                file_item: None,
                video_item: None,
            },
            MessageItem::File(f) => Self {
                type_num: 4,
                text_item: None,
                image_item: None,
                voice_item: None,
                file_item: Some(f),
                video_item: None,
            },
            MessageItem::Video(v) => Self {
                type_num: 5,
                text_item: None,
                image_item: None,
                voice_item: None,
                file_item: None,
                video_item: Some(v),
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TextItem {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageItem {
    pub media: MediaRef,
    #[serde(rename = "aeskey")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceItem {
    pub media: MediaRef,
    pub text: Option<String>,
    #[serde(rename = "aeskey")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileItem {
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub media: MediaRef,
    #[serde(rename = "aeskey")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VideoItem {
    pub media: MediaRef,
    #[serde(rename = "aeskey")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaRef {
    #[serde(rename = "encrypt_query_param")]
    pub encrypt_query_param: Option<String>,
    #[serde(rename = "full_url")]
    pub full_url: Option<String>,
    #[serde(rename = "aes_key")]
    pub aes_key: Option<String>,
}

/// Send message request payload
#[derive(Debug, Clone, Serialize)]
pub struct SendMessagePayload {
    pub msg: OutboundMsg,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutboundMsg {
    #[serde(rename = "from_user_id")]
    pub from_user_id: String,
    #[serde(rename = "to_user_id")]
    pub to_user_id: String,
    #[serde(rename = "client_id")]
    pub client_id: String,
    #[serde(rename = "message_type")]
    pub message_type: u32,
    #[serde(rename = "message_state")]
    pub message_state: u32,
    #[serde(rename = "item_list")]
    pub item_list: Vec<OutboundItem>,
    #[serde(rename = "context_token")]
    pub context_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutboundItem {
    #[serde(rename = "type")]
    pub item_type: u32,
    #[serde(rename = "text_item")]
    pub text_item: Option<TextItemContent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TextItemContent {
    pub text: String,
}

/// QR login response
#[derive(Debug, Clone, Deserialize)]
pub struct QrCodeResponse {
    pub qrcode: Option<String>,
    #[serde(rename = "qrcode_img_content")]
    pub qrcode_img_content: Option<String>,
}

/// QR status response
#[derive(Debug, Clone, Deserialize)]
pub struct QrStatusResponse {
    pub status: String,
    #[serde(rename = "ilink_bot_id")]
    pub ilink_bot_id: Option<String>,
    #[serde(rename = "bot_token")]
    pub bot_token: Option<String>,
    #[serde(rename = "baseurl")]
    pub base_url: Option<String>,
    #[serde(rename = "ilink_user_id")]
    pub ilink_user_id: Option<String>,
    #[serde(rename = "redirect_host")]
    pub redirect_host: Option<String>,
}

/// Typing indicator payload
#[derive(Debug, Clone, Serialize)]
pub struct SendTypingPayload {
    #[serde(rename = "ilink_user_id")]
    pub ilink_user_id: String,
    #[serde(rename = "typing_ticket")]
    pub typing_ticket: String,
    pub status: u32,
}

/// Config response (for getting typing ticket)
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigResponse {
    #[serde(rename = "typing_ticket")]
    pub typing_ticket: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_get_updates_response() {
        let json = r#"{
            "ret": 0,
            "msgs": [
                {
                    "msg_id": "msg123",
                    "from_user_id": "wxid_abc",
                    "to_user_id": "bot_id",
                    "room_id": "",
                    "msg_type": 1,
                    "item_list": [
                        {"type": 1, "text_item": {"text": "Hello"}}
                    ],
                    "context_token": "ctx_123"
                }
            ],
            "get_updates_buf": "sync_buf_abc"
        }"#;
        let resp: GetUpdatesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.ret, 0);
        assert_eq!(resp.msgs.len(), 1);
        assert_eq!(resp.msgs[0].from_user_id, "wxid_abc");
    }

    #[test]
    fn test_message_item_text() {
        let json = r#"{"type": 1, "text_item": {"text": "Hello"}}"#;
        let item: MessageItem = serde_json::from_str(json).unwrap();
        assert!(matches!(item, MessageItem::Text(_)));
    }

    #[test]
    fn test_media_types() {
        assert_eq!(ITEM_TEXT, 1);
        assert_eq!(ITEM_IMAGE, 2);
        assert_eq!(ITEM_VOICE, 3);
        assert_eq!(ITEM_FILE, 4);
        assert_eq!(ITEM_VIDEO, 5);
    }
}
