use super::SlackMessageOps;
    use crate::gateway::channel::{ChannelId, MessageMeta};
    use crate::gateway::interfaces::slack::SlackConfig;

    #[test]
    fn test_convert_basic_message() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello agent!",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        assert_eq!(msg.channel_id.as_str(), "slack");
        assert_eq!(msg.conversation_id.as_str(), "C789");
        assert_eq!(msg.sender_id.as_str(), "U456");
        assert_eq!(msg.text, "Hello agent!");
        assert!(msg.is_group);
    }

    #[test]
    fn test_convert_filters_bot_messages() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Bot message",
            "ts": "1700000000.000100",
            "bot_id": "B999"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_filters_own_user() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "My message",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "U456", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_channel_filter() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");

        // Not in allowed channels
        let config = SlackConfig {
            allowed_channels: vec!["C111".to_string(), "C222".to_string()],
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());

        // In allowed channels
        let config = SlackConfig {
            allowed_channels: vec!["C789".to_string()],
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
    }

    #[test]
    fn test_convert_skips_other_subtypes() {
        let event = serde_json::json!({
            "type": "message",
            "subtype": "channel_join",
            "user": "U456",
            "channel": "C789",
            "text": "joined",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_message_changed() {
        let event = serde_json::json!({
            "type": "message",
            "subtype": "message_changed",
            "channel": "C789",
            "message": {
                "user": "U456",
                "text": "Edited message text",
                "ts": "1700000000.000100"
            },
            "ts": "1700000001.000200"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        assert_eq!(msg.conversation_id.as_str(), "C789");
        assert_eq!(msg.text, "Edited message text");
    }

    #[test]
    fn test_convert_non_message_event() {
        let event = serde_json::json!({
            "type": "reaction_added",
            "user": "U456",
            "reaction": "thumbsup"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_empty_text() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_dm_message() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "D12345",
            "text": "Private message",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");

        // DMs allowed
        let config = SlackConfig {
            dm_allowed: true,
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
        assert!(!msg.unwrap().is_group);

        // DMs not allowed
        let config = SlackConfig {
            dm_allowed: false,
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_thread_reply() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Thread reply",
            "ts": "1700000002.000300",
            "thread_ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "1700000000.000100");
    }

    #[test]
    fn test_convert_normalizes_mrkdwn() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "*bold text*",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        // Slack *bold* normalizes to Markdown **bold**
        assert_eq!(msg.text, "**bold text**");
    }

    #[test]
    fn test_convert_timestamp_parsing() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config)
            .unwrap();

        assert_eq!(msg.timestamp.timestamp(), 1700000000);
    }

    #[test]
    fn test_normalize_emoji_to_slack() {
        fn normalize(emoji: &str) -> String {
            SlackMessageOps::normalize_emoji_to_slack(emoji)
        }

        assert_eq!(normalize("👍"), "thumbsup");
        assert_eq!(normalize("👎"), "thumbsdown");
        assert_eq!(normalize("❤️"), "heart");
        assert_eq!(normalize("😂"), "joy");
        assert_eq!(normalize("😢"), "cry");
        assert_eq!(normalize("😮"), "astonished");
        assert_eq!(normalize("😒"), "disappointed");
        assert_eq!(normalize("😴"), "sleepy");
        assert_eq!(normalize("🤔"), "thinking");
        assert_eq!(normalize("👏"), "clap");
        assert_eq!(normalize("🙄"), "eyes_on_eyes");
        assert_eq!(normalize("😎"), "sunglasses");
        assert_eq!(normalize("🤷"), "shrug");
        assert_eq!(normalize("✅"), "white_check_mark");
        assert_eq!(normalize("❌"), "x");
        assert_eq!(normalize("🔥"), "fire");
        assert_eq!(normalize("💯"), "100");
        assert_eq!(normalize("✨"), "sparkles");
        assert_eq!(normalize("🎉"), "tada");
        assert_eq!(normalize("🚀"), "rocket");
        assert_eq!(normalize("💪"), "muscle");
        assert_eq!(normalize("🙏"), "pray");
        assert_eq!(normalize("😊"), "blush");
        assert_eq!(normalize("🤷‍♀️"), "shrug");
        assert_eq!(normalize("🤷‍♂️"), "shrug");
        assert_eq!(normalize("👀"), "eyes");
        assert_eq!(normalize("🙌"), "raised_hands");
        assert_eq!(normalize("👍🏻"), "thumbsup");
        assert_eq!(normalize("👍🏿"), "thumbsup");
        assert_eq!(normalize(":thumbsup:"), "thumbsup");
        assert_eq!(normalize("unknown_emoji_xyz"), "unknownemojixyz");
        assert_eq!(normalize("💫"), "dizzy");
    }

    #[test]
    fn test_convert_app_mention_basic() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U456",
            "channel": "C789",
            "text": "<@B123> Hello bot!",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config)
                .unwrap();

        assert_eq!(msg.channel_id.as_str(), "slack");
        assert_eq!(msg.conversation_id.as_str(), "C789");
        assert_eq!(msg.sender_id.as_str(), "U456");
        assert!(msg.text.contains("Hello bot"));
        assert!(msg.is_group);
        assert!(msg
            .metadata
            .iter()
            .any(|m| matches!(m, MessageMeta::AppMention)));
    }

    #[test]
    fn test_convert_app_mention_filters_own_message() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "B123",
            "channel": "C789",
            "text": "<@B123> self mention",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_app_mention_channel_filter() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U456",
            "channel": "C789",
            "text": "<@B123> Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");

        let config = SlackConfig {
            allowed_channels: vec!["C111".to_string()],
            ..Default::default()
        };
        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_filters_user_not_in_allowlist() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };

        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_allows_user_in_allowlist() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U123",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };

        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
    }

    #[test]
    fn test_convert_allowlist_empty_allows_all() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();

        let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
    }

    #[test]
    fn test_convert_mention_filters_user_not_in_allowlist() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U456",
            "channel": "C789",
            "text": "<@B123> Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };

        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_mention_allows_user_in_allowlist() {
        let event = serde_json::json!({
            "type": "app_mention",
            "user": "U123",
            "channel": "C789",
            "text": "<@B123> Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig {
            user_allowlist: vec!["U123".to_string()],
            ..Default::default()
        };

        let msg =
            SlackMessageOps::convert_app_mention_to_inbound(&event, &channel_id, "B123", &config);
        assert!(msg.is_some());
    }
