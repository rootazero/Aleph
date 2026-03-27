//! Microsoft Teams MessageOperations Implementation
//!
//! Implements the MessageOperations trait for Teams via Bot Framework REST API v3.

use async_trait::async_trait;
use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::builtin_tools::message::{
    ChannelCapabilities, DeleteParams, EditParams, MessageOperations, MessageResult, ReactParams,
    ReplyParams, SendParams,
};
use crate::error::Result;

use super::api::BotFrameworkClient;
use super::types::{Activity, inject_ai_entity};

// ── ConversationReference ────────────────────────────────────────────────────

/// Minimal conversation reference for proactive messaging.
///
/// Mirrors the private `ConversationReference` in `mod.rs` — duplicated here
/// so `MsTeamsMessageOps` can be constructed independently (e.g., in tests).
#[derive(Debug, Clone)]
pub struct ConversationReference {
    pub service_url: String,
    pub conversation_id: String,
    pub bot_id: String,
    pub last_seen: Instant,
}

// ── MsTeamsMessageOps ────────────────────────────────────────────────────────

/// Teams adapter implementing the cross-channel `MessageOperations` trait.
pub struct MsTeamsMessageOps {
    client: Arc<BotFrameworkClient>,
    conversation_refs: Arc<RwLock<HashMap<String, ConversationReference>>>,
}

impl MsTeamsMessageOps {
    /// Create a new adapter wrapping an existing Bot Framework client and
    /// conversation-reference cache.
    pub fn new(
        client: Arc<BotFrameworkClient>,
        conversation_refs: Arc<RwLock<HashMap<String, ConversationReference>>>,
    ) -> Self {
        Self { client, conversation_refs }
    }

    /// Look up a cached `ConversationReference` by conversation ID.
    async fn get_conversation_ref(&self, conversation_id: &str) -> Result<ConversationReference> {
        let refs = self.conversation_refs.read().await;
        refs.get(conversation_id).cloned().ok_or_else(|| {
            crate::error::AlephError::invalid_input(format!(
                "No conversation reference found for '{conversation_id}'"
            ))
        })
    }
}

// ── MessageOperations impl ───────────────────────────────────────────────────

#[async_trait]
impl MessageOperations for MsTeamsMessageOps {
    fn channel_id(&self) -> &str {
        "msteams"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            reply: true,
            edit: true,
            react: false, // Teams reactions not exposed via Bot Framework REST API
            delete: true,
            send: true,
        }
    }

    async fn reply(&self, params: ReplyParams) -> Result<MessageResult> {
        let conv_ref = self.get_conversation_ref(&params.conversation_id).await?;
        let mut activity = Activity::text_message(&params.text);
        activity.reply_to_id = Some(params.message_id.clone());
        inject_ai_entity(&mut activity);

        match self
            .client
            .reply_to_activity(
                &conv_ref.service_url,
                &conv_ref.conversation_id,
                &params.message_id,
                &activity,
            )
            .await
        {
            Ok(resp) => Ok(MessageResult::success_with_id(&resp.id)),
            Err(e) => Ok(MessageResult::failed(format!("Reply failed: {e}"))),
        }
    }

    async fn edit(&self, params: EditParams) -> Result<MessageResult> {
        let conv_ref = self.get_conversation_ref(&params.conversation_id).await?;
        let mut activity = Activity::text_message(&params.text);
        inject_ai_entity(&mut activity);

        match self
            .client
            .update_activity(
                &conv_ref.service_url,
                &conv_ref.conversation_id,
                &params.message_id,
                &activity,
            )
            .await
        {
            Ok(()) => Ok(MessageResult::success_with_id(&params.message_id)),
            Err(e) => Ok(MessageResult::failed(format!("Edit failed: {e}"))),
        }
    }

    async fn react(&self, _params: ReactParams) -> Result<MessageResult> {
        // Teams does not expose message reactions via the Bot Framework REST API.
        Ok(MessageResult::failed(
            "Reactions are not supported for Microsoft Teams",
        ))
    }

    async fn delete(&self, params: DeleteParams) -> Result<MessageResult> {
        let conv_ref = self.get_conversation_ref(&params.conversation_id).await?;

        match self
            .client
            .delete_activity(
                &conv_ref.service_url,
                &conv_ref.conversation_id,
                &params.message_id,
            )
            .await
        {
            Ok(()) => Ok(MessageResult::success()),
            Err(e) => Ok(MessageResult::failed(format!("Delete failed: {e}"))),
        }
    }

    async fn send(&self, params: SendParams) -> Result<MessageResult> {
        let conv_ref = self.get_conversation_ref(&params.target).await?;
        let mut activity = Activity::text_message(&params.text);
        inject_ai_entity(&mut activity);

        match self
            .client
            .send_activity(&conv_ref.service_url, &conv_ref.conversation_id, &activity)
            .await
        {
            Ok(resp) => Ok(MessageResult::success_with_id(&resp.id)),
            Err(e) => Ok(MessageResult::failed(format!("Send failed: {e}"))),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::auth::TokenCache;

    fn make_ops() -> MsTeamsMessageOps {
        let token_cache = Arc::new(TokenCache::new(
            "test-app".into(),
            "test-password".into(),
            "common".into(),
        ));
        let client = Arc::new(BotFrameworkClient::new(token_cache));
        let refs = Arc::new(RwLock::new(HashMap::new()));
        MsTeamsMessageOps::new(client, refs)
    }

    #[test]
    fn test_channel_id() {
        let ops = make_ops();
        assert_eq!(ops.channel_id(), "msteams");
    }

    #[test]
    fn test_capabilities() {
        let ops = make_ops();
        let caps = ops.capabilities();
        assert!(caps.reply);
        assert!(caps.edit);
        assert!(!caps.react, "Teams does not support reactions");
        assert!(caps.delete);
        assert!(caps.send);
    }

    #[tokio::test]
    async fn test_get_conversation_ref_missing() {
        let ops = make_ops();
        let result = ops.get_conversation_ref("nonexistent").await;
        assert!(result.is_err(), "Should fail for unknown conversation");
    }

    #[tokio::test]
    async fn test_get_conversation_ref_present() {
        let ops = make_ops();
        {
            let mut refs = ops.conversation_refs.write().await;
            refs.insert(
                "conv-1".into(),
                ConversationReference {
                    service_url: "https://smba.trafficmanager.net/amer/".into(),
                    conversation_id: "conv-1".into(),
                    bot_id: "bot-id".into(),
                    last_seen: Instant::now(),
                },
            );
        }
        let result = ops.get_conversation_ref("conv-1").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().service_url, "https://smba.trafficmanager.net/amer/");
    }

    #[tokio::test]
    async fn test_react_unsupported() {
        let ops = make_ops();
        let params = ReactParams {
            channel: "msteams".into(),
            conversation_id: "conv-1".into(),
            message_id: "msg-1".into(),
            emoji: "👍".into(),
            remove: false,
        };
        let result = ops.react(params).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not supported"));
    }
}
