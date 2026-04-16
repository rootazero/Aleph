use chrono::Utc;
use teloxide::types::{MessageReactionUpdated, ReactionType};

use crate::gateway::channel::{
    ChannelId, ConversationId, InboundMessage, MessageId, MessageMeta, UserId,
};

pub fn convert_reaction(
    update: &MessageReactionUpdated,
    channel_id: &str,
) -> Option<InboundMessage> {
    let emojis: Vec<String> = update
        .new_reaction
        .iter()
        .filter_map(|r| match r {
            ReactionType::Emoji { emoji } => Some(emoji.clone()),
            _ => None,
        })
        .collect();

    if emojis.is_empty() {
        return None;
    }

    let user = update.user.as_ref()?;

    Some(InboundMessage {
        id: MessageId::new(format!(
            "react_{}_{}",
            update.chat.id.0, update.message_id.0
        )),
        channel_id: ChannelId::new(channel_id),
        conversation_id: ConversationId::new(update.chat.id.0.to_string()),
        sender_id: UserId::new(user.id.to_string()),
        sender_name: user
            .username
            .clone()
            .or_else(|| Some(user.first_name.clone())),
        text: format!("Reacted with: {}", emojis.join(", ")),
        attachments: Vec::new(),
        timestamp: Utc::now(),
        reply_to: Some(MessageId::new(update.message_id.0.to_string())),
        is_group: update.chat.id.0 < 0,
        raw: None,
        metadata: vec![MessageMeta::Reaction { emojis }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::MessageId;
    use teloxide::types::{
        Chat, ChatId, ChatKind, ChatPrivate, MessageId as TgMessageId, User, UserId as TgUserId,
    };

    fn make_test_update(emojis: Vec<String>) -> MessageReactionUpdated {
        MessageReactionUpdated {
            chat: Chat {
                id: ChatId(-100123456789),
                kind: ChatKind::Private(ChatPrivate {
                    username: None,
                    first_name: Some("Test".to_string()),
                    last_name: None,
                    bio: None,
                    has_private_forwards: None,
                    has_restricted_voice_and_video_messages: None,
                }),
                photo: None,
                pinned_message: None,
                message_auto_delete_time: None,
                has_hidden_members: false,
                has_aggressive_anti_spam_enabled: false,
                available_reactions: None,
                chat_full_info: teloxide::types::ChatFullInfo::default(),
            },
            message_id: TgMessageId(42),
            date: chrono::Utc::now(),
            actor_chat: None,
            user: Some(User {
                id: TgUserId(12345),
                is_bot: false,
                first_name: "Alice".to_string(),
                last_name: None,
                username: Some("alice".to_string()),
                language_code: None,
                is_premium: false,
                added_to_attachment_menu: false,
            }),
            old_reaction: vec![],
            new_reaction: emojis
                .into_iter()
                .map(|e| ReactionType::Emoji { emoji: e })
                .collect(),
        }
    }

    #[test]
    fn test_convert_reaction_basic() {
        let update = make_test_update(vec!["👍".to_string()]);
        let inbound = convert_reaction(&update, "telegram").unwrap();
        assert_eq!(inbound.conversation_id.as_str(), "-100123456789");
        assert_eq!(inbound.sender_id.as_str(), "12345");
        assert_eq!(inbound.text, "Reacted with: 👍");
        assert_eq!(inbound.reply_to, Some(MessageId::new("42".to_string())));
        assert!(inbound.is_group);
        match &inbound.metadata[0] {
            MessageMeta::Reaction { emojis } => {
                assert_eq!(emojis, &vec!["👍".to_string()]);
            }
            _ => panic!("Expected Reaction metadata"),
        }
    }

    #[test]
    fn test_convert_reaction_multiple() {
        let update = make_test_update(vec!["👍".to_string(), "❤️".to_string()]);
        let inbound = convert_reaction(&update, "telegram").unwrap();
        assert_eq!(inbound.text, "Reacted with: 👍, ❤️");
    }

    #[test]
    fn test_convert_reaction_empty_returns_none() {
        let update = make_test_update(vec![]);
        assert!(convert_reaction(&update, "telegram").is_none());
    }
}
