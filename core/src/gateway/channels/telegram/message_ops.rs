//! Telegram MessageOperations Implementation
//!
//! Implements the MessageOperations trait for Telegram Bot API.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::builtin_tools::message::{
    ChannelCapabilities, DeleteParams, EditParams, MessageOperations, MessageResult, ReactParams,
    ReplyParams, SendParams,
};
use crate::error::Result;

#[cfg(feature = "telegram")]
use teloxide::{prelude::*, types::ChatId};

/// Telegram message operations adapter
///
/// Wraps a teloxide Bot instance to provide MessageOperations functionality.
pub struct TelegramMessageOps {
    #[cfg(feature = "telegram")]
    bot: Arc<RwLock<Option<Bot>>>,
    #[cfg(not(feature = "telegram"))]
    _phantom: std::marker::PhantomData<()>,
}

impl TelegramMessageOps {
    /// Create a new Telegram message operations adapter
    #[cfg(feature = "telegram")]
    pub fn new(bot: Arc<RwLock<Option<Bot>>>) -> Self {
        Self { bot }
    }

    /// Create a stub adapter when telegram feature is disabled
    #[cfg(not(feature = "telegram"))]
    pub fn new_stub() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Parse chat ID from string
    #[cfg(feature = "telegram")]
    fn parse_chat_id(s: &str) -> Result<ChatId> {
        s.parse::<i64>()
            .map(ChatId)
            .map_err(|e| crate::error::AetherError::invalid_input(format!("Invalid chat ID: {}", e)))
    }

    /// Parse message ID from string
    #[cfg(feature = "telegram")]
    fn parse_message_id(s: &str) -> Result<teloxide::types::MessageId> {
        s.parse::<i32>()
            .map(teloxide::types::MessageId)
            .map_err(|e| {
                crate::error::AetherError::invalid_input(format!("Invalid message ID: {}", e))
            })
    }
}

#[async_trait]
impl MessageOperations for TelegramMessageOps {
    fn channel_id(&self) -> &str {
        "telegram"
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
        #[cfg(feature = "telegram")]
        {
            let bot_guard = self.bot.read().await;
            let bot = match bot_guard.as_ref() {
                Some(b) => b,
                None => {
                    return Ok(MessageResult::failed("Telegram bot not initialized"));
                }
            };

            let chat_id = Self::parse_chat_id(&params.conversation_id)?;
            let reply_to = Self::parse_message_id(&params.message_id)?;

            debug!(
                chat_id = %params.conversation_id,
                reply_to = %params.message_id,
                "Sending Telegram reply"
            );

            match bot
                .send_message(chat_id, &params.text)
                .reply_parameters(teloxide::types::ReplyParameters::new(reply_to))
                .await
            {
                Ok(sent) => {
                    debug!(message_id = %sent.id.0, "Reply sent successfully");
                    Ok(MessageResult::success_with_id(sent.id.0.to_string()))
                }
                Err(e) => {
                    warn!(error = %e, "Failed to send Telegram reply");
                    Ok(MessageResult::failed(format!("Failed to send reply: {}", e)))
                }
            }
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Telegram support not compiled (enable 'telegram' feature)",
            ))
        }
    }

    async fn edit(&self, params: EditParams) -> Result<MessageResult> {
        #[cfg(feature = "telegram")]
        {
            let bot_guard = self.bot.read().await;
            let bot = match bot_guard.as_ref() {
                Some(b) => b,
                None => {
                    return Ok(MessageResult::failed("Telegram bot not initialized"));
                }
            };

            let chat_id = Self::parse_chat_id(&params.conversation_id)?;
            let message_id = Self::parse_message_id(&params.message_id)?;

            debug!(
                chat_id = %params.conversation_id,
                message_id = %params.message_id,
                "Editing Telegram message"
            );

            match bot
                .edit_message_text(chat_id, message_id, &params.text)
                .await
            {
                Ok(_) => {
                    debug!("Message edited successfully");
                    Ok(MessageResult::success_with_id(&params.message_id))
                }
                Err(e) => {
                    warn!(error = %e, "Failed to edit Telegram message");
                    Ok(MessageResult::failed(format!(
                        "Failed to edit message: {}",
                        e
                    )))
                }
            }
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Telegram support not compiled (enable 'telegram' feature)",
            ))
        }
    }

    async fn react(&self, params: ReactParams) -> Result<MessageResult> {
        #[cfg(feature = "telegram")]
        {
            let bot_guard = self.bot.read().await;
            let bot = match bot_guard.as_ref() {
                Some(b) => b,
                None => {
                    return Ok(MessageResult::failed("Telegram bot not initialized"));
                }
            };

            let chat_id = Self::parse_chat_id(&params.conversation_id)?;
            let message_id = Self::parse_message_id(&params.message_id)?;

            debug!(
                chat_id = %params.conversation_id,
                message_id = %params.message_id,
                emoji = %params.emoji,
                remove = %params.remove,
                "Setting Telegram reaction"
            );

            // Build reaction list
            let reactions = if params.remove {
                vec![] // Empty list to remove reactions
            } else {
                vec![teloxide::types::ReactionType::Emoji {
                    emoji: params.emoji.clone(),
                }]
            };

            match bot
                .set_message_reaction(chat_id, message_id)
                .reaction(reactions)
                .await
            {
                Ok(_) => {
                    debug!("Reaction set successfully");
                    Ok(MessageResult::success())
                }
                Err(e) => {
                    warn!(error = %e, "Failed to set Telegram reaction");
                    Ok(MessageResult::failed(format!(
                        "Failed to set reaction: {}",
                        e
                    )))
                }
            }
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Telegram support not compiled (enable 'telegram' feature)",
            ))
        }
    }

    async fn delete(&self, params: DeleteParams) -> Result<MessageResult> {
        #[cfg(feature = "telegram")]
        {
            let bot_guard = self.bot.read().await;
            let bot = match bot_guard.as_ref() {
                Some(b) => b,
                None => {
                    return Ok(MessageResult::failed("Telegram bot not initialized"));
                }
            };

            let chat_id = Self::parse_chat_id(&params.conversation_id)?;
            let message_id = Self::parse_message_id(&params.message_id)?;

            debug!(
                chat_id = %params.conversation_id,
                message_id = %params.message_id,
                "Deleting Telegram message"
            );

            match bot.delete_message(chat_id, message_id).await {
                Ok(_) => {
                    debug!("Message deleted successfully");
                    Ok(MessageResult::success())
                }
                Err(e) => {
                    warn!(error = %e, "Failed to delete Telegram message");
                    Ok(MessageResult::failed(format!(
                        "Failed to delete message: {}",
                        e
                    )))
                }
            }
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Telegram support not compiled (enable 'telegram' feature)",
            ))
        }
    }

    async fn send(&self, params: SendParams) -> Result<MessageResult> {
        #[cfg(feature = "telegram")]
        {
            let bot_guard = self.bot.read().await;
            let bot = match bot_guard.as_ref() {
                Some(b) => b,
                None => {
                    return Ok(MessageResult::failed("Telegram bot not initialized"));
                }
            };

            let chat_id = Self::parse_chat_id(&params.target)?;

            debug!(
                chat_id = %params.target,
                "Sending Telegram message"
            );

            match bot.send_message(chat_id, &params.text).await {
                Ok(sent) => {
                    debug!(message_id = %sent.id.0, "Message sent successfully");
                    Ok(MessageResult::success_with_id(sent.id.0.to_string()))
                }
                Err(e) => {
                    warn!(error = %e, "Failed to send Telegram message");
                    Ok(MessageResult::failed(format!(
                        "Failed to send message: {}",
                        e
                    )))
                }
            }
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = params;
            Ok(MessageResult::failed(
                "Telegram support not compiled (enable 'telegram' feature)",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities() {
        #[cfg(feature = "telegram")]
        {
            let bot = Arc::new(RwLock::new(None));
            let ops = TelegramMessageOps::new(bot);
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
        #[cfg(feature = "telegram")]
        {
            let bot = Arc::new(RwLock::new(None));
            let ops = TelegramMessageOps::new(bot);
            assert_eq!(ops.channel_id(), "telegram");
        }
    }
}
