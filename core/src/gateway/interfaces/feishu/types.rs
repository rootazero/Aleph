use serde::Deserialize;

// ── Config ──

fn default_domain() -> String { "feishu".to_string() }
fn default_true() -> bool { true }
fn default_render_mode() -> String { "auto".to_string() }

#[derive(Debug, Clone, Deserialize)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    #[serde(default = "default_domain")]
    pub domain: String,
    pub bot_name: Option<String>,
    #[serde(default = "default_true")]
    pub dm_allowed: bool,
    #[serde(default)]
    pub groups_allowed: bool,
    #[serde(default = "default_true")]
    pub require_mention: bool,
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default = "default_render_mode")]
    pub render_mode: String,
    #[serde(default = "default_true")]
    pub typing_indicator: bool,
}

impl FeishuConfig {
    /// Resolve the base URL from the domain field.
    pub fn base_url(&self) -> String {
        match self.domain.as_str() {
            "feishu" => "https://open.feishu.cn".to_string(),
            "lark" => "https://open.larksuite.com".to_string(),
            custom => custom.trim_end_matches('/').to_string(),
        }
    }
}

// ── Chat Types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    P2p,
    Group,
}

// ── Mentions ──

#[derive(Debug, Clone)]
pub struct Mention {
    /// Placeholder key in message text (e.g., "@_user_1")
    pub key: String,
    /// User's open_id
    pub id: String,
    /// Display name
    pub name: String,
    /// Whether this mention refers to the bot itself
    pub is_bot: bool,
}

// ── Events ──

#[derive(Debug, Clone)]
pub enum FeishuEvent {
    MessageReceive {
        message_id: String,
        chat_id: String,
        chat_type: ChatType,
        sender_id: String,
        sender_name: Option<String>,
        message_type: String,
        content: String,
        mentions: Vec<Mention>,
        parent_id: Option<String>,
    },
    Unknown(String),
}

// ── WebSocket Frame Types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsFrameType {
    Event,
    Ping,
    Pong,
}

// ── API Response Types ──

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub code: i32,
    pub msg: String,
    #[serde(rename = "app_access_token")]
    pub app_access_token: Option<String>,
    pub expire: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct WsEndpointResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<WsEndpointData>,
}

#[derive(Debug, Deserialize)]
pub struct WsEndpointData {
    #[serde(rename = "URL")]
    pub url: String,
    pub client_config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct BotInfoResponse {
    pub code: i32,
    pub msg: Option<String>,
    pub bot: Option<BotInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotInfo {
    pub app_name: Option<String>,
    pub open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<SendMessageData>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageData {
    pub message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadImageResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<UploadImageData>,
}

#[derive(Debug, Deserialize)]
pub struct UploadImageData {
    pub image_key: Option<String>,
}

// ── WebSocket Event Envelope ──

#[derive(Debug, Deserialize)]
pub struct WsEventEnvelope {
    pub header: Option<WsEventHeader>,
    pub event: Option<serde_json::Value>,
    /// Ping/pong frames may not have header/event
    #[serde(rename = "type")]
    pub frame_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WsEventHeader {
    pub event_id: Option<String>,
    pub event_type: Option<String>,
    pub token: Option<String>,
    pub create_time: Option<String>,
}

// ── Message Event Payload ──

#[derive(Debug, Deserialize)]
pub struct MessageEventPayload {
    pub sender: Option<MessageSender>,
    pub message: Option<MessageBody>,
}

#[derive(Debug, Deserialize)]
pub struct MessageSender {
    pub sender_id: Option<SenderIdContainer>,
    pub sender_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SenderIdContainer {
    pub open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MessageBody {
    pub message_id: Option<String>,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub message_type: Option<String>,
    pub content: Option<String>,
    pub mentions: Option<Vec<MentionPayload>>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MentionPayload {
    pub key: Option<String>,
    pub id: Option<MentionId>,
    pub name: Option<String>,
    pub tenant_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MentionId {
    pub open_id: Option<String>,
}

// ── Text Content ──

#[derive(Debug, Deserialize)]
pub struct TextContent {
    pub text: Option<String>,
}

// ── Reaction API Response ──

#[derive(Debug, Deserialize)]
pub struct ReactionResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<ReactionData>,
}

#[derive(Debug, Deserialize)]
pub struct ReactionData {
    pub reaction_id: Option<String>,
}

// ── Card Kit API Response ──

#[derive(Debug, Deserialize)]
pub struct CardCreateResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<CardCreateData>,
}

#[derive(Debug, Deserialize)]
pub struct CardCreateData {
    pub card_id: Option<String>,
}

// ── Typing State ──

#[derive(Debug, Clone)]
pub struct TypingState {
    pub message_id: String,
    pub reaction_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_url_feishu() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
        };
        assert_eq!(config.base_url(), "https://open.feishu.cn");
    }

    #[test]
    fn test_base_url_lark() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "lark".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
        };
        assert_eq!(config.base_url(), "https://open.larksuite.com");
    }

    #[test]
    fn test_base_url_custom() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "https://my.feishu.internal/".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
        };
        assert_eq!(config.base_url(), "https://my.feishu.internal");
    }

    #[test]
    fn test_config_deserialization_defaults() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret123"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.domain, "feishu");
        assert!(config.dm_allowed);
        assert!(!config.groups_allowed);
        assert!(config.require_mention);
        assert!(config.bot_name.is_none());
    }

    #[test]
    fn test_config_deserialization_full() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret123",
            "domain": "lark",
            "bot_name": "MyBot",
            "dm_allowed": false,
            "groups_allowed": true,
            "require_mention": false
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.domain, "lark");
        assert!(!config.dm_allowed);
        assert!(config.groups_allowed);
        assert!(!config.require_mention);
        assert_eq!(config.bot_name.as_deref(), Some("MyBot"));
    }

    #[test]
    fn test_config_streaming_defaults() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(config.streaming);
        assert_eq!(config.render_mode, "auto");
        assert!(config.typing_indicator);
    }

    #[test]
    fn test_config_streaming_overrides() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret",
            "streaming": false,
            "render_mode": "card",
            "typing_indicator": false
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(!config.streaming);
        assert_eq!(config.render_mode, "card");
        assert!(!config.typing_indicator);
    }
}
