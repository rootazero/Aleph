use crate::sync_primitives::Arc;

use chrono::Utc;

use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
use crate::gateway::interfaces::feishu::api::FeishuApi;
use crate::gateway::interfaces::feishu::config::{FeishuConfig, GroupSessionScope};
use crate::gateway::interfaces::feishu::feishu_inbound::user_cache::UserProfileCache;
use crate::gateway::interfaces::feishu::types::{ChatType, FeishuEvent, Mention};

/// Map one routed Feishu event to an `InboundMessage`, or `None` when the event
/// carries nothing the agent can answer.
///
/// `sender_name` is resolved through [`UserProfileCache`], which fills the gap
/// Feishu leaves in message events (they carry the sender's `open_id` but no
/// display name) and keeps the lookup cached across messages.
pub async fn map_event_to_inbound(
    event: &FeishuEvent,
    channel_id: &ChannelId,
    config: &FeishuConfig,
    bot_open_id: &str,
    user_cache: &UserProfileCache,
    api: &Arc<FeishuApi>,
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
                sender_name: match sender_name {
                    Some(name) => Some(name.clone()),
                    None => user_cache.get_name(sender_id, api).await,
                },
                text,
                attachments: vec![],
                timestamp: Utc::now(),
                reply_to: parent_id.as_ref().map(MessageId::new),
                is_group,
                raw: Some(serde_json::json!(event)),
                metadata,
            })
        }
        FeishuEvent::BotMenu {
            event_key,
            operator_id,
            operator_name,
            timestamp,
        } => {
            // A bot-menu click is the operator interacting with the bot in their
            // own chat with it, so the DM classification is authoritative here —
            // unlike card actions, which carry no chat type at all.
            Some(InboundMessage {
                id: MessageId::new(format!("menu:{operator_id}:{timestamp}")),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(format!("dm:{operator_id}")),
                sender_id: UserId::new(operator_id),
                sender_name: match operator_name {
                    Some(name) => Some(name.clone()),
                    None => user_cache.get_name(operator_id, api).await,
                },
                text: format!("[Menu] {event_key}"),
                attachments: vec![],
                timestamp: Utc::now(),
                reply_to: None,
                is_group: false,
                raw: Some(serde_json::json!(event)),
                metadata: vec![],
            })
        }
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

    #[tokio::test]
    async fn test_bot_menu_click_becomes_a_direct_message() {
        let event = FeishuEvent::BotMenu {
            event_key: "menu_settings".into(),
            operator_id: "ou_1".into(),
            operator_name: None,
            timestamp: 1_700_000_000_000,
        };
        let api = make_api();
        // Pre-seed the cache so the mapping never reaches the network.
        let cache = UserProfileCache::new();
        cache.insert(
            crate::gateway::interfaces::feishu::feishu_inbound::user_cache::UserProfile {
                open_id: "ou_1".into(),
                name: Some("Operator".into()),
            },
        );
        let msg = map_event_to_inbound(
            &event,
            &ChannelId::new("feishu"),
            &feishu_config(),
            "bot",
            &cache,
            &api,
        )
        .await
        .expect("a menu click is routable");
        // A menu click happens in the operator's own chat with the bot, so it is
        // a DM and goes through the DM policy engine.
        assert!(!msg.is_group);
        assert_eq!(msg.conversation_id.as_str(), "dm:ou_1");
        assert_eq!(msg.sender_id.as_str(), "ou_1");
        assert_eq!(msg.sender_name.as_deref(), Some("Operator"));
        assert_eq!(msg.text, "[Menu] menu_settings");
    }

    #[tokio::test]
    async fn test_sender_name_comes_from_the_profile_cache() {
        let sender = crate::gateway::interfaces::feishu::feishu_inbound::user_cache::UserProfile {
            open_id: "ou_1".into(),
            name: Some("Alice".into()),
        };
        let cache = UserProfileCache::new();
        cache.insert(sender);
        let event = FeishuEvent::MessageReceive {
            message_id: "om_1".into(),
            chat_id: "oc_1".into(),
            chat_type: ChatType::P2p,
            sender_id: "ou_1".into(),
            // Feishu message events never carry a display name.
            sender_name: None,
            message_type: "text".into(),
            content: r#"{"text":"hi"}"#.into(),
            mentions: vec![],
            parent_id: None,
            root_id: None,
        };
        let msg = map_event_to_inbound(
            &event,
            &ChannelId::new("feishu"),
            &feishu_config(),
            "bot",
            &cache,
            &make_api(),
        )
        .await
        .expect("text messages are routable");
        assert_eq!(msg.sender_name.as_deref(), Some("Alice"));
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
