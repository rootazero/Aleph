use crate::gateway::channel::InboundMessage;
use crate::gateway::channel_policy::DmPolicy;
use crate::gateway::interfaces::whatsapp::config::AccessConfig;

pub struct DmPolicyEngine {
    access: AccessConfig,
    paired_numbers: Vec<String>,
}

impl DmPolicyEngine {
    pub fn new(access: AccessConfig, paired_numbers: Vec<String>) -> Self {
        Self {
            access,
            paired_numbers,
        }
    }

    pub fn evaluate(&self, msg: &InboundMessage) -> DmPolicyResult {
        if msg.is_group {
            return DmPolicyResult::Pass;
        }

        match self.access.dm_policy {
            DmPolicy::Disabled => DmPolicyResult::Block("DMs disabled".into()),
            DmPolicy::Open => DmPolicyResult::Pass,
            DmPolicy::Allowlist => {
                let sender = msg.sender_id.as_str();
                let allowed: Vec<&str> =
                    self.access.allow_from.iter().map(String::as_str).collect();
                if allowed.contains(&"*")
                    || allowed.contains(&sender)
                    || self.paired_numbers.iter().any(|n| n == sender)
                {
                    DmPolicyResult::Pass
                } else {
                    DmPolicyResult::Block("Sender not in allowlist".into())
                }
            }
            DmPolicy::Pairing => {
                let sender = msg.sender_id.as_str();
                let allowed: Vec<&str> =
                    self.access.allow_from.iter().map(String::as_str).collect();
                if allowed.contains(&sender) || self.paired_numbers.iter().any(|n| n == sender) {
                    DmPolicyResult::Pass
                } else {
                    DmPolicyResult::NeedsPairing(sender.to_string())
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmPolicyResult {
    Pass,
    Block(String),
    NeedsPairing(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};

    fn make_dm(sender: &str) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("m1"),
            channel_id: ChannelId::new("wa"),
            conversation_id: ConversationId::new(sender),
            sender_id: UserId::new(sender),
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

    #[test]
    fn test_allowlist_blocks_unknown() {
        let access = AccessConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["+1555".into()],
            ..Default::default()
        };
        let engine = DmPolicyEngine::new(access, vec![]);
        let msg = make_dm("+1999");
        assert!(matches!(engine.evaluate(&msg), DmPolicyResult::Block(_)));
    }

    #[test]
    fn test_allowlist_allows_star() {
        let access = AccessConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["*".into()],
            ..Default::default()
        };
        let engine = DmPolicyEngine::new(access, vec![]);
        let msg = make_dm("+1999");
        assert_eq!(engine.evaluate(&msg), DmPolicyResult::Pass);
    }
}
