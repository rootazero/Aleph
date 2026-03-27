//! Microsoft Teams Channel
//!
//! Pure HTTP implementation against Bot Framework REST API v3.

pub mod api;
pub mod auth;
pub mod config;
pub mod streaming;
pub mod types;

pub use config::MsTeamsConfig;

use std::collections::HashMap;
use std::time::Instant;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::gateway::channel::{
    Attachment, Channel, ChannelCapabilities, ChannelError, ChannelId, ChannelInfo, ChannelResult,
    ChannelState, ChannelStatus, ConversationId, InboundMessage, MessageId, NativeStreamHandler,
    OutboundMessage, SendResult, StreamProtocol, UserId,
};
use crate::gateway::webhook_receiver::WebhookHandler;
use crate::sync_primitives::Arc;

use self::api::BotFrameworkClient;
use self::auth::{validate_service_url, JwtValidator, TokenCache};
use self::types::*;

// ── ConversationReference ────────────────────────────────────────────────────

/// Cached reference for proactive messaging and service URL resolution.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct ConversationReference {
    service_url: String,
    conversation_id: String,
    bot_id: String,
    last_seen: Instant,
}

/// Maximum number of cached conversation references (LRU eviction by `last_seen`).
const MAX_CONVERSATION_REFS: usize = 10_000;

// ── MsTeamsChannel ───────────────────────────────────────────────────────────

/// Microsoft Teams channel implementation.
pub struct MsTeamsChannel {
    info: ChannelInfo,
    state: ChannelState,
    config: MsTeamsConfig,
    client: Arc<BotFrameworkClient>,
    jwt_validator: Arc<JwtValidator>,
    conversation_refs: Arc<RwLock<HashMap<String, ConversationReference>>>,
    /// Reverse map: message_id → conversation_id (for delete without conversation context)
    sent_messages: Arc<RwLock<HashMap<String, String>>>,
}

impl MsTeamsChannel {
    /// Create a new Teams channel instance.
    pub fn new(id: &str, config: MsTeamsConfig) -> Self {
        let token_cache = Arc::new(TokenCache::new(
            config.app_id.clone(),
            config.app_password.clone(),
            config.tenant_id.clone(),
        ));

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Microsoft Teams".to_string(),
            channel_type: "msteams".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            state: ChannelState::new(100),
            client: Arc::new(BotFrameworkClient::new(token_cache)),
            jwt_validator: Arc::new(JwtValidator::new(config.app_id.clone())),
            config,
            conversation_refs: Arc::new(RwLock::new(HashMap::new())),
            sent_messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: false,
            replies: true,
            editing: true,
            deletion: true,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true,
            max_message_length: 28_000,
            max_attachment_size: 4_194_304,
            stream_protocol: StreamProtocol::Native,
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Cache a conversation reference, evicting the oldest entry if over capacity.
    async fn cache_conversation_ref(
        &self,
        conversation_id: &str,
        service_url: &str,
        bot_id: &str,
    ) {
        let mut refs = self.conversation_refs.write().await;

        // Update existing or insert new
        let entry = refs.entry(conversation_id.to_string()).or_insert_with(|| {
            ConversationReference {
                service_url: String::new(),
                conversation_id: conversation_id.to_string(),
                bot_id: String::new(),
                last_seen: Instant::now(),
            }
        });
        entry.service_url = service_url.to_string();
        entry.bot_id = bot_id.to_string();
        entry.last_seen = Instant::now();

        // Evict oldest if over capacity
        if refs.len() > MAX_CONVERSATION_REFS {
            if let Some(oldest_key) = refs
                .iter()
                .min_by_key(|(_, v)| v.last_seen)
                .map(|(k, _)| k.clone())
            {
                refs.remove(&oldest_key);
            }
        }
    }

    /// Look up the cached service URL for a conversation.
    async fn get_service_url(&self, conversation_id: &str) -> Option<String> {
        let refs = self.conversation_refs.read().await;
        refs.get(conversation_id).map(|r| r.service_url.clone())
    }

    /// Convert an OutboundMessage into a Bot Framework Activity.
    fn build_outbound_activity(message: &OutboundMessage) -> Activity {
        let mut activity = Activity::text_message(&message.text);

        // Set reply-to if present
        if let Some(ref reply_to) = message.reply_to {
            activity.reply_to_id = Some(reply_to.as_str().to_string());
        }

        // Convert attachments
        if !message.attachments.is_empty() {
            let mut att_list = Vec::new();
            for att in &message.attachments {
                att_list.push(ActivityAttachment {
                    content_type: att.mime_type.clone(),
                    content_url: att.url.clone(),
                    content: None,
                    name: att.filename.clone(),
                });
            }
            activity.attachments = Some(att_list);
        }

        // Inject AI-generated entity
        inject_ai_entity(&mut activity);

        activity
    }
}

// ── Channel trait ────────────────────────────────────────────────────────────

#[async_trait]
impl Channel for MsTeamsChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        info!(channel_id = %self.info.id, "Starting Microsoft Teams channel");

        // Pre-fetch JWKS keys for inbound JWT validation
        if let Err(e) = self.jwt_validator.refresh_keys().await {
            warn!(error = %e, "Failed to refresh JWT keys on startup — webhook verification will fail until keys are loaded");
        }

        // Background task: refresh JWKS keys every 12h or on-demand when signature mismatch
        let validator = Arc::clone(&self.jwt_validator);
        let status_handle = self.state.status_handle();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(12 * 3600));
            interval.tick().await; // skip immediate tick
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        debug!("Periodic JWKS key refresh");
                    }
                    _ = async {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            if validator.key_refresh_needed() { break; }
                        }
                    } => {
                        info!("On-demand JWKS key refresh triggered by signature mismatch");
                    }
                }
                if status_handle.try_read().map(|s| *s == ChannelStatus::Disconnected).unwrap_or(false) {
                    break;
                }
                if let Err(e) = validator.refresh_keys().await {
                    warn!(error = %e, "Background JWKS key refresh failed");
                }
            }
        });

        self.state.set_status(ChannelStatus::Connected).await;
        info!(channel_id = %self.info.id, "Microsoft Teams channel connected");
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        info!(channel_id = %self.info.id, "Stopping Microsoft Teams channel");
        self.state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let conversation_id = message.conversation_id.as_str();
        let service_url = self
            .get_service_url(conversation_id)
            .await
            .ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "No cached service URL for conversation '{}'",
                    conversation_id
                ))
            })?;

        let activity = Self::build_outbound_activity(&message);

        let resp = if let Some(ref reply_to) = message.reply_to {
            self.client
                .reply_to_activity(
                    &service_url,
                    conversation_id,
                    reply_to.as_str(),
                    &activity,
                )
                .await?
        } else {
            self.client
                .send_activity(&service_url, conversation_id, &activity)
                .await?
        };

        // Track sent message for delete-by-id
        {
            let mut sent = self.sent_messages.write().await;
            sent.insert(resp.id.clone(), conversation_id.to_string());
            // Cap at 10k entries
            if sent.len() > MAX_CONVERSATION_REFS {
                // Remove arbitrary entry (not worth tracking age for this)
                if let Some(key) = sent.keys().next().cloned() {
                    sent.remove(&key);
                }
            }
        }

        Ok(SendResult {
            message_id: MessageId::new(&resp.id),
            timestamp: Utc::now(),
        })
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        let service_url = self
            .get_service_url(conversation_id.as_str())
            .await
            .ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "No cached service URL for conversation '{}'",
                    conversation_id.as_str()
                ))
            })?;

        self.client
            .send_typing(&service_url, conversation_id.as_str())
            .await
    }

    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        let service_url = self
            .get_service_url(conversation_id.as_str())
            .await
            .ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "No cached service URL for conversation '{}'",
                    conversation_id.as_str()
                ))
            })?;

        let mut activity = Activity::text_message(new_text);
        inject_ai_entity(&mut activity);

        self.client
            .update_activity(
                &service_url,
                conversation_id.as_str(),
                message_id.as_str(),
                &activity,
            )
            .await
    }

    async fn delete(&self, conversation_id: &ConversationId, message_id: &MessageId) -> ChannelResult<()> {
        // Use provided conversation_id, fall back to sent-message cache
        let conversation_id = if !conversation_id.as_str().is_empty() {
            conversation_id.as_str().to_string()
        } else {
            let sent = self.sent_messages.read().await;
            sent.get(message_id.as_str()).cloned().ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "Cannot delete message '{}': no conversation context",
                    message_id.as_str()
                ))
            })?
        };

        let service_url = self
            .get_service_url(&conversation_id)
            .await
            .ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "No cached service URL for conversation '{}'",
                    conversation_id
                ))
            })?;

        self.client
            .delete_activity(&service_url, &conversation_id, message_id.as_str())
            .await
    }

    fn native_stream_handler(&self) -> Option<Arc<dyn NativeStreamHandler>> {
        Some(Arc::new(streaming::MsTeamsStreamHandler::new(
            Arc::clone(&self.client),
            Arc::clone(&self.conversation_refs),
        )))
    }
}

// ── WebhookHandler trait ─────────────────────────────────────────────────────

#[async_trait]
impl WebhookHandler for MsTeamsChannel {
    fn verify(&self, headers: &HeaderMap, _body: &[u8]) -> bool {
        let token = match headers.get("authorization").and_then(|v| v.to_str().ok()) {
            Some(auth) => {
                if let Some(token) = auth.strip_prefix("Bearer ") {
                    token
                } else {
                    warn!("Teams webhook: Authorization header missing Bearer prefix");
                    return false;
                }
            }
            None => {
                warn!("Teams webhook: no Authorization header");
                return false;
            }
        };

        let valid = self.jwt_validator.validate(token);
        if !valid {
            warn!("Teams webhook: JWT validation failed");
        }
        valid
    }

    async fn handle(
        &self,
        _headers: &HeaderMap,
        body: Bytes,
    ) -> ChannelResult<Vec<InboundMessage>> {
        let activity: Activity = serde_json::from_slice(&body).map_err(|e| {
            ChannelError::ReceiveFailed(format!("Failed to parse Activity JSON: {e}"))
        })?;

        match activity.activity_type.as_str() {
            "message" => self.handle_message(activity).await,
            "conversationUpdate" => {
                self.handle_conversation_update(activity);
                Ok(vec![])
            }
            other => {
                debug!(activity_type = %other, "Ignoring unhandled activity type");
                Ok(vec![])
            }
        }
    }

    fn path(&self) -> &str {
        &self.config.webhook_path
    }
}

// ── Activity handlers ────────────────────────────────────────────────────────

impl MsTeamsChannel {
    async fn handle_message(&self, activity: Activity) -> ChannelResult<Vec<InboundMessage>> {
        let from = activity.from.as_ref().ok_or_else(|| {
            ChannelError::ReceiveFailed("Message activity missing 'from' field".into())
        })?;
        let conversation = activity.conversation.as_ref().ok_or_else(|| {
            ChannelError::ReceiveFailed("Message activity missing 'conversation' field".into())
        })?;

        // Access control: check user allowlist
        // When allowed_users is non-empty, sender MUST have a valid aad_object_id
        // in the list. Missing aad_object_id with active allowlist = reject.
        if !self.config.allowed_users.is_empty() {
            match from.aad_object_id.as_deref() {
                Some(aad_id) if self.config.is_user_allowed(aad_id) => {}
                Some(aad_id) => {
                    debug!(aad_id = %aad_id, "Dropping message from disallowed user");
                    return Ok(vec![]);
                }
                None => {
                    debug!(user_id = %from.id, "Dropping message: no aad_object_id but allowlist is active");
                    return Ok(vec![]);
                }
            }
        }

        // Access control: check conversation type
        if !self
            .config
            .is_conversation_allowed(conversation.conversation_type.as_deref())
        {
            debug!(
                conversation_type = ?conversation.conversation_type,
                "Dropping message from disallowed conversation type"
            );
            return Ok(vec![]);
        }

        // Validate service URL
        let service_url = activity.service_url.as_deref().unwrap_or_default();
        if !validate_service_url(service_url) {
            warn!(
                service_url = %service_url,
                "Rejecting activity with untrusted service URL"
            );
            return Err(ChannelError::ReceiveFailed(format!(
                "Untrusted service URL: {service_url}"
            )));
        }

        // Cache conversation reference
        let bot_id = activity
            .recipient
            .as_ref()
            .map(|r| r.id.as_str())
            .unwrap_or_default();
        self.cache_conversation_ref(&conversation.id, service_url, bot_id)
            .await;

        // Strip @mentions and get text
        let raw_text = activity.text.as_deref().unwrap_or_default();
        let text = strip_mentions(raw_text);

        if text.is_empty() {
            // Empty text (e.g., sticker-only or mention-only) — skip
            return Ok(vec![]);
        }

        // Convert attachments
        let attachments = activity
            .attachments
            .as_ref()
            .map(|atts| {
                atts.iter()
                    .filter_map(|a| {
                        a.content_url.as_ref().map(|url| Attachment {
                            id: uuid::Uuid::new_v4().to_string(),
                            mime_type: a.content_type.clone(),
                            filename: a.name.clone(),
                            size: None,
                            url: Some(url.clone()),
                            path: None,
                            data: None,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let is_group = matches!(
            conversation.conversation_type.as_deref(),
            Some("groupChat") | Some("channel")
        );

        let msg = InboundMessage {
            id: MessageId::new(activity.id.as_deref().unwrap_or("unknown")),
            channel_id: self.info.id.clone(),
            conversation_id: ConversationId::new(&conversation.id),
            sender_id: UserId::new(&from.id),
            sender_name: from.name.clone(),
            text,
            attachments,
            timestamp: Utc::now(),
            reply_to: activity.reply_to_id.as_deref().map(MessageId::new),
            is_group,
            raw: Some(serde_json::to_value(&activity).unwrap_or_default()),
        };

        Ok(vec![msg])
    }

    fn handle_conversation_update(&self, activity: Activity) {
        // Check if the bot was added to the conversation
        let bot_id = activity
            .recipient
            .as_ref()
            .map(|r| r.id.clone())
            .unwrap_or_default();

        let bot_was_added = activity
            .members_added
            .as_ref()
            .map(|members| members.iter().any(|m| m.id == bot_id))
            .unwrap_or(false);

        if !bot_was_added {
            return;
        }

        let service_url = match activity.service_url.as_deref() {
            Some(url) if validate_service_url(url) => url.to_string(),
            _ => return,
        };

        let conversation_id = match activity.conversation.as_ref() {
            Some(c) => c.id.clone(),
            None => return,
        };

        // Cache the conversation ref
        let refs = Arc::clone(&self.conversation_refs);
        let client = Arc::clone(&self.client);
        let conv_id = conversation_id.clone();
        let bot_id_clone = bot_id.clone();
        let svc_url = service_url.clone();

        // Fire-and-forget: send welcome card
        tokio::spawn(async move {
            // Cache the ref
            {
                let mut map = refs.write().await;
                map.insert(
                    conv_id.clone(),
                    ConversationReference {
                        service_url: svc_url.clone(),
                        conversation_id: conv_id.clone(),
                        bot_id: bot_id_clone,
                        last_seen: Instant::now(),
                    },
                );
            }

            let card = build_welcome_card("Aleph", &["What can you do?", "Help me with a task"]);
            let mut activity = Activity {
                activity_type: "message".into(),
                attachments: Some(vec![ActivityAttachment {
                    content_type: "application/vnd.microsoft.card.adaptive".into(),
                    content_url: None,
                    content: Some(card),
                    name: None,
                }]),
                ..Default::default()
            };
            inject_ai_entity(&mut activity);

            if let Err(e) = client.send_activity(&svc_url, &conv_id, &activity).await {
                warn!(error = %e, conversation_id = %conv_id, "Failed to send welcome card");
            } else {
                debug!(conversation_id = %conv_id, "Sent welcome card");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_channel() {
        let config = MsTeamsConfig {
            app_id: "test-app-id".into(),
            app_password: "test-secret".into(),
            ..Default::default()
        };
        let channel = MsTeamsChannel::new("teams-1", config);

        assert_eq!(channel.info.id.as_str(), "teams-1");
        assert_eq!(channel.info.name, "Microsoft Teams");
        assert_eq!(channel.info.channel_type, "msteams");
        assert_eq!(channel.info.status, ChannelStatus::Disconnected);
    }

    #[test]
    fn test_webhook_verify_no_auth_header() {
        let config = MsTeamsConfig {
            app_id: "test-app-id".into(),
            app_password: "test-secret".into(),
            ..Default::default()
        };
        let channel = MsTeamsChannel::new("teams-1", config);

        let headers = HeaderMap::new();
        assert!(!channel.verify(&headers, b"{}"));
    }

    #[test]
    fn test_webhook_verify_no_bearer_prefix() {
        let config = MsTeamsConfig {
            app_id: "test-app-id".into(),
            app_password: "test-secret".into(),
            ..Default::default()
        };
        let channel = MsTeamsChannel::new("teams-1", config);

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Basic abc123".parse().unwrap());
        assert!(!channel.verify(&headers, b"{}"));
    }

    #[test]
    fn test_capabilities() {
        let caps = MsTeamsChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(caps.audio);
        assert!(caps.video);
        assert!(!caps.reactions);
        assert!(caps.replies);
        assert!(caps.editing);
        assert!(caps.deletion);
        assert!(caps.typing_indicator);
        assert!(!caps.read_receipts);
        assert!(caps.rich_text);
        assert_eq!(caps.max_message_length, 28_000);
        assert_eq!(caps.max_attachment_size, 4_194_304);
        assert_eq!(caps.stream_protocol, StreamProtocol::Native);
    }

    #[test]
    fn test_build_outbound_activity() {
        let msg = OutboundMessage::text("conv-1", "Hello Teams!");
        let activity = MsTeamsChannel::build_outbound_activity(&msg);

        assert_eq!(activity.activity_type, "message");
        assert_eq!(activity.text.as_deref(), Some("Hello Teams!"));
        assert!(activity.entities.is_some(), "Should have AI entity");
    }

    #[test]
    fn test_build_outbound_activity_with_reply() {
        let msg = OutboundMessage::text("conv-1", "Reply text").with_reply_to("msg-42");
        let activity = MsTeamsChannel::build_outbound_activity(&msg);

        assert_eq!(activity.reply_to_id.as_deref(), Some("msg-42"));
    }

    #[test]
    fn test_webhook_path() {
        let config = MsTeamsConfig {
            app_id: "app".into(),
            app_password: "pass".into(),
            webhook_path: "/custom/teams".into(),
            ..Default::default()
        };
        let channel = MsTeamsChannel::new("teams-1", config);
        assert_eq!(channel.path(), "/custom/teams");
    }

    #[tokio::test]
    async fn test_cache_conversation_ref() {
        let config = MsTeamsConfig {
            app_id: "app".into(),
            app_password: "pass".into(),
            ..Default::default()
        };
        let channel = MsTeamsChannel::new("teams-1", config);

        channel
            .cache_conversation_ref("conv-1", "https://smba.trafficmanager.net/amer/", "bot-1")
            .await;

        let url = channel.get_service_url("conv-1").await;
        assert_eq!(
            url.as_deref(),
            Some("https://smba.trafficmanager.net/amer/")
        );

        assert!(channel.get_service_url("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_handle_message_access_control() {
        let config = MsTeamsConfig {
            app_id: "app".into(),
            app_password: "pass".into(),
            allowed_users: vec!["allowed-user-aad".into()],
            ..Default::default()
        };
        let channel = MsTeamsChannel::new("teams-1", config);

        // Message from disallowed user
        let activity = Activity {
            activity_type: "message".into(),
            text: Some("Hello".into()),
            service_url: Some("https://smba.trafficmanager.net/amer/".into()),
            from: Some(ChannelAccount {
                id: "user-1".into(),
                name: Some("Disallowed User".into()),
                aad_object_id: Some("disallowed-aad".into()),
            }),
            conversation: Some(ConversationAccount {
                id: "conv-1".into(),
                name: None,
                conversation_type: Some("personal".into()),
                tenant_id: None,
                is_group: None,
            }),
            recipient: Some(ChannelAccount {
                id: "bot-id".into(),
                name: Some("Bot".into()),
                aad_object_id: None,
            }),
            ..Default::default()
        };

        let result = channel.handle_message(activity).await.unwrap();
        assert!(result.is_empty(), "Message from disallowed user should be dropped");
    }

    #[tokio::test]
    async fn test_handle_message_success() {
        let config = MsTeamsConfig {
            app_id: "app".into(),
            app_password: "pass".into(),
            ..Default::default()
        };
        let channel = MsTeamsChannel::new("teams-1", config);

        let activity = Activity {
            activity_type: "message".into(),
            id: Some("msg-123".into()),
            text: Some("<at>Aleph</at> Hello bot".into()),
            service_url: Some("https://smba.trafficmanager.net/amer/".into()),
            from: Some(ChannelAccount {
                id: "user-1".into(),
                name: Some("Test User".into()),
                aad_object_id: Some("aad-123".into()),
            }),
            conversation: Some(ConversationAccount {
                id: "19:conv@thread.v2".into(),
                name: None,
                conversation_type: Some("personal".into()),
                tenant_id: None,
                is_group: None,
            }),
            recipient: Some(ChannelAccount {
                id: "bot-id".into(),
                name: Some("Aleph".into()),
                aad_object_id: None,
            }),
            ..Default::default()
        };

        let result = channel.handle_message(activity).await.unwrap();
        assert_eq!(result.len(), 1);

        let msg = &result[0];
        assert_eq!(msg.text, "Hello bot");
        assert_eq!(msg.sender_id.as_str(), "user-1");
        assert_eq!(msg.conversation_id.as_str(), "19:conv@thread.v2");
        assert!(!msg.is_group);

        // Verify conversation ref was cached
        let url = channel.get_service_url("19:conv@thread.v2").await;
        assert!(url.is_some());
    }
}
