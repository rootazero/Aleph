use serde::{Deserialize, Serialize};

// ── Chat Types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatType {
    P2p,
    Group,
}

// ── Mentions ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Mention {
    /// Placeholder key in message text (e.g., "@_`user_1`")
    pub key: String,
    /// User's `open_id`
    pub id: String,
    /// Display name
    pub name: String,
    /// Whether this mention refers to the bot itself
    pub is_bot: bool,
}

// ── Events ──

/// Feishu event types parsed from WebSocket frames.
/// These cover the full set of events that Aleph can handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum FeishuEvent {
    /// Incoming message in a chat
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
    /// Interactive card button/form submission
    CardAction {
        chat_id: String,
        sender_id: String,
        action_value: String,
        message_id: String,
    },
    /// Bot was added to a chat
    BotAdded {
        chat_id: String,
        operator_id: String,
        operator_name: Option<String>,
    },
    /// Bot was removed from a chat
    BotRemoved {
        chat_id: String,
        operator_id: Option<String>,
    },
    /// Reaction (emoji) added to a message
    ReactionCreated {
        message_id: String,
        chat_id: Option<String>,
        emoji: String,
        operator_id: String,
    },
    /// Reaction (emoji) removed from a message
    ReactionDeleted {
        message_id: String,
        chat_id: Option<String>,
        emoji: String,
        operator_id: String,
        reaction_id: Option<String>,
    },
    /// Bot menu event (user clicked menu button in bot profile)
    BotMenu {
        event_key: String,
        operator_id: String,
        operator_name: Option<String>,
        timestamp: i64,
    },
    /// Drive file comment notification
    DriveComment {
        event_id: String,
        comment_id: String,
        reply_id: Option<String>,
        file_type: String,
        file_token: String,
        from_user_id: String,
        mentioned: bool,
        content: Option<String>,
    },
    /// Unrecognized event type (captured for logging)
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypingState {
    pub message_id: String,
    pub reaction_id: String,
}

// ── User Info ──

#[derive(Debug, Deserialize)]
pub struct UserInfoResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<UserInfoData>,
}

#[derive(Debug, Deserialize)]
pub struct UserInfoData {
    pub user: Option<UserInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct UserInfo {
    pub open_id: Option<String>,
    pub name: Option<String>,
    pub english_name: Option<String>,
    pub avatar_url: Option<String>,
    pub avatar: Option<Avatar>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Avatar {
    pub avatar_72: Option<String>,
    pub avatar_240: Option<String>,
    pub avatar_640: Option<String>,
}

// ── Envelope verdicts ──
//
// Every response above carries Lark's `code` / `msg` pair; this is where each
// one says so, so `envelope::read_checked` can do the three steps (transport,
// parse, verdict) once instead of at each call site. `BotInfoResponse`'s `msg`
// is optional and the others' is not — exactly the kind of per-type difference
// that made the seven hand-rolled copies drift.

macro_rules! lark_envelope {
    ($ty:ty) => {
        impl crate::gateway::interfaces::feishu::envelope::LarkEnvelope for $ty {
            fn code(&self) -> i32 {
                self.code
            }
            fn message(&self) -> String {
                self.msg.clone()
            }
        }
    };
}

lark_envelope!(TokenResponse);
lark_envelope!(WsEndpointResponse);
lark_envelope!(SendMessageResponse);
lark_envelope!(UploadImageResponse);
lark_envelope!(CardCreateResponse);
lark_envelope!(ReactionResponse);
lark_envelope!(UserInfoResponse);

impl crate::gateway::interfaces::feishu::envelope::LarkEnvelope for BotInfoResponse {
    fn code(&self) -> i32 {
        self.code
    }
    fn message(&self) -> String {
        self.msg.clone().unwrap_or_default()
    }
}
