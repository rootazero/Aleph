//! Channel Access Control Policy
//!
//! Provides fine-grained access control for channels supporting DM and group
//! messaging, with configurable policies and allowlists.

use crate::gateway::channel::UserId;
use serde::{Deserialize, Serialize};

/// E.164 formatted phone number
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct E164Number(pub String);

impl E164Number {
    pub fn new(number: impl Into<String>) -> Self {
        Self(number.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Normalize a phone number to E.164 format
    pub fn normalize(raw: &str) -> Option<Self> {
        let cleaned: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        if cleaned.len() < 10 || cleaned.len() > 15 {
            return None;
        }
        Some(Self(format!("+{}", cleaned)))
    }
}

impl std::fmt::Display for E164Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// DM (direct message) access policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// First message requires pairing approval
    Pairing,
    /// Only explicitly allowlisted senders
    Allowlist,
    /// Anyone can send (requires `allow_from: ["*"]`)
    Open,
    /// No messages accepted
    Disabled,
}

impl Default for DmPolicy {
    fn default() -> Self {
        Self::Pairing
    }
}

/// Group access policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Anyone can send
    Open,
    /// Only allowlisted senders
    Allowlist,
    /// No group messages
    Disabled,
}

impl Default for GroupPolicy {
    fn default() -> Self {
        Self::Allowlist
    }
}

/// Policy evaluation result
#[derive(Debug, Clone)]
pub struct PolicyDecision {
    /// Whether the action is allowed
    pub allowed: bool,
    /// Human-readable reason (for denied decisions)
    pub reason: Option<String>,
}

impl PolicyDecision {
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            reason: None,
        }
    }

    pub fn denied(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: Some(reason.into()),
        }
    }
}

/// Channel access control configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelAccessConfig {
    /// DM access policy
    #[serde(default)]
    pub dm_policy: DmPolicy,

    /// Allowlisted phone numbers for DMs (E.164 format)
    #[serde(default)]
    pub allow_from: Vec<String>,

    /// Group access policy
    #[serde(default)]
    pub group_policy: GroupPolicy,

    /// Allowlisted phone numbers for groups (E.164 format)
    #[serde(default)]
    pub group_allow_from: Vec<String>,

    /// Explicitly allowlisted group JIDs
    #[serde(default)]
    pub groups: Vec<String>,
}

impl Default for ChannelAccessConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Pairing,
            allow_from: Vec::new(),
            group_policy: GroupPolicy::Allowlist,
            group_allow_from: Vec::new(),
            groups: Vec::new(),
        }
    }
}

/// Channel policy evaluation trait
///
/// Channels implement this to provide access control decisions
/// based on sender identity and message context.
pub trait ChannelPolicy: Send + Sync {
    /// Evaluate if a sender can send DMs to this channel
    fn evaluate_dm(&self, sender: &UserId) -> PolicyDecision;

    /// Evaluate if a sender can message in a group
    fn evaluate_group(&self, sender: &UserId, group_id: &str) -> PolicyDecision;
}

/// Standard WhatsApp policy implementation
#[derive(Debug, Clone)]
pub struct WhatsAppPolicy {
    config: ChannelAccessConfig,
}

impl WhatsAppPolicy {
    pub fn new(config: ChannelAccessConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &ChannelAccessConfig {
        &self.config
    }

    fn matches_allowlist(&self, sender: &UserId, allowlist: &[String]) -> bool {
        allowlist
            .iter()
            .any(|entry| entry == "*" || entry == sender.as_str() || entry.starts_with('+'))
    }

    fn is_in_group(&self, group_id: &str) -> bool {
        self.config.groups.is_empty() || self.config.groups.iter().any(|g| g == group_id)
    }
}

impl ChannelPolicy for WhatsAppPolicy {
    fn evaluate_dm(&self, sender: &UserId) -> PolicyDecision {
        match self.config.dm_policy {
            DmPolicy::Disabled => PolicyDecision::denied("Direct messages are disabled"),
            DmPolicy::Open => PolicyDecision::allowed(),
            DmPolicy::Pairing => {
                // Pairing is handled by the channel's pairing state
                // For now, allow all - pairing is checked elsewhere
                PolicyDecision::allowed()
            }
            DmPolicy::Allowlist => {
                if self.matches_allowlist(sender, &self.config.allow_from) {
                    PolicyDecision::allowed()
                } else {
                    PolicyDecision::denied("Your number is not in the allowlist")
                }
            }
        }
    }

    fn evaluate_group(&self, sender: &UserId, group_id: &str) -> PolicyDecision {
        // First check if this group is in our allowlist
        if !self.is_in_group(group_id) {
            return PolicyDecision::denied("Group is not in the allowlist");
        }

        match self.config.group_policy {
            GroupPolicy::Disabled => PolicyDecision::denied("Group messages are disabled"),
            GroupPolicy::Open => PolicyDecision::allowed(),
            GroupPolicy::Allowlist => {
                if self.matches_allowlist(sender, &self.config.group_allow_from) {
                    PolicyDecision::allowed()
                } else {
                    PolicyDecision::denied("Your number is not in the group allowlist")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy(dm: DmPolicy, group: GroupPolicy) -> WhatsAppPolicy {
        WhatsAppPolicy::new(ChannelAccessConfig {
            dm_policy: dm,
            allow_from: vec!["+15551234567".to_string()],
            group_policy: group,
            group_allow_from: vec!["+15551234567".to_string()],
            groups: vec!["group@g.us".to_string()],
        })
    }

    #[test]
    fn test_dm_policy_disabled() {
        let policy = make_policy(DmPolicy::Disabled, GroupPolicy::Open);
        let sender = UserId::new("+15551234567");
        let decision = policy.evaluate_dm(&sender);
        assert!(!decision.allowed);
        assert!(decision.reason.unwrap().contains("disabled"));
    }

    #[test]
    fn test_dm_policy_open() {
        let policy = make_policy(DmPolicy::Open, GroupPolicy::Allowlist);
        let sender = UserId::new("+15559999999");
        let decision = policy.evaluate_dm(&sender);
        assert!(decision.allowed);
    }

    #[test]
    fn test_dm_policy_allowlist() {
        let policy = make_policy(DmPolicy::Allowlist, GroupPolicy::Allowlist);

        let allowed = UserId::new("+15551234567");
        assert!(policy.evaluate_dm(&allowed).allowed);

        let denied = UserId::new("+15559999999");
        let decision = policy.evaluate_dm(&denied);
        assert!(!decision.allowed);
    }

    #[test]
    fn test_group_policy_disabled() {
        let policy = make_policy(DmPolicy::Open, GroupPolicy::Disabled);
        let sender = UserId::new("+15551234567");
        let decision = policy.evaluate_group(&sender, "group@g.us");
        assert!(!decision.allowed);
    }

    #[test]
    fn test_group_not_in_allowlist() {
        let policy = make_policy(DmPolicy::Open, GroupPolicy::Open);
        let sender = UserId::new("+15551234567");
        let decision = policy.evaluate_group(&sender, "unknown@g.us");
        assert!(!decision.allowed);
    }

    #[test]
    fn test_e164_normalize() {
        assert_eq!(
            E164Number::normalize("+15551234567").unwrap().as_str(),
            "+15551234567"
        );
        assert_eq!(
            E164Number::normalize("15551234567").unwrap().as_str(),
            "+15551234567"
        );
        assert_eq!(
            E164Number::normalize("(555) 123-4567").unwrap().as_str(),
            "+15551234567"
        );
        assert!(E164Number::normalize("123").is_none());
    }
}
