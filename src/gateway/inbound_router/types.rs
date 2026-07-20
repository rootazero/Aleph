//! Types for the inbound message router

use std::path::PathBuf;

use crate::gateway::execution_engine::BusyInputMode;
use crate::gateway::pairing_store::PairingError;

/// Error type for routing operations
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Execution error: {0}")]
    Execution(String),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error(
        "Link access denied: link \"{link_id}\" is not allowed to access agent \"{agent_id}\""
    )]
    LinkNotAllowed { link_id: String, agent_id: String },

    #[error("Pairing error: {0}")]
    Pairing(#[from] PairingError),
}

/// Permission tier a channel grants its messages, mapped onto Aleph's existing
/// device Chat/Config tier (`gateway/handlers/auth/tier.rs`).
///
/// - `Chat` (Layer 1, default): converse + read. Config-mutating tools are gated
///   (`tools/scoped/dispatch.rs`) and the working directory is locked to the
///   channel's `default_workspace` — the caller cannot choose/create an arbitrary
///   one.
/// - `Config` (Layer 2): the above plus the "Everything is a Tool" config tools
///   and freedom to override the working directory.
///
/// Default is `Chat`, mirroring `default_tier`'s remote=Chat philosophy:
/// untrusted by default, raised explicitly by an operator. This is what closes
/// the prior over-permission where an external channel stamped no role and was
/// therefore treated as operator by `TurnContext::caller_is_operator`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPermissionLevel {
    /// Layer 1: conversation + read, locked workspace. Safe default.
    #[default]
    Chat,
    /// Layer 2: config tools + free workspace choice.
    Config,
}

impl ChannelPermissionLevel {
    /// Connect-style role string this tier maps to, fed into the run's
    /// `caller_role` metadata and read by the tool-dispatch config gate.
    /// `Config` → `"operator"`, `Chat` → `"guest"` — byte-identical to the
    /// strings `tier::role_for_permissions` produces, so the gate is uniform
    /// across WS devices and external channels.
    #[must_use]
    pub const fn caller_role_str(self) -> &'static str {
        match self {
            Self::Config => "operator",
            Self::Chat => "guest",
        }
    }
}

/// Channel permission/workspace tiering, deserialized from each channel
/// instance's flat config block (same pattern as [`SlashAccessConfig`]).
/// All-default → the channel adds nothing over the safe Chat default and boot
/// wiring need not register it.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ChannelPolicyConfig {
    /// Permission tier this channel grants. Default `Chat`.
    #[serde(default)]
    pub permission_level: ChannelPermissionLevel,
    /// Absolute path the Chat tier is locked into. `None` → the agent's own
    /// default workspace. Ignored for `Config` tier (which may override freely).
    #[serde(default)]
    pub default_workspace: Option<PathBuf>,
    /// What to do when a message arrives while this channel's session is already
    /// running a loop: `steer` (default — inject at the next turn boundary),
    /// `interrupt` (cancel the in-flight run; the message restarts as a fresh
    /// run via the busy queue), or `queue` (never disturb the running task; the
    /// message is delivered as a fresh run once it finishes). Set
    /// `busy_input_mode = "interrupt"` / `"queue"` in the channel's config
    /// block to opt in.
    #[serde(default)]
    pub busy_input_mode: BusyInputMode,
    /// Per-channel tool permission override (`tool_permissions` block in the
    /// channel's config): merged by `run_loop` as the third layer over
    /// global + agent permissions, most restrictive wins. Lets an operator
    /// e.g. deny `bash` on a group channel while the same agent keeps it
    /// elsewhere (openclaw per-group tools / opensquilla channel-matrix
    /// parity). `None` → channel adds no tool restrictions.
    #[serde(default)]
    pub tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
}

impl ChannelPolicyConfig {
    /// `true` when the channel configures no tiering over the Chat default —
    /// boot wiring then skips registering it (keeps `channel_configs` minimal).
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.permission_level == ChannelPermissionLevel::Chat
            && self.default_workspace.is_none()
            && self.busy_input_mode == BusyInputMode::Steer
            && self.tool_permissions.is_none()
    }
}

/// Unified channel config for permission checking
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// DM policy
    pub dm_policy: DmPolicy,
    /// Group policy
    pub group_policy: GroupPolicy,
    /// Allowlist for DMs
    pub allow_from: Vec<String>,
    /// Allowlist for groups
    pub group_allow_from: Vec<String>,
    /// Whether to require mention in groups
    pub require_mention: bool,
    /// Bot name for mention detection
    pub bot_name: Option<String>,
    /// Slash-command access tiering for this channel (admin / per-command
    /// allowlists, scoped to DM vs group). Empty → gating OFF (backward-compat).
    pub slash_access: SlashAccessConfig,
    /// Permission tier this channel's messages run at (Layer 1 / Layer 2).
    pub permission_level: ChannelPermissionLevel,
    /// Working directory the Chat tier is locked into (absolute). `None` → the
    /// agent's own default workspace.
    pub default_workspace: Option<PathBuf>,
    /// Busy-input policy for this channel: `Steer` (default), `Interrupt`, or
    /// `Queue`. Stamped into each run's metadata so the execution engine's
    /// busy branch can dispatch without re-reading channel config.
    pub busy_input_mode: BusyInputMode,
    /// Per-channel tool permission override. Stamped (JSON) into each run's
    /// metadata so `run_loop` can merge it over global + agent permissions
    /// without re-reading channel config. `None` → no channel restrictions.
    pub tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
}

/// Per-channel slash-command access tiering.
///
/// Mirrors hermes-agent `SlashAccessPolicy` (per-platform admin + user
/// allowlists, separate DM / group scopes). All-empty → gating is OFF and every
/// already-authorized sender may run every command (pre-tiering behavior).
///
/// Deserialized directly from each channel instance's config block, so the same
/// flat keys (`allow_admin_from`, `user_allowed_commands`, …) work uniformly
/// across every channel type, not just iMessage.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct SlashAccessConfig {
    /// DM-scope admin user IDs — these users can run every registered slash command.
    /// Empty → slash-command gating is OFF for DM scope (backward-compat).
    #[serde(default)]
    pub allow_admin_from: Vec<String>,
    /// DM-scope allowlist of slash command names non-admin senders may run.
    /// Names are stored lowercased without leading `/`. Ignored when gating is OFF.
    #[serde(default)]
    pub user_allowed_commands: Vec<String>,
    /// Group-scope admin user IDs.
    /// Empty → slash-command gating is OFF for group scope (backward-compat).
    #[serde(default)]
    pub group_allow_admin_from: Vec<String>,
    /// Group-scope allowlist of slash command names non-admin senders may run.
    #[serde(default)]
    pub group_user_allowed_commands: Vec<String>,
}

impl SlashAccessConfig {
    /// `true` when no scope has any admin configured, i.e. gating is fully OFF.
    /// Used by boot wiring to skip registering a channel that opts out, keeping
    /// `channel_configs` byte-identical to the pre-tiering allow-all default.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.allow_admin_from.is_empty() && self.group_allow_admin_from.is_empty()
    }
}

/// Outcome of a slash-command access check.
///
/// Returned by [`ChannelConfig::slash_command_gate`]. When `gating_enabled`
/// is `false`, every command is allowed (backward-compat with pre-tiering
/// installs where `allow_admin_from` is empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashAccessDecision {
    /// `true` → caller may invoke the command; `false` → deny.
    pub allowed: bool,
    /// `true` → gating is configured for this scope. When `false`, every
    /// already-authorized sender is treated as admin.
    pub gating_enabled: bool,
}

/// Slash commands that are universally allowed for any user passing the
/// channel allowlist. Mirrors hermes-agent `slash_access._ALWAYS_ALLOWED_FOR_USERS`
/// so opt-in admins do not accidentally lock users out of help / paging.
const ALWAYS_ALLOWED_SLASH_COMMANDS: &[&str] = &["help", "ping", "start", "footer"];

/// Normalize a slash command name for comparison: drop leading `/`, lowercase,
/// strip the optional `@botname` suffix used by Telegram-style commands.
#[must_use]
pub fn normalize_slash_command_name(raw: &str) -> String {
    let trimmed = raw.trim().trim_start_matches('/');
    let without_bot = match trimmed.split_once('@') {
        Some((name, _)) => name,
        None => trimmed,
    };
    let head = match without_bot.split_once(char::is_whitespace) {
        Some((name, _)) => name,
        None => without_bot,
    };
    head.to_lowercase()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmPolicy {
    Open,
    Allowlist,
    Pairing,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupPolicy {
    Open,
    Allowlist,
    Disabled,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Pairing,
            group_policy: GroupPolicy::Open,
            allow_from: Vec::new(),
            group_allow_from: Vec::new(),
            require_mention: true,
            bot_name: None,
            slash_access: SlashAccessConfig::default(),
            permission_level: ChannelPermissionLevel::default(),
            default_workspace: None,
            busy_input_mode: BusyInputMode::default(),
            tool_permissions: None,
        }
    }
}

impl ChannelConfig {
    /// Connect-style role string for this channel's permission tier, stamped
    /// into the run's `caller_role` metadata so the tool-dispatch config gate
    /// applies uniformly to external-channel messages.
    #[must_use]
    pub fn caller_role_str(&self) -> &'static str {
        self.permission_level.caller_role_str()
    }

    /// Wire string for this channel's busy-input policy, stamped into the run's
    /// `busy_input_mode` metadata so the execution engine's busy branch can
    /// dispatch (`steer` / `interrupt` / `queue`) without re-reading channel
    /// config.
    #[must_use]
    pub fn busy_input_mode_wire(&self) -> &'static str {
        self.busy_input_mode.as_wire()
    }

    /// Resolve the Chat-tier locked workspace, if any: only an **absolute,
    /// existing** directory is honored — anything else falls back to `None`
    /// (the agent's own default workspace) rather than handing the engine a
    /// path it cannot safely chdir into.
    #[must_use]
    pub fn resolved_default_workspace(&self) -> Option<PathBuf> {
        let path = self.default_workspace.as_ref()?;
        if path.is_absolute() && path.is_dir() {
            Some(path.clone())
        } else {
            tracing::warn!(
                "channel default_workspace {:?} is not an existing absolute directory; \
                 falling back to the agent default workspace",
                path
            );
            None
        }
    }

    /// Decide whether `sender` may invoke slash command `command_name` in the
    /// requested scope (dm vs group). Thin delegate to [`SlashAccessConfig`].
    #[must_use]
    pub fn slash_command_gate(
        &self,
        sender: &str,
        command_name: &str,
        is_group: bool,
    ) -> SlashAccessDecision {
        self.slash_access
            .slash_command_gate(sender, command_name, is_group)
    }
}

impl SlashAccessConfig {
    /// Decide whether `sender` may invoke slash command `command_name` in
    /// the requested scope (dm vs group).
    ///
    /// Semantics (mirrors hermes-agent `slash_access.policy_for_source`):
    /// - Gating is **off** when the relevant `*_allow_admin_from` list is
    ///   empty → every authorized sender can run every command. This
    ///   preserves behavior of installs that haven't opted in.
    /// - Gating is **on** when at least one admin is configured.
    ///   - Admin users can run anything.
    ///   - Non-admins can run commands in `*_user_allowed_commands` plus
    ///     the small always-allowed set (help, ping, start, footer).
    #[must_use]
    pub fn slash_command_gate(
        &self,
        sender: &str,
        command_name: &str,
        is_group: bool,
    ) -> SlashAccessDecision {
        let (admin_list, user_list) = if is_group {
            (
                &self.group_allow_admin_from,
                &self.group_user_allowed_commands,
            )
        } else {
            (&self.allow_admin_from, &self.user_allowed_commands)
        };

        if admin_list.is_empty() {
            return SlashAccessDecision {
                allowed: true,
                gating_enabled: false,
            };
        }

        let normalized_cmd = normalize_slash_command_name(command_name);
        let is_admin = !sender.is_empty() && admin_list.iter().any(|a| a == sender);
        if is_admin {
            return SlashAccessDecision {
                allowed: true,
                gating_enabled: true,
            };
        }
        if ALWAYS_ALLOWED_SLASH_COMMANDS
            .iter()
            .any(|cmd| *cmd == normalized_cmd)
        {
            return SlashAccessDecision {
                allowed: true,
                gating_enabled: true,
            };
        }
        let allowed = user_list
            .iter()
            .any(|c| normalize_slash_command_name(c) == normalized_cmd);
        SlashAccessDecision {
            allowed,
            gating_enabled: true,
        }
    }
}

// Ungated: the BlueBubbles transport runs on all platforms and needs this
// IMessageConfig -> ChannelConfig conversion for inbound gating off-macOS too.
impl From<&crate::gateway::interfaces::imessage::IMessageConfig> for ChannelConfig {
    fn from(cfg: &crate::gateway::interfaces::imessage::IMessageConfig) -> Self {
        use crate::gateway::interfaces::imessage::{IMessageDmPolicy, IMessageGroupPolicy};

        Self {
            dm_policy: match cfg.dm_policy {
                IMessageDmPolicy::Open => DmPolicy::Open,
                IMessageDmPolicy::Allowlist => DmPolicy::Allowlist,
                IMessageDmPolicy::Pairing => DmPolicy::Pairing,
                IMessageDmPolicy::Disabled => DmPolicy::Disabled,
            },
            group_policy: match cfg.group_policy {
                IMessageGroupPolicy::Open => GroupPolicy::Open,
                IMessageGroupPolicy::Allowlist => GroupPolicy::Allowlist,
                IMessageGroupPolicy::Disabled => GroupPolicy::Disabled,
            },
            allow_from: cfg.allow_from.clone(),
            group_allow_from: cfg.group_allow_from.clone(),
            require_mention: cfg.require_mention,
            bot_name: cfg.bot_name.clone(),
            slash_access: SlashAccessConfig {
                allow_admin_from: cfg.allow_admin_from.clone(),
                user_allowed_commands: cfg.user_allowed_commands.clone(),
                group_allow_admin_from: cfg.group_allow_admin_from.clone(),
                group_user_allowed_commands: cfg.group_user_allowed_commands.clone(),
            },
            // iMessage carries no per-channel tier override yet; safe Chat default.
            permission_level: ChannelPermissionLevel::default(),
            default_workspace: None,
            busy_input_mode: BusyInputMode::default(),
            // Boot wiring overlays `tool_permissions` from the instance's flat
            // config block (same ChannelPolicyConfig parse as other channels).
            tool_permissions: None,
        }
    }
}

// Bridge the Telegram channel's own multi-account config into the central
// permission layer so the inbound router is the single source of truth for
// Telegram access (R4). Gating is driven by the default (first) account —
// matching the channel-local access controller's account-level resolution.
// Without this bridge the router fell back to `ChannelConfig::default()`
// (Pairing / Open), silently ignoring the operator's Telegram dm_policy /
// group_policy / allowlists.
impl From<&crate::gateway::interfaces::telegram::TelegramConfigV2> for ChannelConfig {
    fn from(cfg: &crate::gateway::interfaces::telegram::TelegramConfigV2) -> Self {
        use crate::gateway::interfaces::telegram::config_v2::{
            DmPolicy as TgDm, GroupPolicy as TgGroup,
        };

        let account = cfg.accounts.first();
        let dm_policy = account.and_then(|a| a.dm_policy.clone()).unwrap_or_default();
        let group_policy = account
            .and_then(|a| a.group_policy.clone())
            .unwrap_or_default();
        let allowed_users = account
            .and_then(|a| a.allowed_users.clone())
            .unwrap_or_default();
        let allowed_groups = account
            .and_then(|a| a.allowed_groups.clone())
            .unwrap_or_default();
        // Matches the interface default (mod.rs): groups respond to every message
        // unless `require_mention` is explicitly enabled.
        let require_mention = account.and_then(|a| a.require_mention).unwrap_or(false);
        let bot_name = account.and_then(|a| a.bot_username.clone());

        Self {
            dm_policy: match dm_policy {
                TgDm::Open => DmPolicy::Open,
                TgDm::Allowlist => DmPolicy::Allowlist,
                TgDm::Pairing => DmPolicy::Pairing,
                TgDm::Disabled => DmPolicy::Disabled,
            },
            group_policy: match group_policy {
                TgGroup::Open => GroupPolicy::Open,
                // Telegram semantic: `Allowlist` with an empty `allowed_groups`
                // means "allow all groups" (see access.rs
                // test_group_allowlist_empty_allows_all). The router's Allowlist
                // *denies* when `group_allow_from` is empty, so preserve the
                // "allow all" intent by mapping the empty case to Open.
                TgGroup::Allowlist if allowed_groups.is_empty() => GroupPolicy::Open,
                TgGroup::Allowlist => GroupPolicy::Allowlist,
                TgGroup::Disabled => GroupPolicy::Disabled,
            },
            allow_from: allowed_users.iter().map(i64::to_string).collect(),
            group_allow_from: allowed_groups.iter().map(i64::to_string).collect(),
            require_mention,
            bot_name,
            slash_access: SlashAccessConfig::default(),
            permission_level: ChannelPermissionLevel::default(),
            default_workspace: None,
            busy_input_mode: BusyInputMode::default(),
            // Boot wiring overlays `tool_permissions` from the instance's flat
            // config block (same ChannelPolicyConfig parse as other channels).
            tool_permissions: None,
        }
    }
}

#[cfg(test)]
mod telegram_bridge_tests {
    use super::*;
    use crate::gateway::interfaces::telegram::config_v2::{
        DmPolicy as TgDm, GroupPolicy as TgGroup, TelegramAccountConfig, TelegramConfigV2,
    };

    fn cfg(account: TelegramAccountConfig) -> TelegramConfigV2 {
        TelegramConfigV2 {
            accounts: vec![account],
            coalescing: None,
        }
    }

    #[test]
    fn dm_pairing_and_allowlist_are_mapped() {
        let c = ChannelConfig::from(&cfg(TelegramAccountConfig {
            dm_policy: Some(TgDm::Allowlist),
            allowed_users: Some(vec![123, 456]),
            ..Default::default()
        }));
        assert!(matches!(c.dm_policy, DmPolicy::Allowlist));
        assert_eq!(c.allow_from, vec!["123".to_string(), "456".to_string()]);
    }

    #[test]
    fn empty_allowlist_group_maps_to_open() {
        // Telegram's `Allowlist` + empty groups == "allow all"; the router's
        // Allowlist would deny on empty, so it must become Open.
        let c = ChannelConfig::from(&cfg(TelegramAccountConfig {
            group_policy: Some(TgGroup::Allowlist),
            allowed_groups: None,
            ..Default::default()
        }));
        assert!(matches!(c.group_policy, GroupPolicy::Open));
        assert!(c.group_allow_from.is_empty());
    }

    #[test]
    fn nonempty_allowlist_group_stays_allowlist() {
        let c = ChannelConfig::from(&cfg(TelegramAccountConfig {
            group_policy: Some(TgGroup::Allowlist),
            allowed_groups: Some(vec![-100111]),
            ..Default::default()
        }));
        assert!(matches!(c.group_policy, GroupPolicy::Allowlist));
        assert_eq!(c.group_allow_from, vec!["-100111".to_string()]);
    }

    #[test]
    fn require_mention_defaults_to_false() {
        let c = ChannelConfig::from(&cfg(TelegramAccountConfig::default()));
        assert!(!c.require_mention);
    }

    #[test]
    fn no_accounts_falls_back_to_telegram_defaults() {
        // Empty accounts → telegram policy defaults: DM Pairing, group Allowlist
        // (which, with no allowed_groups, maps to Open).
        let c = ChannelConfig::from(&TelegramConfigV2 {
            accounts: vec![],
            coalescing: None,
        });
        assert!(matches!(c.dm_policy, DmPolicy::Pairing));
        assert!(matches!(c.group_policy, GroupPolicy::Open));
    }
}

/// Check if a link (channel) is allowed to access an agent.
pub(crate) fn check_link_access(
    allowed_links: &Option<Vec<String>>,
    link_id: &str,
    agent_id: &str,
) -> Result<(), RoutingError> {
    match allowed_links {
        None => Ok(()),
        Some(list) if list.is_empty() => Ok(()),
        Some(list) => {
            if list.iter().any(|l| l == link_id) {
                Ok(())
            } else {
                Err(RoutingError::LinkNotAllowed {
                    link_id: link_id.to_string(),
                    agent_id: agent_id.to_string(),
                })
            }
        }
    }
}

/// Metadata key for slash command execution mode in `RunRequest`
pub const SLASH_COMMAND_MODE_KEY: &str = "slash_command_mode";

#[cfg(test)]
mod slash_access_tests {
    use super::*;

    fn cfg_with_admins(admins: &[&str], allowed: &[&str], is_group: bool) -> ChannelConfig {
        let mut c = ChannelConfig::default();
        if is_group {
            c.slash_access.group_allow_admin_from =
                admins.iter().map(|s| (*s).to_string()).collect();
            c.slash_access.group_user_allowed_commands =
                allowed.iter().map(|s| (*s).to_string()).collect();
        } else {
            c.slash_access.allow_admin_from = admins.iter().map(|s| (*s).to_string()).collect();
            c.slash_access.user_allowed_commands =
                allowed.iter().map(|s| (*s).to_string()).collect();
        }
        c
    }

    #[test]
    fn gating_off_when_no_admins_configured() {
        let cfg = ChannelConfig::default();
        let d = cfg.slash_command_gate("alice", "image", false);
        assert!(!d.gating_enabled);
        assert!(d.allowed);
    }

    #[test]
    fn admin_runs_anything_dm() {
        let cfg = cfg_with_admins(&["alice"], &[], false);
        let d = cfg.slash_command_gate("alice", "image", false);
        assert!(d.gating_enabled);
        assert!(d.allowed);
    }

    #[test]
    fn non_admin_blocked_when_command_not_in_user_allowed() {
        let cfg = cfg_with_admins(&["alice"], &["status"], false);
        let d = cfg.slash_command_gate("bob", "image", false);
        assert!(d.gating_enabled);
        assert!(!d.allowed);
    }

    #[test]
    fn non_admin_allowed_when_command_in_user_allowed() {
        let cfg = cfg_with_admins(&["alice"], &["status", "ping"], false);
        let d = cfg.slash_command_gate("bob", "status", false);
        assert!(d.allowed);
    }

    #[test]
    fn always_allowed_commands_pass_for_non_admin() {
        let cfg = cfg_with_admins(&["alice"], &[], false);
        for cmd in ["help", "ping", "start", "footer"] {
            let d = cfg.slash_command_gate("bob", cmd, false);
            assert!(d.allowed, "{cmd} should be always allowed");
        }
    }

    #[test]
    fn group_scope_is_independent_from_dm() {
        // gating only enabled for DM
        let cfg = cfg_with_admins(&["alice"], &["status"], false);
        let d = cfg.slash_command_gate("bob", "image", true);
        // group has no admins → gating off → allowed
        assert!(!d.gating_enabled);
        assert!(d.allowed);
    }

    #[test]
    fn normalize_handles_telegram_bot_suffix() {
        assert_eq!(normalize_slash_command_name("/Image@MyBot foo"), "image");
        assert_eq!(normalize_slash_command_name("Status"), "status");
        assert_eq!(normalize_slash_command_name("/help"), "help");
        assert_eq!(normalize_slash_command_name(""), "");
    }

    #[test]
    fn empty_sender_with_gating_on_is_non_admin() {
        let cfg = cfg_with_admins(&["alice"], &["status"], false);
        let d = cfg.slash_command_gate("", "image", false);
        assert!(d.gating_enabled);
        assert!(!d.allowed);
    }

    #[test]
    fn slash_access_is_empty_only_when_no_admins() {
        assert!(SlashAccessConfig::default().is_empty());
        let dm_only = SlashAccessConfig {
            allow_admin_from: vec!["alice".into()],
            ..Default::default()
        };
        assert!(!dm_only.is_empty());
        let group_only = SlashAccessConfig {
            group_allow_admin_from: vec!["bob".into()],
            ..Default::default()
        };
        assert!(!group_only.is_empty());
        // user_allowed_commands without admins still counts as "off" — gating
        // never activates without an admin, so we must not register it.
        let user_list_only = SlashAccessConfig {
            user_allowed_commands: vec!["status".into()],
            ..Default::default()
        };
        assert!(user_list_only.is_empty());
    }

    #[test]
    fn slash_access_deserializes_flat_keys_ignoring_channel_fields() {
        // Mirrors what boot wiring does: parse a full channel instance config
        // value into SlashAccessConfig, picking up only the slash keys and
        // ignoring unrelated channel fields (no deny_unknown_fields).
        let raw = serde_json::json!({
            "bot_token": "secret",
            "polling_interval_secs": 5,
            "allow_admin_from": ["111"],
            "group_user_allowed_commands": ["status", "ping"],
        });
        let sa: SlashAccessConfig = serde_json::from_value(raw).expect("parses");
        assert_eq!(sa.allow_admin_from, vec!["111".to_string()]);
        assert_eq!(
            sa.group_user_allowed_commands,
            vec!["status".to_string(), "ping".to_string()]
        );
        assert!(sa.user_allowed_commands.is_empty());
        assert!(!sa.is_empty());

        // A channel config with no slash keys parses to the empty (allow-all) form.
        let bare = serde_json::json!({ "bot_token": "secret" });
        let sa2: SlashAccessConfig = serde_json::from_value(bare).expect("parses");
        assert!(sa2.is_empty());
    }
}

#[cfg(test)]
mod permission_tier_tests {
    use super::*;

    #[test]
    fn default_tier_is_chat() {
        assert_eq!(
            ChannelPermissionLevel::default(),
            ChannelPermissionLevel::Chat
        );
        assert_eq!(
            ChannelConfig::default().permission_level,
            ChannelPermissionLevel::Chat
        );
        // Safe default: an unconfigured channel maps to the gated "guest" role.
        assert_eq!(ChannelConfig::default().caller_role_str(), "guest");
    }

    #[test]
    fn caller_role_str_maps_tiers_to_connect_role_strings() {
        assert_eq!(ChannelPermissionLevel::Chat.caller_role_str(), "guest");
        assert_eq!(ChannelPermissionLevel::Config.caller_role_str(), "operator");
    }

    #[test]
    fn policy_deserializes_flat_keys_ignoring_channel_fields() {
        let raw = serde_json::json!({
            "bot_token": "secret",
            "permission_level": "config",
            "default_workspace": "/srv/work",
        });
        let p: ChannelPolicyConfig = serde_json::from_value(raw).expect("parses");
        assert_eq!(p.permission_level, ChannelPermissionLevel::Config);
        assert_eq!(
            p.default_workspace.as_deref(),
            Some(std::path::Path::new("/srv/work"))
        );
        assert!(!p.is_default());
    }

    #[test]
    fn policy_parses_busy_input_mode_and_affects_is_default() {
        // Opting a channel into interrupt mode is a non-default policy even when
        // permission/workspace are left at the safe Chat default — boot wiring
        // must register it so the metadata stamp actually fires.
        let raw = serde_json::json!({
            "bot_token": "secret",
            "busy_input_mode": "interrupt",
        });
        let p: ChannelPolicyConfig = serde_json::from_value(raw).expect("parses");
        assert_eq!(p.busy_input_mode, BusyInputMode::Interrupt);
        assert_eq!(p.permission_level, ChannelPermissionLevel::Chat);
        assert!(!p.is_default());
        // Wire round-trips through the stamping helper.
        assert_eq!(
            ChannelConfig {
                busy_input_mode: BusyInputMode::Interrupt,
                ..Default::default()
            }
            .busy_input_mode_wire(),
            "interrupt"
        );
        assert_eq!(ChannelConfig::default().busy_input_mode_wire(), "steer");
    }

    #[test]
    fn policy_parses_queue_busy_input_mode() {
        // The follow-up lane: queue mode must parse, register as non-default
        // (so the metadata stamp fires), and round-trip its wire string.
        let raw = serde_json::json!({
            "bot_token": "secret",
            "busy_input_mode": "queue",
        });
        let p: ChannelPolicyConfig = serde_json::from_value(raw).expect("parses");
        assert_eq!(p.busy_input_mode, BusyInputMode::Queue);
        assert!(!p.is_default());
        assert_eq!(
            ChannelConfig {
                busy_input_mode: BusyInputMode::Queue,
                ..Default::default()
            }
            .busy_input_mode_wire(),
            "queue"
        );
        assert_eq!(
            BusyInputMode::from_wire(Some("queue")),
            BusyInputMode::Queue
        );
    }

    #[test]
    fn policy_bare_config_is_default_chat() {
        let bare = serde_json::json!({ "bot_token": "secret" });
        let p: ChannelPolicyConfig = serde_json::from_value(bare).expect("parses");
        assert_eq!(p.permission_level, ChannelPermissionLevel::Chat);
        assert_eq!(p.busy_input_mode, BusyInputMode::Steer);
        assert!(p.default_workspace.is_none());
        assert!(p.is_default(), "no tiering configured → skip registration");
    }

    #[test]
    fn config_tier_alone_is_not_default() {
        let raw = serde_json::json!({ "permission_level": "config" });
        let p: ChannelPolicyConfig = serde_json::from_value(raw).expect("parses");
        assert!(!p.is_default());
    }

    #[test]
    fn resolved_default_workspace_rejects_relative_or_missing() {
        // Relative path → None (never handed to the engine).
        let cfg = ChannelConfig {
            default_workspace: Some(std::path::PathBuf::from("relative/dir")),
            ..Default::default()
        };
        assert!(cfg.resolved_default_workspace().is_none());

        // Absolute but non-existent → None.
        let cfg = ChannelConfig {
            default_workspace: Some(std::path::PathBuf::from("/nonexistent/aleph/ws/xyz")),
            ..Default::default()
        };
        assert!(cfg.resolved_default_workspace().is_none());

        // Absolute + existing dir → honored.
        let cfg = ChannelConfig {
            default_workspace: Some(std::env::temp_dir()),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_default_workspace(), Some(std::env::temp_dir()));
    }

    /// A `tool_permissions` block in a channel's flat config parses into the
    /// policy and disqualifies the channel from the skip-registration default
    /// — otherwise the override would be silently dropped at boot.
    #[test]
    fn policy_tool_permissions_parse_and_break_default() {
        use crate::extension::PermissionAction;

        let raw = serde_json::json!({
            "bot_token": "secret",
            "tool_permissions": {
                "default": "allow",
                "overrides": { "bash": "deny", "file_write": "ask" }
            }
        });
        let p: ChannelPolicyConfig = serde_json::from_value(raw).expect("parses");
        assert!(
            !p.is_default(),
            "tool_permissions alone must force registration"
        );
        let perms = p.tool_permissions.expect("block surfaces");
        assert_eq!(perms.resolve("bash"), PermissionAction::Deny);
        assert_eq!(perms.resolve("file_write"), PermissionAction::Ask);
        assert_eq!(perms.resolve("read_file"), PermissionAction::Allow);
    }
}
