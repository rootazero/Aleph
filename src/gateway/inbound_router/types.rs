//! Types for the inbound message router

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
    /// DM-scope admin user IDs — these users can run every registered slash command.
    /// Empty → slash-command gating is OFF for DM scope (backward-compat).
    pub allow_admin_from: Vec<String>,
    /// DM-scope allowlist of slash command names non-admin senders may run.
    /// Names are stored lowercased without leading `/`. Ignored when gating is OFF.
    pub user_allowed_commands: Vec<String>,
    /// Group-scope admin user IDs.
    /// Empty → slash-command gating is OFF for group scope (backward-compat).
    pub group_allow_admin_from: Vec<String>,
    /// Group-scope allowlist of slash command names non-admin senders may run.
    pub group_user_allowed_commands: Vec<String>,
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
    /// `true` → caller is admin (only meaningful when `gating_enabled`).
    pub is_admin: bool,
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
            allow_admin_from: Vec::new(),
            user_allowed_commands: Vec::new(),
            group_allow_admin_from: Vec::new(),
            group_user_allowed_commands: Vec::new(),
        }
    }
}

impl ChannelConfig {
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
                is_admin: true,
            };
        }

        let normalized_cmd = normalize_slash_command_name(command_name);
        let is_admin = !sender.is_empty() && admin_list.iter().any(|a| a == sender);
        if is_admin {
            return SlashAccessDecision {
                allowed: true,
                gating_enabled: true,
                is_admin: true,
            };
        }
        if ALWAYS_ALLOWED_SLASH_COMMANDS
            .iter()
            .any(|cmd| *cmd == normalized_cmd)
        {
            return SlashAccessDecision {
                allowed: true,
                gating_enabled: true,
                is_admin: false,
            };
        }
        let allowed = user_list
            .iter()
            .any(|c| normalize_slash_command_name(c) == normalized_cmd);
        SlashAccessDecision {
            allowed,
            gating_enabled: true,
            is_admin: false,
        }
    }
}

#[cfg(target_os = "macos")]
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
            allow_admin_from: cfg.allow_admin_from.clone(),
            user_allowed_commands: cfg.user_allowed_commands.clone(),
            group_allow_admin_from: cfg.group_allow_admin_from.clone(),
            group_user_allowed_commands: cfg.group_user_allowed_commands.clone(),
        }
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

/// Metadata key for slash command execution mode in RunRequest
pub const SLASH_COMMAND_MODE_KEY: &str = "slash_command_mode";

#[cfg(test)]
mod slash_access_tests {
    use super::*;

    fn cfg_with_admins(admins: &[&str], allowed: &[&str], is_group: bool) -> ChannelConfig {
        let mut c = ChannelConfig::default();
        if is_group {
            c.group_allow_admin_from = admins.iter().map(|s| (*s).to_string()).collect();
            c.group_user_allowed_commands = allowed.iter().map(|s| (*s).to_string()).collect();
        } else {
            c.allow_admin_from = admins.iter().map(|s| (*s).to_string()).collect();
            c.user_allowed_commands = allowed.iter().map(|s| (*s).to_string()).collect();
        }
        c
    }

    #[test]
    fn gating_off_when_no_admins_configured() {
        let cfg = ChannelConfig::default();
        let d = cfg.slash_command_gate("alice", "image", false);
        assert!(!d.gating_enabled);
        assert!(d.allowed);
        assert!(d.is_admin, "with gating off everyone is treated as admin");
    }

    #[test]
    fn admin_runs_anything_dm() {
        let cfg = cfg_with_admins(&["alice"], &[], false);
        let d = cfg.slash_command_gate("alice", "image", false);
        assert!(d.gating_enabled);
        assert!(d.is_admin);
        assert!(d.allowed);
    }

    #[test]
    fn non_admin_blocked_when_command_not_in_user_allowed() {
        let cfg = cfg_with_admins(&["alice"], &["status"], false);
        let d = cfg.slash_command_gate("bob", "image", false);
        assert!(d.gating_enabled);
        assert!(!d.is_admin);
        assert!(!d.allowed);
    }

    #[test]
    fn non_admin_allowed_when_command_in_user_allowed() {
        let cfg = cfg_with_admins(&["alice"], &["status", "ping"], false);
        let d = cfg.slash_command_gate("bob", "status", false);
        assert!(d.allowed);
        assert!(!d.is_admin);
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
        assert!(!d.is_admin);
        assert!(!d.allowed);
    }
}
