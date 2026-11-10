use crate::gateway::channel::InboundMessage;
use crate::gateway::channel_policy::DmPolicy;
use crate::gateway::interfaces::whatsapp::config::AccessConfig;

pub struct DmPolicyEngine {
    access: AccessConfig,
    paired_numbers: Vec<String>,
}

impl DmPolicyEngine {
    #[must_use]
    pub const fn new(access: AccessConfig, paired_numbers: Vec<String>) -> Self {
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
                // Normalize the sender JID to a phone number for comparison with
                // E.164 allowlist entries (e.g. "15551234567@s.whatsapp.net" -> "15551234567").
                // The local part of a user JID is always the bare phone number; group JIDs
                // use a different suffix and are excluded above by the is_group guard.
                let sender_phone = sender.split('@').next().unwrap_or(sender);
                let allowed: Vec<&str> =
                    self.access.allow_from.iter().map(String::as_str).collect();
                // Strip a leading '+' from allowlist entries so that E.164 values like
                // "+15551234567" match the JID local part "15551234567".
                let matches_allowlist = allowed.iter().any(|entry| {
                    let e = entry.strip_prefix('+').unwrap_or(entry);
                    *entry == "*" || e == sender_phone || *entry == sender
                });
                let matches_paired = self.paired_numbers.iter().any(|n| {
                    let n_phone = n.strip_prefix('+').unwrap_or(n.as_str());
                    n_phone == sender_phone || n == sender
                });
                if matches_allowlist || matches_paired {
                    DmPolicyResult::Pass
                } else {
                    DmPolicyResult::Block("Sender not in allowlist".into())
                }
            }
            DmPolicy::Pairing => {
                let sender = msg.sender_id.as_str();
                let sender_phone = sender.split('@').next().unwrap_or(sender);
                let allowed: Vec<&str> =
                    self.access.allow_from.iter().map(String::as_str).collect();
                let matches_allowlist = allowed.iter().any(|entry| {
                    let e = entry.strip_prefix('+').unwrap_or(entry);
                    e == sender_phone || *entry == sender
                });
                let matches_paired = self.paired_numbers.iter().any(|n| {
                    let n_phone = n.strip_prefix('+').unwrap_or(n.as_str());
                    n_phone == sender_phone || n == sender
                });
                if matches_allowlist || matches_paired {
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
