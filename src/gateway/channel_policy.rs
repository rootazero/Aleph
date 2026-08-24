//! Channel Access Control Policy
//!
//! The channel-level rungs of the permission hierarchy: the tier clamp for
//! untrusted channels ([`clamp_tier_for_channel`]), the access-policy config
//! types ([`ChannelAccessConfig`] / [`DmPolicy`] / [`GroupPolicy`]) consumed by
//! channel adapters (WhatsApp evaluates them in `wa_policy/`; the inbound
//! router has its own `ChannelConfig` twin), and [`E164Number`] normalization.

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::config::types::policies::ExecTier;
use crate::gateway::execution_engine::CHANNEL_TOOL_PERMISSIONS_KEY;
use crate::gateway::inbound_router::{ChannelConfig, ChannelPermissionLevel};
use crate::gateway::pair_loop_guard::PairLoopGuardConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::Notify;

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
///
/// `IndistinguishableDefault`, on the axis that matters. The guest floor is
/// unconditional (`channel_identity_meta` writes it before consulting the
/// config at all), so losing the snapshot cannot promote anyone. What it loses
/// is the DENY layer: [`system_continuation_identity`]'s `unwrap_or_default()`
/// yields a `ChannelConfig::default()`, whose `tool_permissions` is `None`, and
/// the resulting metadata is byte-identical to the metadata for a channel the
/// operator never configured. An admin's explicit per-channel deny is simply
/// not there, and no caller can tell that apart from "this channel has no
/// denies" — which is the fail-open gap the paragraph above says this snapshot
/// exists to close.
///
/// ⚠️ Read alongside [`wait_for_channel_config_snapshot`]: boot scans park on
/// the publish rather than racing it, so on a healthy boot this default is
/// unreachable. The diagnostic is for the boot that is not healthy.
static CHANNEL_CONFIG_SNAPSHOT: CapabilitySlot<HashMap<String, ChannelConfig>> =
    CapabilitySlot::new(
        "gateway/channel-config-snapshot",
        MissingSemantics::IndistinguishableDefault {
            reads_as: "ChannelConfig::default() -- the guest floor with NO per-channel \
                       tool-deny layer, identical to an unconfigured channel",
        },
    );

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn channel_config_snapshot_slot() -> &'static dyn SlotStatus {
    &CHANNEL_CONFIG_SNAPSHOT
}

/// Notified once the snapshot is published, so boot-time system-continuation
/// scans (resume / goal wake) that are spawned *before*
/// `initialize_inbound_router` runs can park on
/// [`wait_for_channel_config_snapshot`] instead of racing the publish and
/// silently dropping the per-channel deny layer.
static SNAPSHOT_READY: Notify = Notify::const_new();

/// Publish the boot channel-config snapshot. Called once from
/// `initialize_inbound_router` after every `register_channel_config`, before the
/// router is sealed in `Arc`. Idempotent — a later set (e.g. a second boot in a
/// test process) is ignored by the `OnceLock`.
pub fn set_channel_config_snapshot(configs: HashMap<String, ChannelConfig>) {
    if !CHANNEL_CONFIG_SNAPSHOT.install(configs) {
        tracing::debug!("channel config snapshot already published; ignoring re-set");
    }
    // Wake any boot scans parked in `wait_for_channel_config_snapshot`. Callers
    // that arrive after this see the `OnceLock` already set and never park.
    SNAPSHOT_READY.notify_waiters();
}

/// Park until the boot channel-config snapshot is published, or `timeout`
/// elapses. Returns `true` if the snapshot is available.
///
/// The resume boot scan (`ResumeCoordinator`) and the goal-wake boot sweep
/// (`GoalWakeService::rearm_parked_goals`) are both spawned early in boot —
/// before `initialize_inbound_router` assembles and publishes the snapshot —
/// yet both derive [`system_continuation_identity`], which reads it. Without
/// this gate a fast boot scan reads the snapshot before it exists, hits the
/// `unwrap_or_default` miss path, and silently runs a resumed/woken turn WITHOUT
/// the channel's per-channel tool-deny layer (the fail-open gap this closes).
/// Both scans call this first; the publish is unconditional at boot end, so in a
/// healthy boot the wait resolves in milliseconds. On timeout the caller
/// proceeds with the guest-floor + no-deny fallback (correct when no router ever
/// publishes — e.g. a channel-less deployment). Callers proceed regardless of
/// the outcome, so the bool is informational (returned for tests / future use).
pub async fn wait_for_channel_config_snapshot(timeout: std::time::Duration) -> bool {
    if CHANNEL_CONFIG_SNAPSHOT.get().is_some() {
        return true;
    }
    let park = async {
        loop {
            // Register interest BEFORE re-checking so a publish racing between
            // the check and the await cannot be lost.
            let notified = SNAPSHOT_READY.notified();
            if CHANNEL_CONFIG_SNAPSHOT.get().is_some() {
                return;
            }
            notified.await;
            if CHANNEL_CONFIG_SNAPSHOT.get().is_some() {
                return;
            }
        }
    };
    tokio::time::timeout(timeout, park).await.is_ok()
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
/// A snapshot miss (unknown/unconfigured channel) resolves to guest + no deny
/// via `unwrap_or_default` — the same fail-closed default `channel_run_identity`
/// pins for a live message on an unknown channel. Boot-time system-continuation
/// scans (resume / goal wake) park on [`wait_for_channel_config_snapshot`] first,
/// so a *configured* channel's deny layer is never dropped merely by racing the
/// boot publish.
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

// NOTE (entropy reduction 2026-07-17): the `ChannelPolicy` trait, its sole implementor
// `WhatsAppPolicy`, and their `PolicyDecision` result type were deleted here —
// a dead abstraction island with zero consumers. WhatsApp's live policy
// evaluation is `interfaces/whatsapp/wa_policy/{dm_policy,group_policy}.rs`,
// which consumes the `ChannelAccessConfig` data types above directly.

#[cfg(test)]
mod tests {
    use super::*;

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    ///
    /// The `reads_as` sentence is asserted, not just the variant: it is shown
    /// to an operator verbatim, and a sentence naming the wrong default is a
    /// diagnostic that lies (Task 8 caught exactly that in a brief's table).
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = channel_config_snapshot_slot();
        assert_eq!(slot.id(), "gateway/channel-config-snapshot");
        match slot.missing() {
            MissingSemantics::IndistinguishableDefault { reads_as } => assert!(
                reads_as.contains("ChannelConfig::default()"),
                "the sentence must name the real fallback, not a nearby \
                 constant; got {reads_as:?}"
            ),
            other => panic!("expected IndistinguishableDefault, got {other:?}"),
        }
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
        let cfg = ChannelConfig {
            permission_level: ChannelPermissionLevel::Config, // operator-tier channel…
            tool_permissions: Some(ToolPermissionsConfig {
                default: PermissionAction::Allow,
                overrides: HashMap::from([("web_fetch".to_string(), PermissionAction::Deny)]),
            }),
            ..ChannelConfig::default()
        };
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
