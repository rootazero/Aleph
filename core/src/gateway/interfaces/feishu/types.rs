use serde::Deserialize;

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
        root_id: Option<String>,
    },
    CardAction {
        chat_id: String,
        sender_id: String,
        action_value: String,
        message_id: String,
    },
    Unknown(String),
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
    pub root_id: Option<String>,
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
