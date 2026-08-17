use crate::sync_primitives::Arc;

use chrono::Utc;

use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
use crate::gateway::interfaces::feishu::api::FeishuApi;
use crate::gateway::interfaces::feishu::config::{FeishuConfig, GroupSessionScope};
use crate::gateway::interfaces::feishu::feishu_inbound::user_cache::UserProfileCache;
use crate::gateway::interfaces::feishu::types::{ChatType, FeishuEvent, Mention};

pub fn map_event_to_inbound(
    event: &FeishuEvent,
    channel_id: &ChannelId,
    config: &FeishuConfig,
    bot_open_id: &str,
    _user_cache: &UserProfileCache,
    _api: &Arc<FeishuApi>,
) -> Option<InboundMessage> {
    match event {
        FeishuEvent::MessageReceive {
            message_id,
            chat_id,
            chat_type,
            sender_id,
            sender_name,
            message_type,
            content,
            mentions,
            parent_id,
            root_id,
        } => {
            let is_group = *chat_type == ChatType::Group;
            let conversation_id =
                build_conversation_id(chat_id, sender_id, root_id, config, is_group);
            // Mark which mentions point at the bot once, then reuse for gating + stripping.
            let mut marked = mentions.clone();
            crate::gateway::interfaces::feishu::feishu_inbound::events::mark_bot_mentions(
                &mut marked,
                bot_open_id,
            );
            let bot_mentioned = marked.iter().any(|m| m.is_bot);
            let text = extract_message_text(message_type, content, &marked);
            let mut metadata = vec![];
            if bot_mentioned {
                metadata.push(crate::gateway::channel::MessageMeta::AppMention);
            }
            Some(InboundMessage {
                id: MessageId::new(message_id),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(conversation_id),
                sender_id: UserId::new(sender_id),
                sender_name: sender_name.clone(),
                text,
                attachments: vec![],
                timestamp: Utc::now(),
                reply_to: parent_id.as_ref().map(MessageId::new),
                is_group,
                raw: Some(serde_json::json!(event)),
                metadata,
            })
        }
        FeishuEvent::CardAction {
            chat_id,
            sender_id,
            action_value,
            message_id,
        } => Some(InboundMessage {
            id: MessageId::new(message_id),
            channel_id: channel_id.clone(),
            conversation_id: ConversationId::new(chat_id.clone()),
            sender_id: UserId::new(sender_id),
            sender_name: None,
            text: action_value.clone(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: Some(MessageId::new(message_id)),
            is_group: false,
            raw: None,
            metadata: vec![],
        }),
        _ => None,
    }
}

fn build_conversation_id(
    chat_id: &str,
    sender_id: &str,
    root_id: &Option<String>,
    config: &FeishuConfig,
    is_group: bool,
) -> String {
    if !is_group {
        return chat_id.to_string();
    }
    match config.group_session_scope {
        GroupSessionScope::Group => chat_id.to_string(),
        GroupSessionScope::User => format!("{chat_id}:{sender_id}"),
        GroupSessionScope::Thread => root_id
            .as_ref()
            .map_or_else(|| chat_id.to_string(), |r| format!("{chat_id}:{r}")),
    }
}

/// Convert a Feishu message body into the plain text handed to the agent.
///
/// `content` is the raw Feishu content JSON (e.g. `{"text":"..."}`); `mentions`
/// must already be marked (see [`events::mark_bot_mentions`]) so bot @placeholders
/// are stripped from text messages.
fn extract_message_text(message_type: &str, content: &str, mentions: &[Mention]) -> String {
    use crate::gateway::interfaces::feishu::feishu_inbound::events;
    match message_type {
        "text" => events::extract_text_content(content, mentions).unwrap_or_default(),
        "post" => events::parse_post_content(content),
        "image" => "[Image]".to_string(),
        "file" => serde_json::from_str::<serde_json::Value>(content).map_or_else(
            |_| "[File]".to_string(),
            |v| v["file_name"].as_str().unwrap_or("[File]").to_string(),
        ),
        "audio" => "[Audio]".to_string(),
        "video" => "[Video]".to_string(),
        "sticker" => "[Sticker]".to_string(),
        "merge_forward" => serde_json::from_str::<serde_json::Value>(content).map_or_else(
            |_| "[Forwarded Messages]".to_string(),
            |v| {
                v["title"]
                    .as_str()
                    .unwrap_or("[Forwarded Messages]")
                    .to_string()
            },
        ),
        _ => format!("[{message_type} message]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::feishu::config::{FeishuConfig, GroupSessionScope};

    fn make_api() -> Arc<FeishuApi> {
        Arc::new(FeishuApi::new(
            crate::sync_primitives::Arc::new(
                crate::gateway::interfaces::feishu::auth::TokenManager::new(
                    "",
                    "",
                    "https://open.feishu.cn",
                    reqwest::Client::new(),
                ),
            ),
            "https://open.feishu.cn",
            reqwest::Client::new(),
        ))
    }

    fn feishu_config() -> FeishuConfig {
        FeishuConfig {
            app_id: "test".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: true,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: false,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: None,
        }
    }

    #[test]
    fn test_card_action_to_inbound_message() {
        let event = FeishuEvent::CardAction {
            chat_id: "oc_1".into(),
            sender_id: "ou_1".into(),
            action_value: "approve".into(),
            message_id: "om_1".into(),
        };
        let api = make_api();
        let cache = UserProfileCache::new();
        let msg = map_event_to_inbound(
            &event,
            &ChannelId::new("feishu"),
            &feishu_config(),
            "bot",
            &cache,
            &api,
        );
        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.text, "approve");
        assert_eq!(msg.conversation_id.as_str(), "oc_1");
    }

    #[test]
    fn test_text_message_extracts_plain_text() {
        // Regression: previously returned the raw JSON `{"text":"hello"}` verbatim.
        assert_eq!(
            extract_message_text("text", r#"{"text":"hello"}"#, &[]),
            "hello"
        );
    }

    #[test]
    fn test_text_message_strips_bot_mention() {
        let mentions = vec![Mention {
            key: "@_user_1".into(),
            id: "ou_bot".into(),
            name: "Aleph".into(),
            is_bot: true,
        }];
        assert_eq!(
            extract_message_text("text", r#"{"text":"@_user_1 ping"}"#, &mentions),
            "ping"
        );
    }

    #[test]
    fn test_post_message_parsed() {
        let content = r#"{"title":"T","content":[[{"tag":"text","text":"hello world"}]]}"#;
        assert_eq!(
            extract_message_text("post", content, &[]),
            "T\n\nhello world"
        );
    }

    #[test]
    fn test_post_message_fallback_on_garbage() {
        assert_eq!(
            extract_message_text("post", "{}", &[]),
            "[Rich text message]"
        );
    }

    #[test]
    fn test_file_message_placeholder() {
        assert_eq!(
            extract_message_text("file", r#"{"file_name":"report.pdf"}"#, &[]),
            "report.pdf"
        );
    }
}
