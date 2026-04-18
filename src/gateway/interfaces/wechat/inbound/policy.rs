//! DM/Group Access Policy
//!
//! Enforces DM and group access policies based on configuration.

use crate::gateway::interfaces::wechat::config::WeChatConfig;
use crate::gateway::interfaces::wechat::types::Message;

/// Check if a message should be accepted based on policy.
pub fn should_accept_message(msg: &Message, config: &WeChatConfig) -> bool {
    let room_id = msg.room_id.as_deref().or(msg.chat_room_id.as_deref());
    let from_user_id = &msg.from_user_id;

    if room_id.is_some() {
        let group_id = room_id.unwrap();
        if group_id.is_empty() {
            return config.is_user_allowed(from_user_id);
        }
        config.is_group_allowed(group_id)
    } else {
        config.is_user_allowed(from_user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_accept_dm_open_policy() {
        let config = WeChatConfig {
            dm_policy: crate::gateway::interfaces::wechat::config::DmPolicy::Open,
            ..Default::default()
        };
        let msg = Message {
            msg_id: "1".to_string(),
            from_user_id: "wxid_abc".to_string(),
            to_user_id: "bot".to_string(),
            room_id: None,
            chat_room_id: None,
            msg_type: 1,
            item_list: vec![],
            context_token: None,
        };
        assert!(should_accept_message(&msg, &config));
    }

    #[test]
    fn test_should_accept_dm_disabled() {
        let config = WeChatConfig {
            dm_policy: crate::gateway::interfaces::wechat::config::DmPolicy::Disabled,
            ..Default::default()
        };
        let msg = Message {
            msg_id: "1".to_string(),
            from_user_id: "wxid_abc".to_string(),
            to_user_id: "bot".to_string(),
            room_id: None,
            chat_room_id: None,
            msg_type: 1,
            item_list: vec![],
            context_token: None,
        };
        assert!(!should_accept_message(&msg, &config));
    }

    #[test]
    fn test_should_accept_group_disabled() {
        let config = WeChatConfig {
            group_policy: GroupPolicy::Disabled,
            ..Default::default()
        };
        let msg = Message {
            msg_id: "1".to_string(),
            from_user_id: "wxid_abc".to_string(),
            to_user_id: "bot".to_string(),
            room_id: Some("group123".to_string()),
            chat_room_id: None,
            msg_type: 1,
            item_list: vec![],
            context_token: None,
        };
        assert!(!should_accept_message(&msg, &config));
    }

    #[test]
    fn test_should_accept_group_open() {
        let config = WeChatConfig {
            group_policy: GroupPolicy::Open,
            ..Default::default()
        };
        let msg = Message {
            msg_id: "1".to_string(),
            from_user_id: "wxid_abc".to_string(),
            to_user_id: "bot".to_string(),
            room_id: Some("group123".to_string()),
            chat_room_id: None,
            msg_type: 1,
            item_list: vec![],
            context_token: None,
        };
        assert!(should_accept_message(&msg, &config));
    }
}
