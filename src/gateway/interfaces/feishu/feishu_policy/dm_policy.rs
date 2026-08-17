use crate::gateway::channel::InboundMessage;
use crate::gateway::interfaces::feishu::config::FeishuConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmPolicyResult {
    Pass,
    Block(String),
}

pub struct DmPolicyEngine {
    config: FeishuConfig,
}

impl DmPolicyEngine {
    #[must_use]
    pub const fn new(config: FeishuConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn evaluate(&self, msg: &InboundMessage) -> DmPolicyResult {
        if msg.is_group {
            return DmPolicyResult::Pass;
        }
        if !self.config.dm_allowed {
            return DmPolicyResult::Block("DMs are disabled".into());
        }
        DmPolicyResult::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
    use crate::gateway::interfaces::feishu::config::{FeishuConfig, GroupSessionScope};

    fn make_dm() -> InboundMessage {
        InboundMessage {
            id: MessageId::new("m1"),
            channel_id: ChannelId::new("feishu"),
            conversation_id: ConversationId::new("ou_1"),
            sender_id: UserId::new("ou_1"),
            sender_name: None,
            text: "hi".into(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        }
    }

    fn feishu_config(dm_allowed: bool) -> FeishuConfig {
        FeishuConfig {
            app_id: "test".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
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
    fn test_dm_disabled() {
        let engine = DmPolicyEngine::new(feishu_config(false));
        assert!(matches!(
            engine.evaluate(&make_dm()),
            DmPolicyResult::Block(_)
        ));
    }

    #[test]
    fn test_dm_allowed() {
        let engine = DmPolicyEngine::new(feishu_config(true));
        assert_eq!(engine.evaluate(&make_dm()), DmPolicyResult::Pass);
    }
}
