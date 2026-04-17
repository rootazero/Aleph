//! Per-Team Policy Routing
//!
//! Provides fine-grained access control and routing policies per team.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::gateway::channel::UserId;

/// Default value for allow_dm
fn default_allow_dm() -> bool {
    true
}

/// Policy for a specific team
///
/// Controls which users can interact with the bot and whether DMs are allowed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamPolicy {
    /// Team ID this policy applies to
    pub team_id: String,
    /// Whether to allow direct messages from team members (default: true)
    #[serde(default = "default_allow_dm")]
    pub allow_dm: bool,
    /// Explicitly allowed users (empty = allow all except blocked)
    /// If non-empty, only these users are allowed
    #[serde(default)]
    pub allowed_users: Vec<UserId>,
    /// Explicitly blocked users (blocklist - takes precedence over allowed_users)
    #[serde(default)]
    pub blocked_users: Vec<UserId>,
}

impl TeamPolicy {
    /// Create a new default policy for a team
    pub fn new(team_id: String) -> Self {
        Self {
            team_id,
            allow_dm: true,
            allowed_users: Vec::new(),
            blocked_users: Vec::new(),
        }
    }

    /// Check if a user is allowed to interact with the bot under this policy
    ///
    /// Returns true if:
    /// - User is not in the blocklist, AND
    /// - Either allowed_users is empty (allow all) OR user is in allowed_users
    pub fn is_user_allowed(&self, user_id: &UserId) -> bool {
        // Blocklist check - blocked users are never allowed
        if self.blocked_users.contains(user_id) {
            return false;
        }
        // If allowlist is empty, allow everyone not blocked
        if self.allowed_users.is_empty() {
            return true;
        }
        // Otherwise, user must be in the allowlist
        self.allowed_users.contains(user_id)
    }
}

/// Channel config with per-team policy support
///
/// Provides hierarchical policy lookup:
/// - Team-specific policy if available
/// - Falls back to default policy
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    /// Default policy applied to teams without explicit policy
    pub default: TeamPolicy,
    /// Per-team policy overrides
    pub teams: HashMap<String, TeamPolicy>,
}

impl PolicyConfig {
    /// Get the effective policy for a team
    ///
    /// Returns team-specific policy if defined, otherwise falls back to default
    pub fn effective_policy(&self, team_id: &str) -> &TeamPolicy {
        self.teams.get(team_id).unwrap_or(&self.default)
    }

    /// Get mutable reference to team policy, creating if necessary
    pub fn team_policy_mut(&mut self, team_id: &str) -> &mut TeamPolicy {
        if !self.teams.contains_key(team_id) {
            self.teams
                .insert(team_id.to_string(), TeamPolicy::new(team_id.to_string()));
        }
        self.teams.get_mut(team_id).unwrap()
    }
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            default: TeamPolicy::new("default".to_string()),
            teams: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_user_id(id: &str) -> UserId {
        UserId::new(id)
    }

    #[test]
    fn test_team_policy_blocked_user() {
        let policy = TeamPolicy {
            team_id: "team1".to_string(),
            allow_dm: true,
            allowed_users: Vec::new(),
            blocked_users: vec![create_user_id("blocked-user")],
        };

        // Blocked user should not be allowed
        assert!(!policy.is_user_allowed(&create_user_id("blocked-user")));
        // Other users should be allowed (empty allowlist = allow all)
        assert!(policy.is_user_allowed(&create_user_id("other-user")));
    }

    #[test]
    fn test_team_policy_allowlist() {
        let policy = TeamPolicy {
            team_id: "team1".to_string(),
            allow_dm: true,
            allowed_users: vec![create_user_id("allowed-user")],
            blocked_users: Vec::new(),
        };

        // Allowed user should be allowed
        assert!(policy.is_user_allowed(&create_user_id("allowed-user")));
        // Other users should not be allowed (non-empty allowlist)
        assert!(!policy.is_user_allowed(&create_user_id("other-user")));
    }

    #[test]
    fn test_team_policy_blocklist_precedence() {
        // Blocklist should take precedence over allowlist
        let policy = TeamPolicy {
            team_id: "team1".to_string(),
            allow_dm: true,
            allowed_users: vec![create_user_id("user-in-both")],
            blocked_users: vec![create_user_id("user-in-both")],
        };

        // Blocked user should NOT be allowed even if in allowlist
        assert!(!policy.is_user_allowed(&create_user_id("user-in-both")));
    }

    #[test]
    fn test_team_policy_default() {
        let policy = TeamPolicy::new("test-team".to_string());
        assert_eq!(policy.team_id, "test-team");
        assert!(policy.allow_dm);
        assert!(policy.allowed_users.is_empty());
        assert!(policy.blocked_users.is_empty());
        // Empty allowlist should allow everyone
        assert!(policy.is_user_allowed(&create_user_id("anyone")));
    }

    #[test]
    fn test_policy_config_fallback() {
        let config = PolicyConfig::default();

        // Unknown team should fallback to default
        let policy = config.effective_policy("unknown-team");
        assert_eq!(policy.team_id, "default");
    }

    #[test]
    fn test_policy_config_team_specific() {
        let mut config = PolicyConfig::default();
        config.teams.insert(
            "specific-team".to_string(),
            TeamPolicy {
                team_id: "specific-team".to_string(),
                allow_dm: false,
                allowed_users: vec![create_user_id("special-user")],
                blocked_users: Vec::new(),
            },
        );

        // Specific team should use its own policy
        let policy = config.effective_policy("specific-team");
        assert_eq!(policy.team_id, "specific-team");
        assert!(!policy.allow_dm);
        assert!(policy.is_user_allowed(&create_user_id("special-user")));
        assert!(!policy.is_user_allowed(&create_user_id("other-user")));

        // Other teams should fallback to default
        let default_policy = config.effective_policy("other-team");
        assert_eq!(default_policy.team_id, "default");
    }

    #[test]
    fn test_policy_config_team_policy_mut() {
        let mut config = PolicyConfig::default();

        // Should create policy if not exists
        let policy = config.team_policy_mut("new-team");
        assert_eq!(policy.team_id, "new-team");

        // Should return existing policy
        let policy2 = config.team_policy_mut("new-team");
        assert_eq!(policy2.team_id, "new-team");
    }

    #[test]
    fn test_policy_config_default_instance() {
        let config = PolicyConfig::default();
        assert_eq!(config.effective_policy("any-team").team_id, "default");
    }
}
