//! Agent management tools — create, switch, list, delete agents at runtime.

pub mod create;
pub mod delete;
pub mod list;
pub mod switch;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Shared session context injected by ExecutionEngine each run.
///
/// Carries the channel and peer_id of the current conversation so that
/// agent management tools can auto-switch the active agent for the caller.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub channel: String,
    pub peer_id: String,
}

pub type SessionContextHandle = Arc<RwLock<SessionContext>>;

pub fn new_session_context_handle() -> SessionContextHandle {
    Arc::new(RwLock::new(SessionContext::default()))
}

/// Per-agent tool access policy injected by ExecutionEngine each run.
#[derive(Debug, Clone, Default)]
pub struct ToolPolicy {
    /// If non-empty, only these tools are allowed
    pub whitelist: Vec<String>,
    /// These tools are always denied
    pub blacklist: Vec<String>,
}

impl ToolPolicy {
    pub fn is_allowed(&self, tool_name: &str) -> bool {
        if !self.whitelist.is_empty() && !self.whitelist.iter().any(|w| w == tool_name) {
            return false;
        }
        if self.blacklist.iter().any(|b| b == tool_name) {
            return false;
        }
        true
    }
}

pub type ToolPolicyHandle = Arc<RwLock<ToolPolicy>>;

pub fn new_tool_policy_handle() -> ToolPolicyHandle {
    Arc::new(RwLock::new(ToolPolicy::default()))
}

pub use create::{AgentCreateArgs, AgentCreateOutput, AgentCreateTool};
pub use delete::{AgentDeleteArgs, AgentDeleteOutput, AgentDeleteTool};
pub use list::{AgentListArgs, AgentListInfo, AgentListOutput, AgentListTool};
pub use switch::{AgentSwitchArgs, AgentSwitchOutput, AgentSwitchTool};

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn test_tool_policy_empty_allows_all() {
        let policy = ToolPolicy::default();
        assert!(policy.is_allowed("search"));
        assert!(policy.is_allowed("bash"));
    }

    #[test]
    fn test_tool_policy_whitelist() {
        let policy = ToolPolicy {
            whitelist: vec!["search".into(), "web_fetch".into()],
            blacklist: vec![],
        };
        assert!(policy.is_allowed("search"));
        assert!(policy.is_allowed("web_fetch"));
        assert!(!policy.is_allowed("bash"));
    }

    #[test]
    fn test_tool_policy_blacklist() {
        let policy = ToolPolicy {
            whitelist: vec![],
            blacklist: vec!["bash".into()],
        };
        assert!(policy.is_allowed("search"));
        assert!(!policy.is_allowed("bash"));
    }

    #[test]
    fn test_tool_policy_blacklist_overrides_whitelist() {
        let policy = ToolPolicy {
            whitelist: vec!["search".into(), "bash".into()],
            blacklist: vec!["bash".into()],
        };
        assert!(policy.is_allowed("search"));
        assert!(!policy.is_allowed("bash"));
    }
}
