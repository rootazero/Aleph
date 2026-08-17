use crate::gateway::channel::InboundMessage;
use crate::gateway::interfaces::feishu::config::FeishuConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupPolicyResult {
    Pass,
    Block(String),
}

pub struct GroupPolicyEngine {
    config: FeishuConfig,
}

impl GroupPolicyEngine {
    #[must_use]
    pub const fn new(config: FeishuConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn evaluate(&self, msg: &InboundMessage) -> GroupPolicyResult {
        if !msg.is_group {
            return GroupPolicyResult::Pass;
        }
        if !self.config.groups_allowed {
            return GroupPolicyResult::Block("Group messages are disabled".into());
        }
        if self.config.group_policy == "allowlist" {
            let chat = msg.conversation_id.as_str();
            if !self
                .config
                .group_allowlist
                .iter()
                .any(|g| g == chat || g == "*")
            {
                return GroupPolicyResult::Block("Group not in allowlist".into());
            }
        }
        GroupPolicyResult::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
    use crate::gateway::interfaces::feishu::config::{FeishuConfig, GroupSessionScope};

    fn make_group(chat: &str) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("m1"),
            channel_id: ChannelId::new("feishu"),
            conversation_id: ConversationId::new(chat),
            sender_id: UserId::new("ou_1"),
            sender_name: None,
            text: "hi".into(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: true,
            raw: None,
            metadata: vec![],
        }
    }

    fn feishu_config(
        groups_allowed: bool,
        group_policy: &str,
        allowlist: Vec<String>,
    ) -> FeishuConfig {
        FeishuConfig {
            app_id: "test".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed,
            group_policy: group_policy.into(),
            group_allowlist: allowlist,
            require_mention: true,
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
    fn test_group_disabled() {
        let engine = GroupPolicyEngine::new(feishu_config(false, "open", vec![]));
        assert!(matches!(
            engine.evaluate(&make_group("oc_1")),
            GroupPolicyResult::Block(_)
        ));
    }

    #[test]
    fn test_group_allowlist_pass() {
        let engine = GroupPolicyEngine::new(feishu_config(true, "allowlist", vec!["oc_1".into()]));
        assert_eq!(
            engine.evaluate(&make_group("oc_1")),
            GroupPolicyResult::Pass
        );
    }
}
