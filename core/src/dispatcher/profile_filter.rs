//! Profile-based Tool Filtering
//!
//! Provides tool filtering based on workspace profiles.
//! This module is used by both Thinker (schema filtering) and Executor (execution validation).
//!
//! # Two-Layer Security
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Layer 1: Thinker (The Lens)                                 │
//! │   - Filters tool schemas before LLM sees them               │
//! │   - Reduces token usage                                     │
//! │   - Prevents LLM from even knowing about disabled tools     │
//! └─────────────────────────────────────────────────────────────┘
//!                           ↓
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Layer 2: Executor (The Gatekeeper)                          │
//! │   - Validates tool calls before execution                   │
//! │   - Defense-in-depth against LLM hallucinations             │
//! │   - Returns PermissionDenied for blocked tools              │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use crate::config::ProfileConfig;
use crate::dispatcher::UnifiedTool;

/// Profile-based tool filter
///
/// Wraps a ProfileConfig and provides convenient filtering methods.
/// If no profile is set, all tools are allowed (permissive default).
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct ProfileFilter {
    profile: Option<ProfileConfig>,
}


impl ProfileFilter {
    /// Create a new profile filter
    pub fn new(profile: Option<ProfileConfig>) -> Self {
        Self { profile }
    }

    /// Create a filter that allows all tools
    pub fn allow_all() -> Self {
        Self { profile: None }
    }

    /// Create a filter from a profile config
    pub fn from_profile(profile: ProfileConfig) -> Self {
        Self {
            profile: Some(profile),
        }
    }

    /// Check if a tool is allowed by this profile
    pub fn is_allowed(&self, tool_name: &str) -> bool {
        match &self.profile {
            None => true,
            Some(profile) => profile.is_tool_allowed(tool_name),
        }
    }

    /// Filter a list of tools, keeping only those allowed by the profile
    pub fn filter_tools(&self, tools: &[UnifiedTool]) -> Vec<UnifiedTool> {
        if self.profile.is_none() {
            return tools.to_vec();
        }

        tools
            .iter()
            .filter(|tool| self.is_allowed(&tool.name))
            .cloned()
            .collect()
    }

    /// Get the profile's tool whitelist patterns
    pub fn whitelist_patterns(&self) -> Option<&[String]> {
        self.profile.as_ref().map(|p| p.tools.as_slice())
    }

    /// Check if this filter has any restrictions
    pub fn has_restrictions(&self) -> bool {
        self.profile.as_ref().is_some_and(|p| !p.tools.is_empty())
    }

    /// Get the bound model from the profile (if any)
    pub fn bound_model(&self) -> Option<&str> {
        self.profile.as_ref().and_then(|p| p.model.as_deref())
    }
}

/// Result of a tool permission check
#[derive(Debug, Clone, PartialEq)]
pub enum ToolPermission {
    /// Tool is allowed
    Allowed,
    /// Tool is blocked by profile whitelist
    BlockedByProfile {
        tool_name: String,
        profile_patterns: Vec<String>,
    },
}

impl ToolPermission {
    /// Check if the tool is allowed
    pub fn is_allowed(&self) -> bool {
        matches!(self, ToolPermission::Allowed)
    }

    /// Get error message if blocked
    pub fn error_message(&self) -> Option<String> {
        match self {
            ToolPermission::Allowed => None,
            ToolPermission::BlockedByProfile {
                tool_name,
                profile_patterns,
            } => Some(format!(
                "Tool '{}' is not allowed in current workspace. Allowed patterns: {:?}",
                tool_name, profile_patterns
            )),
        }
    }
}

impl ProfileFilter {
    /// Check tool permission with detailed result
    pub fn check_permission(&self, tool_name: &str) -> ToolPermission {
        if self.is_allowed(tool_name) {
            ToolPermission::Allowed
        } else {
            ToolPermission::BlockedByProfile {
                tool_name: tool_name.to_string(),
                profile_patterns: self
                    .whitelist_patterns()
                    .map(|p| p.to_vec())
                    .unwrap_or_default(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coding_profile() -> ProfileConfig {
        ProfileConfig {
            tools: vec![
                "git_*".to_string(),
                "fs_*".to_string(),
                "terminal".to_string(),
            ],
            model: Some("claude-sonnet".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_no_profile_allows_all() {
        let filter = ProfileFilter::default();
        assert!(filter.is_allowed("any_tool"));
        assert!(!filter.has_restrictions());
    }

    #[test]
    fn test_coding_profile_whitelist() {
        let filter = ProfileFilter::from_profile(coding_profile());
        assert!(filter.is_allowed("git_commit"));
        assert!(filter.is_allowed("fs_read"));
        assert!(filter.is_allowed("terminal"));
        assert!(!filter.is_allowed("search"));
        assert!(filter.has_restrictions());
    }

    #[test]
    fn test_check_permission() {
        let filter = ProfileFilter::from_profile(coding_profile());
        assert!(filter.check_permission("git_commit").is_allowed());
        assert!(!filter.check_permission("search").is_allowed());
    }
}
