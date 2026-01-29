//! Sandbox session visibility control.
//!
//! Controls what sessions a sandboxed session can see and interact with.

use serde::{Deserialize, Serialize};

/// Session visibility policy for sandboxed sessions
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionToolsVisibility {
    /// Can see all sessions
    All,
    /// Can only see sessions spawned by self (default)
    #[default]
    Spawned,
    /// Cannot see any other sessions
    None,
}

/// Context for session visibility checks
#[derive(Debug, Clone)]
pub struct VisibilityContext {
    /// Current session's key
    pub requester_key: String,
    /// Is this a sandboxed session?
    pub sandboxed: bool,
    /// Visibility policy
    pub visibility: SessionToolsVisibility,
}

impl VisibilityContext {
    /// Create a new visibility context
    pub fn new(
        requester_key: impl Into<String>,
        sandboxed: bool,
        visibility: SessionToolsVisibility,
    ) -> Self {
        Self {
            requester_key: requester_key.into(),
            sandboxed,
            visibility,
        }
    }

    /// Create a non-sandboxed context (full access)
    pub fn full_access(requester_key: impl Into<String>) -> Self {
        Self {
            requester_key: requester_key.into(),
            sandboxed: false,
            visibility: SessionToolsVisibility::All,
        }
    }

    /// Check if requester can see target session
    pub fn can_see(&self, target_key: &str, spawned_by: Option<&str>) -> bool {
        // Non-sandboxed sessions can see everything
        if !self.sandboxed {
            return true;
        }

        match self.visibility {
            SessionToolsVisibility::All => true,
            SessionToolsVisibility::None => false,
            SessionToolsVisibility::Spawned => {
                // Can always see self
                if target_key == self.requester_key {
                    return true;
                }
                // Can see sessions spawned by self
                spawned_by
                    .map(|s| s == self.requester_key)
                    .unwrap_or(false)
            }
        }
    }

    /// Check if requester can send to target session
    pub fn can_send(&self, target_key: &str, spawned_by: Option<&str>) -> bool {
        self.can_see(target_key, spawned_by)
    }

    /// Check if requester can spawn sub-agents
    pub fn can_spawn(&self) -> bool {
        if !self.sandboxed {
            return true;
        }
        // Subagent sessions cannot spawn further subagents
        !is_subagent_session(&self.requester_key)
    }
}

/// Check if a session key is a subagent session
fn is_subagent_session(key: &str) -> bool {
    key.contains(":subagent:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_sandboxed_sees_all() {
        let ctx = VisibilityContext::full_access("agent:main:main");
        assert!(ctx.can_see("agent:work:main", None));
        assert!(ctx.can_see("agent:main:subagent:task1", None));
    }

    #[test]
    fn test_sandboxed_visibility_all() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::All);
        assert!(ctx.can_see("agent:work:main", None));
    }

    #[test]
    fn test_sandboxed_visibility_none() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::None);
        assert!(!ctx.can_see("agent:work:main", None));
        assert!(!ctx.can_see("agent:main:main", None)); // Even self is hidden
    }

    #[test]
    fn test_sandboxed_visibility_spawned_sees_self() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::Spawned);
        assert!(ctx.can_see("agent:main:main", None));
    }

    #[test]
    fn test_sandboxed_visibility_spawned_sees_children() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::Spawned);
        // Can see session spawned by self
        assert!(ctx.can_see(
            "agent:main:subagent:task1",
            Some("agent:main:main")
        ));
        // Cannot see session spawned by others
        assert!(!ctx.can_see(
            "agent:work:subagent:task2",
            Some("agent:work:main")
        ));
        // Cannot see session with no spawner
        assert!(!ctx.can_see("agent:work:main", None));
    }

    #[test]
    fn test_can_send_follows_can_see() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::Spawned);
        assert!(ctx.can_send("agent:main:main", None));
        assert!(!ctx.can_send("agent:work:main", None));
    }

    #[test]
    fn test_non_sandboxed_can_spawn() {
        let ctx = VisibilityContext::full_access("agent:main:main");
        assert!(ctx.can_spawn());
    }

    #[test]
    fn test_sandboxed_main_can_spawn() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::Spawned);
        assert!(ctx.can_spawn());
    }

    #[test]
    fn test_subagent_cannot_spawn() {
        let ctx = VisibilityContext::new(
            "agent:main:subagent:task1",
            true,
            SessionToolsVisibility::Spawned,
        );
        assert!(!ctx.can_spawn());
    }

    #[test]
    fn test_is_subagent_session() {
        assert!(is_subagent_session("agent:main:subagent:task1"));
        assert!(is_subagent_session("agent:main:main:subagent:nested"));
        assert!(!is_subagent_session("agent:main:main"));
        assert!(!is_subagent_session("agent:work:peer:user123"));
    }
}
