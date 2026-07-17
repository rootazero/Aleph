//! Channel Access Control Policy
//!
//! Provides fine-grained access control for channels supporting DM and group
//! messaging, with configurable policies and allowlists.

use crate::config::types::policies::ExecTier;
use crate::gateway::channel::UserId;
use crate::gateway::execution_engine::CHANNEL_TOOL_PERMISSIONS_KEY;
use crate::gateway::inbound_router::{ChannelConfig, ChannelPermissionLevel};
use crate::gateway::pair_loop_guard::PairLoopGuardConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

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

/// Process-global snapshot of the boot-assembled `channel_id → ChannelConfig`
/// map. The live map is owned privately by the inbound router — built once at
/// boot via `register_channel_config`, immutable after the router is `Arc::new`d
/// (there is no runtime re-registration). This read-only snapshot is published
/// once from that map at the end of `initialize_inbound_router`, so a
/// system-initiated continuation (goal wait-barrier wake / boot resume) firing
/// long after boot can consult the SAME channel deny layer a live inbound
/// message gets — which those paths otherwise cannot reach (the config map is in
/// no global; `AgentInstance::origin_route` returns only channel + conversation,
/// no permission data). `None` until boot sets it (tests / pre-channel-init) →
/// callers fail closed to guest + no deny layer.
static CHANNEL_CONFIG_SNAPSHOT: OnceLock<HashMap<String, ChannelConfig>> = OnceLock::new();

/// Publish the boot channel-config snapshot. Called once from
/// `initialize_inbound_router` after every `register_channel_config`, before the
/// router is sealed in `Arc`. Idempotent — a later set (e.g. a second boot in a
/// test process) is ignored by the `OnceLock`.
pub fn set_channel_config_snapshot(configs: HashMap<String, ChannelConfig>) {
    if CHANNEL_CONFIG_SNAPSHOT.set(configs).is_err() {
        tracing::debug!("channel config snapshot already published; ignoring re-set");
    }
}

/// Run-identity metadata for a **system-initiated continuation** (goal
/// wait-barrier wake / boot resume) whose session origin is a channel. Both are
/// woken by the daemon with no live human at the keyboard and no completing run
/// to inherit policy metadata from, so this is deliberately fail-closed on BOTH
/// axes the live inbound path derives from channel config:
///
/// - `caller_role = "guest"` FLOOR — an unattended continuation never silently
///   runs at `operator`, even on a `Config`-tier channel. (Round-6 stamped this
///   floor for wakes; generalized here so resume shares it — see the wake
///   identity doc in `execution_engine::goal_wait`.)
/// - the channel's own `tool_permissions` DENY layer IS honored
///   (`CHANNEL_TOOL_PERMISSIONS_KEY`), so a wake/resume never bypasses an
///   admin's explicit per-channel tool deny. Dropping it was the fail-open gap
///   this closes — both the wake path (never stamped it) and the resume path
///   (its config map was never wired) silently ran without it before.
///
/// A snapshot miss (unknown/unconfigured channel, or boot not yet complete)
/// resolves to guest + no deny via `unwrap_or_default` — the same fail-closed
/// default `channel_run_identity` pins for a live message on an unknown channel.
#[must_use]
pub fn system_continuation_identity(channel: &str, conversation: &str) -> HashMap<String, String> {
    let cfg = CHANNEL_CONFIG_SNAPSHOT
        .get()
        .and_then(|m| m.get(channel).cloned())
        .unwrap_or_default();
    channel_identity_meta(&cfg, channel, conversation)
}

/// Pure cfg → identity metadata (guest floor + deny layer), split from the
/// global lookup so the deny-layer serialization is unit-testable with a
/// hand-built `ChannelConfig` (the snapshot is a set-once process global).
fn channel_identity_meta(
    cfg: &ChannelConfig,
    channel: &str,
    conversation: &str,
) -> HashMap<String, String> {
    let mut meta = HashMap::new();
    meta.insert("caller_role".to_string(), "guest".to_string());
    if let Some(perms) = cfg.tool_permissions.as_ref() {
        match serde_json::to_string(perms) {
            Ok(json) => {
                meta.insert(CHANNEL_TOOL_PERMISSIONS_KEY.to_string(), json);
            }
            // The guest floor above still stands; only the deny layer is lost.
            Err(e) => tracing::error!(
                channel = %channel,
                error = %e,
                "system continuation: channel tool_permissions failed to serialize — deny layer skipped"
            ),
        }
    }
    meta.insert("channel_id".to_string(), channel.to_string());
    meta.insert("conversation_id".to_string(), conversation.to_string());
    meta
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

/// Policy evaluation result
#[derive(Debug, Clone)]
pub struct PolicyDecision {
    /// Whether the action is allowed
    pub allowed: bool,
    /// Human-readable reason (for denied decisions)
    pub reason: Option<String>,
}

impl PolicyDecision {
    #[must_use]
    pub const fn allowed() -> Self {
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

/// Standard `WhatsApp` policy implementation
#[derive(Debug, Clone)]
pub struct WhatsAppPolicy {
    config: ChannelAccessConfig,
}

impl WhatsAppPolicy {
    #[must_use]
    pub const fn new(config: ChannelAccessConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub const fn config(&self) -> &ChannelAccessConfig {
        &self.config
    }

    fn matches_allowlist(&self, sender: &UserId, allowlist: &[String]) -> bool {
        allowlist.iter().any(|entry| {
            if entry == "*" {
                return true;
            }
            if entry == sender.as_str() {
                return true;
            }
            // Normalize both sides for E.164 comparison
            if let (Some(entry_norm), Some(sender_norm)) = (
                E164Number::normalize(entry),
                E164Number::normalize(sender.as_str()),
            ) {
                return entry_norm.as_str() == sender_norm.as_str();
            }
            false
        })
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
            bot_loop_protection: None,
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
    fn system_continuation_identity_is_guest_floor_with_no_deny_by_default() {
        // A channel with no tool_permissions override → guest floor, channel +
        // conversation stamped, and NO deny-layer key (the fail-closed default a
        // snapshot miss also lands on).
        let cfg = ChannelConfig::default();
        let meta = channel_identity_meta(&cfg, "telegram", "chat-42");
        assert_eq!(meta.get("caller_role").map(String::as_str), Some("guest"));
        assert_eq!(meta.get("channel_id").map(String::as_str), Some("telegram"));
        assert_eq!(
            meta.get("conversation_id").map(String::as_str),
            Some("chat-42")
        );
        assert!(
            !meta.contains_key(CHANNEL_TOOL_PERMISSIONS_KEY),
            "no channel tool_permissions ⇒ no deny layer key"
        );
    }

    #[test]
    fn system_continuation_identity_carries_the_channel_deny_layer() {
        // A channel that denies a (non-operator) tool: the wake/resume run must
        // carry that deny layer — the fail-open gap this closes. caller_role
        // stays the guest floor regardless of the channel's tier.
        use crate::config::types::policies::ToolPermissionsConfig;
        use crate::extension::PermissionAction;
        let mut cfg = ChannelConfig::default();
        cfg.permission_level = ChannelPermissionLevel::Config; // operator-tier channel…
        cfg.tool_permissions = Some(ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: HashMap::from([("web_fetch".to_string(), PermissionAction::Deny)]),
        });
        let meta = channel_identity_meta(&cfg, "slack", "C123");
        // …yet an unattended continuation still runs at the guest floor.
        assert_eq!(meta.get("caller_role").map(String::as_str), Some("guest"));
        let deny = meta
            .get(CHANNEL_TOOL_PERMISSIONS_KEY)
            .expect("deny layer must be stamped");
        assert!(
            deny.contains("web_fetch") && deny.contains("deny"),
            "serialized deny layer should preserve the per-channel override, got: {deny}"
        );
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
