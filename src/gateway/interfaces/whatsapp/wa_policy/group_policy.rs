use crate::gateway::channel::InboundMessage;
use crate::gateway::channel_policy::GroupPolicy;
use crate::gateway::interfaces::whatsapp::config::AccessConfig;

pub struct GroupPolicyEngine {
    access: AccessConfig,
}

impl GroupPolicyEngine {
    pub fn new(access: AccessConfig) -> Self {
        Self { access }
    }

    pub fn evaluate(&self, msg: &InboundMessage) -> GroupPolicyResult {
        if !msg.is_group {
            return GroupPolicyResult::Pass;
        }

        let chat = msg.conversation_id.as_str();

        if !self.access.groups.is_empty()
            && !self.access.groups.iter().any(|g| g == chat || g == "*")
        {
            return GroupPolicyResult::Block("Group not in allowlist".into());
        }

        match self.access.group_policy {
            GroupPolicy::Disabled => GroupPolicyResult::Block("Group messages disabled".into()),
            GroupPolicy::Open => GroupPolicyResult::Pass,
            GroupPolicy::Allowlist => {
                let sender = msg.sender_id.as_str();
                let group_allow: Vec<&str> = if self.access.group_allow_from.is_empty() {
                    self.access.allow_from.iter().map(String::as_str).collect()
                } else {
                    self.access
                        .group_allow_from
                        .iter()
                        .map(String::as_str)
                        .collect()
                };
                if group_allow.contains(&"*") || group_allow.contains(&sender) {
                    GroupPolicyResult::Pass
                } else {
                    GroupPolicyResult::Block("Sender not in group allowlist".into())
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupPolicyResult {
    Pass,
    Block(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};

    fn make_group_msg(chat: &str, sender: &str) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("m1"),
            channel_id: ChannelId::new("wa"),
            conversation_id: ConversationId::new(chat),
            sender_id: UserId::new(sender),
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

    #[test]
    fn test_group_allowlist() {
        let access = AccessConfig {
            group_policy: GroupPolicy::Allowlist,
            allow_from: vec!["+1555".into()],
            ..Default::default()
        };
        let engine = GroupPolicyEngine::new(access);
        assert_eq!(
            engine.evaluate(&make_group_msg("g@g.us", "+1555")),
            GroupPolicyResult::Pass
        );
        assert!(matches!(
            engine.evaluate(&make_group_msg("g@g.us", "+1999")),
            GroupPolicyResult::Block(_)
        ));
    }
}
