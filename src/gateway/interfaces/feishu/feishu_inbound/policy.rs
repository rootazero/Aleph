use crate::gateway::channel::InboundMessage;
use crate::gateway::interfaces::feishu::config::FeishuConfig;
use crate::gateway::interfaces::feishu::feishu_policy::{
    DmPolicyEngine, DmPolicyResult, GroupPolicyEngine, GroupPolicyResult,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundPolicyResult {
    Accept,
    Block(String),
}

pub struct InboundPolicy {
    dm: DmPolicyEngine,
    group: GroupPolicyEngine,
    require_mention: bool,
}

impl InboundPolicy {
    #[must_use]
    pub fn new(config: FeishuConfig, _bot_open_id: String) -> Self {
        Self {
            dm: DmPolicyEngine::new(config.clone()),
            group: GroupPolicyEngine::new(config.clone()),
            require_mention: config.require_mention,
        }
    }

    #[must_use]
    pub fn evaluate(&self, msg: &InboundMessage) -> InboundPolicyResult {
        if let GroupPolicyResult::Block(reason) = self.group.evaluate(msg) {
            return InboundPolicyResult::Block(reason);
        }
        if let DmPolicyResult::Block(reason) = self.dm.evaluate(msg) {
            return InboundPolicyResult::Block(reason);
        }
        if self.require_mention && msg.is_group {
            let mentioned = msg
                .metadata
                .iter()
                .any(|m| matches!(m, crate::gateway::channel::MessageMeta::AppMention));
            if !mentioned {
                return InboundPolicyResult::Block("Bot not mentioned".into());
            }
        }
        InboundPolicyResult::Accept
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
    use crate::gateway::interfaces::feishu::config::{FeishuConfig, GroupSessionScope};

    fn make_msg(is_group: bool) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("m1"),
            channel_id: ChannelId::new("feishu"),
            conversation_id: ConversationId::new("oc_1"),
            sender_id: UserId::new("ou_1"),
            sender_name: None,
            text: "hi".into(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group,
            raw: None,
            metadata: vec![],
        }
    }

    fn feishu_config(
        dm_allowed: bool,
        groups_allowed: bool,
        require_mention: bool,
    ) -> FeishuConfig {
        FeishuConfig {
            app_id: "test".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed,
            groups_allowed,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention,
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
    fn test_dm_blocked() {
        let policy = InboundPolicy::new(feishu_config(false, true, false), "bot".into());
        assert!(matches!(
            policy.evaluate(&make_msg(false)),
            InboundPolicyResult::Block(_)
        ));
    }

    #[test]
    fn test_group_allowed() {
        let policy = InboundPolicy::new(feishu_config(true, true, false), "bot".into());
        assert_eq!(
            policy.evaluate(&make_msg(true)),
            InboundPolicyResult::Accept
        );
    }

    #[test]
    fn test_mention_required() {
        let policy = InboundPolicy::new(feishu_config(true, true, true), "bot".into());
        assert!(matches!(
            policy.evaluate(&make_msg(true)),
            InboundPolicyResult::Block(_)
        ));
    }
}
