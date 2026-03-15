//! Permission checking for inbound messages

use std::collections::HashMap;
use tracing::{info, warn};

use crate::gateway::channel::OutboundMessage;
use crate::gateway::inbound_context::InboundContext;

use super::types::{ChannelConfig, DmPolicy, GroupPolicy, RoutingError};
use super::InboundMessageRouter;

#[cfg(target_os = "macos")]
use crate::gateway::interfaces::imessage::normalize_phone;

#[cfg(not(target_os = "macos"))]
use super::normalize_phone;

impl InboundMessageRouter {
    /// Check if message is permitted
    pub(super) async fn check_permission(&self, mut ctx: InboundContext) -> Result<InboundContext, RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let channel_config = self
            .channel_configs
            .get(channel_id)
            .cloned()
            .unwrap_or_default();

        if ctx.message.is_group {
            // Group message permission check
            match channel_config.group_policy {
                GroupPolicy::Disabled => {
                    return Err(RoutingError::PermissionDenied(
                        "Group messages disabled".to_string(),
                    ));
                }
                GroupPolicy::Allowlist => {
                    let chat_id = ctx.message.conversation_id.as_str();
                    if !channel_config.group_allow_from.iter().any(|a| a == chat_id) {
                        return Err(RoutingError::PermissionDenied(
                            "Group not in allowlist".to_string(),
                        ));
                    }
                }
                GroupPolicy::Open => {
                    // Check mention requirement
                    if channel_config.require_mention {
                        let mentioned = self.check_mention(&ctx.message.text, &channel_config);
                        if !mentioned {
                            return Err(RoutingError::PermissionDenied(
                                "Mention required in group".to_string(),
                            ));
                        }
                        ctx = ctx.with_mention(true);
                    }
                }
            }
        } else {
            // DM permission check
            match channel_config.dm_policy {
                DmPolicy::Disabled => {
                    return Err(RoutingError::PermissionDenied(
                        "DMs disabled".to_string(),
                    ));
                }
                DmPolicy::Open => {
                    // Always allow
                }
                DmPolicy::Allowlist => {
                    if !self.is_in_allowlist(&ctx.sender_normalized, &channel_config.allow_from) {
                        return Err(RoutingError::PermissionDenied(
                            "Sender not in allowlist".to_string(),
                        ));
                    }
                }
                DmPolicy::Pairing => {
                    // Check allowlist first
                    if self.is_in_allowlist(&ctx.sender_normalized, &channel_config.allow_from) {
                        // Already approved via config
                    } else if self.pairing_store.is_approved(channel_id, &ctx.sender_normalized).await? {
                        // Approved via pairing
                    } else {
                        // Need pairing
                        self.send_pairing_request(&ctx).await?;
                        return Err(RoutingError::PermissionDenied(
                            "Pairing required".to_string(),
                        ));
                    }
                }
            }
        }

        ctx = ctx.authorize();
        Ok(ctx)
    }

    /// Check if sender is in allowlist
    pub(super) fn is_in_allowlist(&self, sender: &str, allowlist: &[String]) -> bool {
        if allowlist.is_empty() {
            return false;
        }
        if allowlist.iter().any(|a| a == "*") {
            return true;
        }

        // Normalize both for comparison
        let sender_normalized = normalize_phone(sender);
        allowlist.iter().any(|a| {
            let allowed_normalized = normalize_phone(a);
            sender == a
                || sender.to_lowercase() == a.to_lowercase()
                || (!sender_normalized.is_empty()
                    && !allowed_normalized.is_empty()
                    && sender_normalized == allowed_normalized)
        })
    }

    /// Check if bot was mentioned in message
    pub(super) fn check_mention(&self, text: &str, config: &ChannelConfig) -> bool {
        let text_lower = text.to_lowercase();

        // Check bot name
        if let Some(bot_name) = &config.bot_name {
            if text_lower.contains(&bot_name.to_lowercase()) {
                return true;
            }
        }

        // Check common patterns
        let patterns = ["@aleph", "@bot", "aleph"];
        patterns.iter().any(|p| text_lower.contains(p))
    }

    /// Send pairing request to unknown sender
    ///
    /// Always sends the pairing code message, even if the request already exists
    /// (the initial delivery may have failed due to channel not being connected).
    pub(super) async fn send_pairing_request(&self, ctx: &InboundContext) -> Result<(), RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let sender_id = &ctx.sender_normalized;

        let mut metadata = HashMap::new();
        metadata.insert("sender_display".to_string(), ctx.message.sender_id.as_str().to_string());

        let (code, created) = self
            .pairing_store
            .upsert(channel_id, sender_id, metadata)
            .await?;

        if created {
            info!("Created new pairing request for {}:{} with code {}", channel_id, sender_id, code);
        } else {
            info!("Resending existing pairing code for {}:{}", channel_id, sender_id);
        }

        // Always send the pairing message (not just on first create)
        // because the initial delivery may have failed.
        let message = format!(
            "Hi! I'm Aleph, a personal AI assistant.\n\n\
            To chat with me, please have my owner approve your access.\n\n\
            Your ID: {}\n\
            Pairing code: {}\n\n\
            Once approved, just send me a message!",
            sender_id, code
        );

        let outbound = OutboundMessage::text(
            ctx.reply_route.conversation_id.as_str(),
            message,
        );

        if let Err(e) = self
            .channel_registry
            .send(&ctx.reply_route.channel_id, outbound)
            .await
        {
            warn!("Failed to send pairing message to {}:{}: {}", channel_id, sender_id, e);
        } else {
            info!("Sent pairing code {} to {}:{}", code, channel_id, sender_id);
        }

        Ok(())
    }
}
