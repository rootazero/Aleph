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

    #[error("Link access denied: link \"{link_id}\" is not allowed to access agent \"{agent_id}\"")]
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
