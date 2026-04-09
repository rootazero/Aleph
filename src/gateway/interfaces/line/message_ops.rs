//! LINE Messaging API Operations
//!
//! Outbound message builders for LINE Messaging API v2 calls.

use reqwest::Client;
use serde::{Deserialize, Serialize};

/// LINE Messaging API client.
#[derive(Clone)]
pub struct LineMessagingApi {
    http: Client,
    channel_access_token: String,
    api_base: String,
}

impl LineMessagingApi {
    pub fn new(channel_access_token: String) -> Self {
        Self {
            http: Client::new(),
            channel_access_token,
            api_base: "https://api.line.me".to_string(),
        }
    }

    /// Send a push message (proactive to user).
    pub async fn push(&self, payload: &LinePushPayload) -> Result<String, String> {
        let url = format!("{}/v2/bot/message/push", self.api_base);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.channel_access_token))
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if resp.status().is_success() {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {}", e))?;
            Ok(body
                .get("sentMessages")
                .and_then(|m| m.get(0))
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string())
        } else {
            Err(format!("LINE API error: {}", resp.status()))
        }
    }

    /// Send a reply message (using replyToken).
    pub async fn reply(
        &self,
        reply_token: &str,
        messages: Vec<ReplyMessage>,
    ) -> Result<String, String> {
        let url = format!("{}/v2/bot/message/reply", self.api_base);
        let payload = ReplyPayload {
            reply_token: reply_token.to_string(),
            messages,
        };
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.channel_access_token))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if resp.status().is_success() {
            Ok("ok".to_string())
        } else {
            Err(format!("LINE API error: {}", resp.status()))
        }
    }

    /// Push a text message.
    pub async fn push_text(&self, to: &str, text: &str) -> Result<String, String> {
        let payload = LinePushPayload::Text(TextPayload {
            to: to.to_string(),
            messages: vec![PushMessage::Text(TextPushContent::new(text))],
        });
        self.push(&payload).await
    }

    /// Push a Flex message.
    pub async fn push_flex(
        &self,
        to: &str,
        alt_text: &str,
        contents: FlexBubbleContents,
    ) -> Result<String, String> {
        let payload = LinePushPayload::Flex(FlexPayload {
            to: to.to_string(),
            alt_text: alt_text.to_string(),
            contents,
        });
        self.push(&payload).await
    }

    /// Push a Quick Reply message.
    pub async fn push_quick_reply(
        &self,
        to: &str,
        text: &str,
        actions: Vec<QuickReplyAction>,
    ) -> Result<String, String> {
        let payload = LinePushPayload::Text(TextPayload {
            to: to.to_string(),
            messages: vec![PushMessage::TextWithQuickReply(TextPushContent {
                msg_type: "text".to_string(),
                text: text.to_string(),
                quick_reply: Some(QuickReply {
                    items: actions
                        .into_iter()
                        .map(|a| QuickReplyItem { action: a })
                        .collect(),
                }),
            })],
        });
        self.push(&payload).await
    }

    /// Push a Template message.
    pub async fn push_template(
        &self,
        to: &str,
        alt_text: &str,
        template: TemplatePayload,
    ) -> Result<String, String> {
        let payload = LinePushPayload::Template(TemplatePayload {
            to: to.to_string(),
            alt_text: alt_text.to_string(),
            contents: Box::new(template),
        });
        self.push(&payload).await
    }
}

// --- Payload types ---

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum LinePushPayload {
    Text(TextPayload),
    Flex(FlexPayload),
    Template(TemplatePayload),
}

#[derive(Debug, Clone, Serialize)]
pub struct TextPayload {
    pub to: String,
    pub messages: Vec<PushMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum PushMessage {
    Text(TextPushContent),
    Image(ImagePushContent),
    Video(VideoPushContent),
    Audio(AudioPushContent),
    File(FilePushContent),
    Location(LocationPushContent),
    Sticker(StickerPushContent),
    TextWithQuickReply(TextPushContent),
}

#[derive(Debug, Clone, Serialize)]
pub struct TextPushContent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick_reply: Option<QuickReply>,
}

impl TextPushContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            msg_type: "text".to_string(),
            text: text.into(),
            quick_reply: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImagePushContent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub original_content_url: String,
    pub preview_image_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoPushContent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub original_content_url: String,
    pub preview_image_url: String,
    pub duration: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioPushContent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub original_content_url: String,
    pub duration: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilePushContent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub file_name: String,
    pub file_size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocationPushContent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub title: String,
    pub address: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StickerPushContent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub package_id: String,
    pub sticker_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlexPayload {
    pub to: String,
    #[serde(rename = "altText")]
    pub alt_text: String,
    #[serde(rename = "type")]
    pub payload_type: String,
    pub contents: FlexBubbleContents,
}

impl FlexPayload {
    pub fn new(to: impl Into<String>, alt_text: impl Into<String>, contents: FlexBubbleContents) -> Self {
        Self {
            to: to.into(),
            alt_text: alt_text.into(),
            payload_type: "flex".to_string(),
            contents,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FlexBubbleContents {
    #[serde(rename = "type")]
    pub bubble_type: String,
    pub body: Option<FlexBox>,
    pub footer: Option<FlexBox>,
    pub header: Option<FlexBox>,
}

impl FlexBubbleContents {
    pub fn new() -> Self {
        Self {
            bubble_type: "bubble".to_string(),
            body: None,
            footer: None,
            header: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FlexBox {
    #[serde(rename = "type")]
    pub box_type: String,
    pub layout: String,
    pub contents: Vec<FlexComponent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum FlexComponent {
    Box(FlexBox),
    Text(FlexText),
    Image(FlexImage),
    Button(FlexButton),
    Icon(FlexIcon),
    Spacer(FlexSpacer),
}

#[derive(Debug, Clone, Serialize)]
pub struct FlexText {
    #[serde(rename = "type")]
    pub text_type: String,
    pub text: String,
    pub size: Option<String>,
    pub weight: Option<String>,
    pub color: Option<String>,
}

impl FlexText {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text_type: "text".to_string(),
            text: text.into(),
            size: None,
            weight: None,
            color: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FlexImage {
    #[serde(rename = "type")]
    pub image_type: String,
    pub url: String,
    pub size: Option<String>,
}

impl FlexImage {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            image_type: "image".to_string(),
            url: url.into(),
            size: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FlexButton {
    #[serde(rename = "type")]
    pub button_type: String,
    pub action: FlexAction,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlexSpacer {
    #[serde(rename = "type")]
    pub spacer_type: String,
    pub size: String,
}

impl FlexSpacer {
    pub fn new(size: impl Into<String>) -> Self {
        Self {
            spacer_type: "spacer".to_string(),
            size: size.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FlexIcon {
    #[serde(rename = "type")]
    pub icon_type: String,
    pub url: String,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlexAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub label: Option<String>,
    pub text: Option<String>,
    pub uri: Option<String>,
}

impl FlexAction {
    pub fn message(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            action_type: "message".to_string(),
            label: Some(label.into()),
            text: Some(text.into()),
            uri: None,
        }
    }

    pub fn uri(label: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            action_type: "uri".to_string(),
            label: Some(label.into()),
            text: None,
            uri: Some(uri.into()),
        }
    }
}

// --- Template payloads ---

#[derive(Debug, Clone, Serialize)]
pub struct TemplatePayload {
    #[serde(rename = "type")]
    pub template_type: String,
    pub text: Option<String>,
    #[serde(rename = "altText")]
    pub alt_text: String,
    pub template: Box<TemplateContent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfirmTemplate {
    pub text: String,
    pub actions: Vec<TemplateAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ButtonsTemplate {
    pub thumbnail_image_url: Option<String>,
    pub image_size: Option<String>,
    pub image_crop: Option<String>,
    pub title: Option<String>,
    pub text: String,
    pub default_action: Option<TemplateAction>,
    pub actions: Vec<TemplateAction>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TemplateContent {
    Confirm(ConfirmTemplate),
    Buttons(Box<ButtonsTemplate>),
    Carousel(CarouselTemplate),
}

#[derive(Debug, Clone, Serialize)]
pub struct CarouselTemplate {
    pub columns: Vec<CarouselColumn>,
    pub image_aspect_ratio: Option<String>,
    pub image_size: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CarouselColumn {
    pub thumbnail_image_url: Option<String>,
    pub image_background_color: Option<String>,
    pub title: Option<String>,
    pub text: String,
    pub default_action: Option<TemplateAction>,
    pub actions: Vec<TemplateAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub label: String,
    pub text: Option<String>,
    pub uri: Option<String>,
    pub data: Option<String>,
}

impl TemplateAction {
    pub fn message(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            action_type: "message".to_string(),
            label: label.into(),
            text: Some(text.into()),
            uri: None,
            data: None,
        }
    }

    pub fn uri(label: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            action_type: "uri".to_string(),
            label: label.into(),
            text: None,
            uri: Some(uri.into()),
            data: None,
        }
    }

    pub fn postback(label: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            action_type: "postback".to_string(),
            label: label.into(),
            text: None,
            uri: None,
            data: Some(data.into()),
        }
    }
}

// --- Quick Reply ---

#[derive(Debug, Clone, Serialize)]
pub struct QuickReply {
    pub items: Vec<QuickReplyItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuickReplyItem {
    pub action: QuickReplyAction,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum QuickReplyAction {
    Message(QuickReplyMessageAction),
    Postback(QuickReplyPostbackAction),
    Uri(QuickReplyUriAction),
}

#[derive(Debug, Clone, Serialize)]
pub struct QuickReplyMessageAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub label: String,
    pub text: String,
}

impl QuickReplyMessageAction {
    pub fn new(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            action_type: "message".to_string(),
            label: label.into(),
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QuickReplyPostbackAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub label: String,
    pub data: String,
    pub display_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuickReplyUriAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub label: String,
    pub uri: String,
}

// --- Reply types ---

#[derive(Debug, Clone, Serialize)]
pub struct ReplyPayload {
    pub reply_token: String,
    pub messages: Vec<ReplyMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ReplyMessage {
    Text(ReplyTextContent),
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplyTextContent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub text: String,
}

impl ReplyTextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            msg_type: "text".to_string(),
            text: text.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_text_payload() {
        let payload = LinePushPayload::Text(TextPayload {
            to: "U123".to_string(),
            messages: vec![PushMessage::Text(TextPushContent::new("Hello"))],
        });
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"text\":\"Hello\""));
        assert!(json.contains("\"type\":\"text\""));
    }

    #[test]
    fn test_build_flex_payload() {
        let payload = LinePushPayload::Flex(FlexPayload::new(
            "U123",
            "Flex message",
            FlexBubbleContents::new(),
        ));
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"flex\""));
        assert!(json.contains("\"altText\":\"Flex message\""));
    }
}