//! Discord MessageOperations Implementation
//!
//! Implements the MessageOperations trait for Discord API via serenity.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::builtin_tools::message::{
    ChannelCapabilities, DeleteParams, EditParams, MessageOperations, MessageResult, ReactParams,
    ReplyParams, SendParams,
};
use crate::error::Result;

#[cfg(feature = "discord")]
use serenity::{
    all::{ChannelId as SerenityChannelId, CreateMessage, EditMessage, MessageId as SerenityMessageId},
    http::Http,
};

/// Discord message operations adapter
///
/// Wraps a serenity HTTP client to provide MessageOperations functionality.
pub struct DiscordMessageOps {
    #[cfg(feature = "discord")]
    http: Arc<RwLock<Option<Arc<Http>>>>,
    #[cfg(not(feature = "discord"))]
    _phantom: std::marker::PhantomData<()>,
}

impl DiscordMessageOps {
    /// Create a new Discord message operations adapter
    #[cfg(feature = "discord")]
    pub fn new(http: Arc<RwLock<Option<Arc<Http>>>>) -> Self {
        Self { http }
    }

    /// Create a stub adapter when discord feature is disabled
    #[cfg(not(feature = "discord"))]
    pub fn new_stub() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Parse channel ID from string
    #[cfg(feature = "discord")]
    fn parse_channel_id(s: &str) -> Result<SerenityChannelId> {
        s.parse::<u64>()
            .map(SerenityChannelId::new)
            .map_err(|e| {
                crate::error::AetherError::invalid_input(format!("Invalid channel ID: {}", e))
            })
    }

    /// Parse message ID from string
    #[cfg(feature = "discord")]
    fn parse_message_id(s: &str) -> Result<SerenityMessageId> {
        s.parse::<u64>()
            .map(SerenityMessageId::new)
            .map_err(|e| {
                crate::error::AetherError::invalid_input(format!("Invalid message ID: {}", e))
            })
    }
}

#[async_trait]
impl MessageOperations for DiscordMessageOps {
    fn channel_id(&self) -> &str {
        "discord"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            reply: true,
            edit: true,
            react: true,
            delete: true,
            send: true,
        }
    }

    async fn reply(&self, params: ReplyParams) -> Result<MessageResult> {
        #[cfg(feature = "discord")]
        {
            let http_guard = self.http.read().await;
            let http = match http_guard.as_ref() {
                Some(h) => h,
                None => {
                    return Ok(MessageResult::failed("Discord HTTP client not initialized"));
                }
            };

            let channel_id = Self::parse_channel_id(&params.conversation_id)?;
            let reply_to = Self::parse_message_id(&params.message_id)?;

            debug!(
                channel_id = %params.conversation_id,
                reply_to = %params.message_id,
                "Sending Discord reply"
            );

            let builder = CreateMessage::new()
                .content(&params.text)
                .reference_message(serenity::all::MessageReference::from((
                    channel_id,
                    reply_to,
                )));

            match channel_id.send_message(http.as_ref(), builder).await {
                Ok(sent) => {
                    debug!(message_id = %sent.id, "Reply sent successfully");
                    Ok(MessageResult::success_with_id(sent.id.to_string()))
                }
                Err(e) => {
                    warn!(error = %e, "Failed to send Discord reply");
                    Ok(MessageResult::failed(format!("Failed to send reply: {}", e)))
                }
            }
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Discord support not compiled (enable 'discord' feature)",
            ))
        }
    }

    async fn edit(&self, params: EditParams) -> Result<MessageResult> {
        #[cfg(feature = "discord")]
        {
            let http_guard = self.http.read().await;
            let http = match http_guard.as_ref() {
                Some(h) => h,
                None => {
                    return Ok(MessageResult::failed("Discord HTTP client not initialized"));
                }
            };

            let channel_id = Self::parse_channel_id(&params.conversation_id)?;
            let message_id = Self::parse_message_id(&params.message_id)?;

            debug!(
                channel_id = %params.conversation_id,
                message_id = %params.message_id,
                "Editing Discord message"
            );

            let builder = EditMessage::new().content(&params.text);

            match channel_id
                .edit_message(http.as_ref(), message_id, builder)
                .await
            {
                Ok(_) => {
                    debug!("Message edited successfully");
                    Ok(MessageResult::success_with_id(&params.message_id))
                }
                Err(e) => {
                    warn!(error = %e, "Failed to edit Discord message");
                    Ok(MessageResult::failed(format!(
                        "Failed to edit message: {}",
                        e
                    )))
                }
            }
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Discord support not compiled (enable 'discord' feature)",
            ))
        }
    }

    async fn react(&self, params: ReactParams) -> Result<MessageResult> {
        #[cfg(feature = "discord")]
        {
            let http_guard = self.http.read().await;
            let http = match http_guard.as_ref() {
                Some(h) => h,
                None => {
                    return Ok(MessageResult::failed("Discord HTTP client not initialized"));
                }
            };

            let channel_id = Self::parse_channel_id(&params.conversation_id)?;
            let message_id = Self::parse_message_id(&params.message_id)?;

            debug!(
                channel_id = %params.conversation_id,
                message_id = %params.message_id,
                emoji = %params.emoji,
                remove = %params.remove,
                "Setting Discord reaction"
            );

            // Parse emoji - could be Unicode or custom emoji
            let reaction_type = if params.emoji.starts_with('<') {
                // Custom emoji format: <:name:id> or <a:name:id>
                serenity::all::ReactionType::try_from(params.emoji.as_str()).map_err(|e| {
                    crate::error::AetherError::invalid_input(format!("Invalid emoji format: {}", e))
                })?
            } else {
                // Unicode emoji
                serenity::all::ReactionType::Unicode(params.emoji.clone())
            };

            let result = if params.remove {
                // TODO: Implement reaction removal
                // Requires bot user ID which is not currently available in DiscordMessageOps
                // Need to refactor to pass bot_user_id from DiscordChannel
                return Err(crate::error::AetherError::invalid_input(
                    "Removing reactions is not yet implemented for Discord"
                ));
            } else {
                http.create_reaction(channel_id, message_id, &reaction_type)
                    .await
            };

            match result {
                Ok(_) => {
                    debug!("Reaction {} successfully", if params.remove { "removed" } else { "added" });
                    Ok(MessageResult::success())
                }
                Err(e) => {
                    warn!(error = %e, "Failed to set Discord reaction");
                    Ok(MessageResult::failed(format!(
                        "Failed to {} reaction: {}",
                        if params.remove { "remove" } else { "add" },
                        e
                    )))
                }
            }
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Discord support not compiled (enable 'discord' feature)",
            ))
        }
    }

    async fn delete(&self, params: DeleteParams) -> Result<MessageResult> {
        #[cfg(feature = "discord")]
        {
            let http_guard = self.http.read().await;
            let http = match http_guard.as_ref() {
                Some(h) => h,
                None => {
                    return Ok(MessageResult::failed("Discord HTTP client not initialized"));
                }
            };

            let channel_id = Self::parse_channel_id(&params.conversation_id)?;
            let message_id = Self::parse_message_id(&params.message_id)?;

            debug!(
                channel_id = %params.conversation_id,
                message_id = %params.message_id,
                "Deleting Discord message"
            );

            match channel_id.delete_message(http.as_ref(), message_id).await {
                Ok(_) => {
                    debug!("Message deleted successfully");
                    Ok(MessageResult::success())
                }
                Err(e) => {
                    warn!(error = %e, "Failed to delete Discord message");
                    Ok(MessageResult::failed(format!(
                        "Failed to delete message: {}",
                        e
                    )))
                }
            }
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Discord support not compiled (enable 'discord' feature)",
            ))
        }
    }

    async fn send(&self, params: SendParams) -> Result<MessageResult> {
        #[cfg(feature = "discord")]
        {
            let http_guard = self.http.read().await;
            let http = match http_guard.as_ref() {
                Some(h) => h,
                None => {
                    return Ok(MessageResult::failed("Discord HTTP client not initialized"));
                }
            };

            // Handle DM targets (dm:user_id format)
            let channel_id = if params.target.starts_with("dm:") {
                let user_id: u64 = params.target
                    .strip_prefix("dm:")
                    .unwrap()
                    .parse()
                    .map_err(|e| {
                        crate::error::AetherError::invalid_input(format!("Invalid user ID: {}", e))
                    })?;

                let user = serenity::all::UserId::new(user_id);
                match user.create_dm_channel(http.as_ref()).await {
                    Ok(dm) => dm.id,
                    Err(e) => {
                        return Ok(MessageResult::failed(format!(
                            "Failed to create DM channel: {}",
                            e
                        )));
                    }
                }
            } else {
                Self::parse_channel_id(&params.target)?
            };

            debug!(
                target = %params.target,
                "Sending Discord message"
            );

            let builder = CreateMessage::new().content(&params.text);

            match channel_id.send_message(http.as_ref(), builder).await {
                Ok(sent) => {
                    debug!(message_id = %sent.id, "Message sent successfully");
                    Ok(MessageResult::success_with_id(sent.id.to_string()))
                }
                Err(e) => {
                    warn!(error = %e, "Failed to send Discord message");
                    Ok(MessageResult::failed(format!(
                        "Failed to send message: {}",
                        e
                    )))
                }
            }
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Discord support not compiled (enable 'discord' feature)",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities() {
        #[cfg(feature = "discord")]
        {
            let http = Arc::new(RwLock::new(None));
            let ops = DiscordMessageOps::new(http);
            let caps = ops.capabilities();
            assert!(caps.reply);
            assert!(caps.edit);
            assert!(caps.react);
            assert!(caps.delete);
            assert!(caps.send);
        }
    }

    #[test]
    fn test_channel_id() {
        #[cfg(feature = "discord")]
        {
            let http = Arc::new(RwLock::new(None));
            let ops = DiscordMessageOps::new(http);
            assert_eq!(ops.channel_id(), "discord");
        }
    }
}
