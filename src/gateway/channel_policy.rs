//! Channel Access Control Policy
//!
//! The channel-level rungs of the permission hierarchy: the tier clamp for
//! untrusted channels ([`clamp_tier_for_channel`]), the access-policy config
//! types ([`ChannelAccessConfig`] / [`DmPolicy`] / [`GroupPolicy`]) consumed by
//! channel adapters (WhatsApp evaluates them in `wa_policy/`; the inbound
//! router has its own `ChannelConfig` twin), and [`E164Number`] normalization.

use crate::config::types::policies::ExecTier;
use crate::gateway::inbound_router::ChannelPermissionLevel;
use crate::gateway::pair_loop_guard::PairLoopGuardConfig;
use serde::{Deserialize, Serialize};

/// Highest execution tier an untrusted (`Chat`) channel may run at.
///
/// A remote conversational surface (Telegram / Slack / a paired phone) must
/// never silently run at `Full` just because the host's global tier says so:
/// nobody is at the keyboard to notice. `Config`-tier channels are operator
/// surfaces and run at the global tier unchanged.
pub const fn clamp_tier_for_channel(level: ChannelPermissionLevel, tier: ExecTier) -> ExecTier {
    match (level, tier) {
        (ChannelPermissionLevel::Chat, ExecTier::Full) => ExecTier::Auto,
        (_, tier) => tier,
    }
}

/// Permission level of the channel a run originated from, derived from the
/// `caller_role` the inbound router stamps into run metadata
/// ([`ChannelPermissionLevel::caller_role_str`]). `None` for Panel / CLI / cron
/// turns, which carry no channel and are therefore not clamped.
#[must_use]
pub fn channel_permission_level_from_role(caller_role: &str) -> Option<ChannelPermissionLevel> {
    match caller_role {
        "guest" => Some(ChannelPermissionLevel::Chat),
        "operator" => Some(ChannelPermissionLevel::Config),
        _ => None,
    }
}

/// E.164 formatted phone number
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct E164Number(pub String);

impl E164Number {
    pub fn new(number: impl Into<String>) -> Self {
        Self(number.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Normalize a phone number to E.164 format.
    ///
    /// 10-digit numbers are assumed to be US/Canada (country code 1).
    /// 11-15 digit numbers are assumed to already include the country code.
    #[must_use]
    pub fn normalize(raw: &str) -> Option<Self> {
        let cleaned: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        if cleaned.len() < 10 || cleaned.len() > 15 {
            return None;
        }
        if cleaned.len() == 10 {
            // Assume US/Canada: prepend country code 1
            Some(Self(format!("+1{cleaned}")))
        } else {
            Some(Self(format!("+{cleaned}")))
        }
    }
}

impl std::fmt::Display for E164Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// DM (direct message) access policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// First message requires pairing approval
    #[default]
    Pairing,
    /// Only explicitly allowlisted senders
    Allowlist,
    /// Anyone can send (requires `allow_from: ["*"]`)
    Open,
    /// No messages accepted
    Disabled,
}

/// Group access policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Only explicitly allowlisted senders
    #[default]
    Allowlist,
    /// Anyone can send
    Open,
    /// No group messages
    Disabled,
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

    /// Bot-to-bot loop protection (active when the channel admits bot-authored
    /// inbound messages). Per-channel override; falls back to global defaults
    /// via [`crate::gateway::pair_loop_guard::resolve_pair_loop_settings`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_loop_protection: Option<PairLoopGuardConfig>,
}

impl Default for ChannelAccessConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Pairing,
            allow_from: Vec::new(),
            group_policy: GroupPolicy::Allowlist,
            group_allow_from: Vec::new(),
            groups: Vec::new(),
            bot_loop_protection: None,
        }
    }
}

// NOTE (熵减 2026-07-17): the `ChannelPolicy` trait, its sole implementor
// `WhatsAppPolicy`, and their `PolicyDecision` result type were deleted here —
// a dead abstraction island with zero consumers. WhatsApp's live policy
// evaluation is `interfaces/whatsapp/wa_policy/{dm_policy,group_policy}.rs`,
// which consumes the `ChannelAccessConfig` data types above directly.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_channel_never_runs_at_full_tier() {
        assert_eq!(
            clamp_tier_for_channel(ChannelPermissionLevel::Chat, ExecTier::Full),
            ExecTier::Auto
        );
        // Lower tiers pass through untouched — the clamp only ever tightens.
        assert_eq!(
            clamp_tier_for_channel(ChannelPermissionLevel::Chat, ExecTier::Ask),
            ExecTier::Ask
        );
        assert_eq!(
            clamp_tier_for_channel(ChannelPermissionLevel::Chat, ExecTier::Auto),
            ExecTier::Auto
        );
        // Operator-tier channels keep the global tier.
        assert_eq!(
            clamp_tier_for_channel(ChannelPermissionLevel::Config, ExecTier::Full),
            ExecTier::Full
        );
    }

    #[test]
    fn test_channel_permission_level_from_role() {
        assert_eq!(
            channel_permission_level_from_role("guest"),
            Some(ChannelPermissionLevel::Chat)
        );
        assert_eq!(
            channel_permission_level_from_role("operator"),
            Some(ChannelPermissionLevel::Config)
        );
        // Panel / CLI / cron turns stamp no channel role → no clamp.
        assert_eq!(channel_permission_level_from_role(""), None);
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
