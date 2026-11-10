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

/// Whether the channel adapter already validated that this message addresses
/// the bot (e.g. Telegram/Slack [`MessageMeta::AppMention`]).
///
/// Channels with richer signal than plain text (entities, reply-to-bot, the
/// real `@username`) gate mentions in their own limb and tag the result; the
/// router trusts that tag rather than re-running the weaker substring
/// heuristic in [`InboundMessageRouter::check_mention`].
fn message_pre_validated_mention(message: &crate::gateway::channel::InboundMessage) -> bool {
    use crate::gateway::channel::MessageMeta;
    message
        .metadata
        .iter()
        .any(|m| matches!(m, MessageMeta::AppMention))
}

/// Strip the punctuation that clings to a mention token — `"@aleph,"`,
/// `"aleph."`, `"（aleph）"` — so the comparison sees the name itself.
///
/// The leading `@` is KEPT: it is what separates an explicitly addressed
/// handle from a bare word, and the bare word is not a mention unless the
/// channel configured the bot's name.
fn mention_token(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphanumeric() && c != '@')
}

/// Whether `tokens` contains `name`'s word sequence as a contiguous run of
/// whole tokens — the whole-word form of `text.contains(name)`.
///
/// A configured name may carry its own `@` prefix (`"@aleph"`); it is
/// stripped here so the comparison sees the name itself, exactly like the
/// tokens do.
fn contains_word_sequence(tokens: &[&str], name: &str) -> bool {
    let name_words: Vec<&str> = name
        .split_whitespace()
        .map(mention_token)
        .map(|word| word.strip_prefix('@').unwrap_or(word))
        .collect();
    if name_words.is_empty() || name_words.len() > tokens.len() {
        return false;
    }
    tokens.windows(name_words.len()).any(|window| {
        window
            .iter()
            .zip(&name_words)
            .all(|(token, word)| token.eq_ignore_ascii_case(word))
    })
}

impl InboundMessageRouter {
    /// Check if message is permitted
    pub(super) async fn check_permission(
        &self,
        mut ctx: InboundContext,
    ) -> Result<InboundContext, RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let mut channel_config = self
            .channel_configs
            .get(channel_id)
            .cloned()
            .unwrap_or_default();
        let mut telegram_chat_id = None;
        if let Some(handle) = self.channel_registry.get(&ctx.message.channel_id).await {
            let channel = handle.read().await;
            if let Some(telegram) = channel.as_any().and_then(|channel| {
                channel.downcast_ref::<crate::gateway::interfaces::telegram::TelegramChannel>()
            }) {
                let config = telegram
                    .inbound_config(&ctx.message)
                    .map_err(|error| RoutingError::PermissionDenied(error.to_string()))?;
                let effective = ChannelConfig::from(config);
                channel_config.dm_policy = effective.dm_policy;
                channel_config.group_policy = effective.group_policy;
                channel_config.allow_from = effective.allow_from;
                channel_config.group_allow_from = effective.group_allow_from;
                channel_config.bot_name = effective.bot_name;
                channel_config.require_mention = telegram.require_mention(&config.account_id);
                telegram_chat_id = Some(
                    crate::gateway::interfaces::telegram::delivery::parse_conversation_id(
                        ctx.message.conversation_id.as_str(),
                    )
                    .map_err(|error| RoutingError::PermissionDenied(error.to_string()))?
                    .0
                     .0
                    .to_string(),
                );
            }
        }

        if ctx.message.is_group {
            // Group message permission check
            match channel_config.group_policy {
                GroupPolicy::Disabled => {
                    return Err(RoutingError::PermissionDenied(
                        "Group messages disabled".to_string(),
                    ));
                }
                GroupPolicy::Allowlist => {
                    let chat_id = telegram_chat_id
                        .as_deref()
                        .unwrap_or(ctx.message.conversation_id.as_str());
                    if !channel_config.group_allow_from.iter().any(|a| a == chat_id) {
                        return Err(RoutingError::PermissionDenied(
                            "Group not in allowlist".to_string(),
                        ));
                    }
                }
                GroupPolicy::Open => {
                    // Check mention requirement
                    if channel_config
                        .room_require_mention
                        .get(ctx.message.conversation_id.as_str())
                        .copied()
                        .unwrap_or(channel_config.require_mention)
                    {
                        // A channel adapter that already validated the mention
                        // with platform-native signals (real @username,
                        // reply-to-bot, command targeting) tags the message with
                        // `MessageMeta::AppMention`. Honor that instead of
                        // re-running the weaker substring heuristic, which would
                        // otherwise drop e.g. a reply to the bot carrying no
                        // textual @-mention.
                        let mentioned = message_pre_validated_mention(&ctx.message)
                            || self.check_mention(&ctx.message.text, &channel_config);
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
                    return Err(RoutingError::PermissionDenied("DMs disabled".to_string()));
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
                    } else if self
                        .pairing_store
                        .is_approved(channel_id, &ctx.sender_normalized)
                        .await?
                    {
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
    ///
    /// A mention is a WHOLE TOKEN, never a substring: "the aleph roadmap"
    /// mentions nobody. The default pattern list used to hold the bare
    /// string `"aleph"`, so `require_mention` was satisfied by any group
    /// message containing those five letters — for every channel that never
    /// configured a `bot_name` the group gate was a no-op.
    ///
    /// What counts as a mention:
    /// - a configured `bot_name`, matched as a whole word (its `@` prefix is
    ///   optional). Naming the bot is opt-in per channel, so only an operator
    ///   who configured it gets the bare name as a trigger.
    /// - otherwise an explicitly addressed handle — `"@aleph"` / `"@bot"` —
    ///   which still needs the `@`.
    pub(super) fn check_mention(&self, text: &str, config: &ChannelConfig) -> bool {
        let tokens: Vec<&str> = text.split_whitespace().map(mention_token).collect();
        let cores: Vec<&str> = tokens
            .iter()
            .copied()
            .map(|t| t.strip_prefix('@').unwrap_or(t))
            .collect();

        // Check bot name — whole word, so "alphabet" is not "aleph".
        if let Some(bot_name) = &config.bot_name {
            if contains_word_sequence(&cores, bot_name) {
                return true;
            }
        }

        // Check common patterns — these apply to every channel, so they stay
        // explicitly addressed.
        tokens.iter().zip(&cores).any(|(token, core)| {
            token.starts_with('@')
                && (core.eq_ignore_ascii_case("aleph") || core.eq_ignore_ascii_case("bot"))
        })
    }

    /// Send pairing request to unknown sender
    ///
    /// Always sends the pairing code message, even if the request already exists
    /// (the initial delivery may have failed due to channel not being connected).
    pub(super) async fn send_pairing_request(
        &self,
        ctx: &InboundContext,
    ) -> Result<(), RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let sender_id = &ctx.sender_normalized;

        let mut metadata = HashMap::new();
        metadata.insert(
            "sender_display".to_string(),
            ctx.message.sender_id.as_str().to_string(),
        );

        let (code, created) = self
            .pairing_store
            .upsert(channel_id, sender_id, metadata)
            .await?;

        if created {
            // The pairing code IS the credential: anyone who can read the log
            // stream can approve themselves. It belongs in the outbound
            // message to the owner only — never in the log.
            info!(
                "Created new pairing request for {}:{} (code redacted)",
                channel_id, sender_id
            );
        } else {
            info!(
                "Resending existing pairing code for {}:{}",
                channel_id, sender_id
            );
        }

        // Always send the pairing message (not just on first create)
        // because the initial delivery may have failed.
        let message = format!(
            "Hi! I'm Aleph, a personal AI assistant.\n\n\
            To chat with me, please have my owner approve your access.\n\n\
            Your ID: {sender_id}\n\
            Pairing code: {code}\n\n\
            Once approved, just send me a message!"
        );

        let outbound = OutboundMessage::text(ctx.reply_route.conversation_id.as_str(), message);

        if let Err(e) = self
            .channel_registry
            .send(&ctx.reply_route.channel_id, outbound)
            .await
        {
            warn!(
                "Failed to send pairing message to {}:{}: {}",
                channel_id, sender_id, e
            );
        } else {
            info!(
                "Sent pairing code (redacted) to {}:{}",
                channel_id, sender_id
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod pre_validated_mention_tests {
    use super::message_pre_validated_mention;
    use crate::gateway::channel::{
        ChannelId, ConversationId, InboundMessage, MessageId, MessageMeta, UserId,
    };
    use chrono::Utc;

    fn group_msg(metadata: Vec<MessageMeta>) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("1"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("-100"),
            sender_id: UserId::new("42"),
            sender_name: None,
            text: "yes please".to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: true,
            raw: None,
            metadata,
        }
    }

    #[test]
    fn app_mention_is_pre_validated() {
        assert!(message_pre_validated_mention(&group_msg(vec![
            MessageMeta::AppMention
        ])));
    }

    #[test]
    fn absent_app_mention_is_not_pre_validated() {
        assert!(!message_pre_validated_mention(&group_msg(vec![])));
        assert!(!message_pre_validated_mention(&group_msg(vec![
            MessageMeta::BotAuthored
        ])));
    }
}
