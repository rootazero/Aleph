//! Microsoft Teams Message Operations
//!
//! Helper struct for message send/reply/edit/delete operations via
//! the Bot Framework REST API. Separated from the channel struct for testability.

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

use crate::gateway::channel::ChannelError;

use super::api::BotFrameworkClient;
use super::types::{Activity, inject_ai_entity};
use super::ConversationReference;

/// Microsoft Teams message operations helper.
///
/// Wraps the `BotFrameworkClient` and conversation reference cache to provide
/// high-level send/reply/edit/delete methods.
pub struct MsTeamsMessageOps {
    client: Arc<BotFrameworkClient>,
    conversation_refs: Arc<RwLock<HashMap<String, ConversationReference>>>,
}

impl MsTeamsMessageOps {
    pub fn new(
        client: Arc<BotFrameworkClient>,
        conversation_refs: Arc<RwLock<HashMap<String, ConversationReference>>>,
    ) -> Self {
        Self { client, conversation_refs }
    }

    /// Resolve the cached service URL for a conversation.
    async fn get_service_url(&self, conversation_id: &str) -> std::result::Result<String, ChannelError> {
        let refs = self.conversation_refs.read().await;
        refs.get(conversation_id)
            .map(|r| r.service_url.clone())
            .ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "No conversation reference for '{}'", conversation_id
                ))
            })
    }

    /// Send a new message to a conversation.
    pub async fn send(&self, conversation_id: &str, text: &str) -> Result<String, ChannelError> {
        let service_url = self.get_service_url(conversation_id).await?;
        let mut activity = Activity::text_message(text);
        inject_ai_entity(&mut activity);
        let resp = self.client.send_activity(&service_url, conversation_id, &activity).await?;
        Ok(resp.id)
    }

    /// Reply to a specific message in a conversation.
    pub async fn reply(
        &self,
        conversation_id: &str,
        message_id: &str,
        text: &str,
    ) -> Result<String, ChannelError> {
        let service_url = self.get_service_url(conversation_id).await?;
        let mut activity = Activity::text_message(text);
        activity.reply_to_id = Some(message_id.to_string());
        inject_ai_entity(&mut activity);
        let resp = self.client
            .reply_to_activity(&service_url, conversation_id, message_id, &activity)
            .await?;
        Ok(resp.id)
    }

    /// Edit an existing message.
    pub async fn edit(
        &self,
        conversation_id: &str,
        message_id: &str,
        text: &str,
    ) -> Result<(), ChannelError> {
        let service_url = self.get_service_url(conversation_id).await?;
        let mut activity = Activity::text_message(text);
        inject_ai_entity(&mut activity);
        self.client
            .update_activity(&service_url, conversation_id, message_id, &activity)
            .await?;
        Ok(())
    }

    /// Delete a message.
    pub async fn delete(&self, conversation_id: &str, message_id: &str) -> Result<(), ChannelError> {
        let service_url = self.get_service_url(conversation_id).await?;
        self.client
            .delete_activity(&service_url, conversation_id, message_id)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ops() -> MsTeamsMessageOps {
        let tc = Arc::new(super::super::auth::TokenCache::new(
            "a".into(), "b".into(), "c".into(),
        ));
        let client = Arc::new(BotFrameworkClient::new(tc));
        MsTeamsMessageOps::new(client, Arc::new(RwLock::new(HashMap::new())))
    }

    #[tokio::test]
    async fn test_get_service_url_missing() {
        let ops = make_ops();
        let result = ops.get_service_url("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_service_url_found() {
        let ops = make_ops();
        {
            let mut refs = ops.conversation_refs.write().await;
            refs.insert("conv-1".into(), ConversationReference {
                service_url: "https://smba.trafficmanager.net/amer/".into(),
                conversation_id: "conv-1".into(),
                bot_id: "bot-1".into(),
                last_seen: std::time::Instant::now(),
            });
        }
        let url = ops.get_service_url("conv-1").await.unwrap();
        assert_eq!(url, "https://smba.trafficmanager.net/amer/");
    }
}
