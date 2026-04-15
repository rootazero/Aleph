use crate::gateway::channel::InboundMessage;
use crate::gateway::interfaces::whatsapp::config::AccessConfig;
use crate::gateway::interfaces::whatsapp::wa_policy::{
    DmPolicyEngine, DmPolicyResult, GroupPolicyEngine, GroupPolicyResult, PairingTracker,
};

pub enum InboundPolicyResult {
    Accept,
    Block(String),
    NeedsPairing(String),
}

pub struct InboundPolicy {
    dm: DmPolicyEngine,
    group: GroupPolicyEngine,
    tracker: PairingTracker,
}

impl InboundPolicy {
    pub fn new(access: AccessConfig, paired_numbers: Vec<String>) -> Self {
        Self {
            dm: DmPolicyEngine::new(access.clone(), paired_numbers),
            group: GroupPolicyEngine::new(access),
            tracker: PairingTracker::new(),
        }
    }

    pub fn evaluate(&self, msg: &InboundMessage) -> InboundPolicyResult {
        match self.group.evaluate(msg) {
            GroupPolicyResult::Block(reason) => return InboundPolicyResult::Block(reason),
            _ => {}
        }

        match self.dm.evaluate(msg) {
            DmPolicyResult::Pass => InboundPolicyResult::Accept,
            DmPolicyResult::Block(reason) => InboundPolicyResult::Block(reason),
            DmPolicyResult::NeedsPairing(sender) => InboundPolicyResult::NeedsPairing(sender),
        }
    }

    pub fn approve_pairing(&self, sender_id: &str) -> bool {
        self.tracker.approve(sender_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
    use crate::gateway::channel_policy::{DmPolicy, GroupPolicy};

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
    fn test_dm_allowlist_blocks_unknown() {
        let access = AccessConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["+1555".into()],
            ..Default::default()
        };
        let policy = InboundPolicy::new(access, vec![]);
        let msg = make_dm("+1999");
        assert!(matches!(
            policy.evaluate(&msg),
            InboundPolicyResult::Block(_)
        ));
    }

    #[test]
    fn test_dm_allowlist_allows_star() {
        let access = AccessConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["*".into()],
            ..Default::default()
        };
        let policy = InboundPolicy::new(access, vec![]);
        let msg = make_dm("+1999");
        assert!(matches!(policy.evaluate(&msg), InboundPolicyResult::Accept));
    }
}
