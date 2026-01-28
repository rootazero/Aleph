//! sessions_list tool implementation.
//!
//! Lists visible sessions with filtering options.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::{SessionKind, SessionListRow};

/// Parameters for sessions_list tool
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SessionsListParams {
    /// Filter by session kinds: main, dm, group, task, subagent, ephemeral
    #[serde(default)]
    pub kinds: Option<Vec<String>>,

    /// Maximum sessions to return (default: 50)
    #[serde(default)]
    pub limit: Option<u32>,

    /// Only sessions active within N minutes
    #[serde(default)]
    pub active_minutes: Option<u32>,

    /// Include last N messages (0-20, default: 0)
    #[serde(default)]
    pub message_limit: Option<u32>,
}

impl SessionsListParams {
    /// Get limit with default
    pub fn get_limit(&self) -> u32 {
        self.limit.unwrap_or(50).min(200)
    }

    /// Get message limit with bounds
    pub fn get_message_limit(&self) -> u32 {
        self.message_limit.unwrap_or(0).min(20)
    }

    /// Parse kinds filter
    pub fn get_kinds(&self) -> Option<Vec<SessionKind>> {
        self.kinds.as_ref().map(|kinds| {
            kinds.iter().filter_map(|s| SessionKind::parse(s)).collect()
        })
    }
}

/// Result of sessions_list tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsListResult {
    /// Total count of matching sessions
    pub count: usize,
    /// Session list
    pub sessions: Vec<SessionListRow>,
}

impl SessionsListResult {
    /// Create an empty result
    pub fn empty() -> Self {
        Self {
            count: 0,
            sessions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_defaults() {
        let params = SessionsListParams::default();
        assert_eq!(params.get_limit(), 50);
        assert_eq!(params.get_message_limit(), 0);
        assert!(params.get_kinds().is_none());
    }

    #[test]
    fn test_params_limit_bounds() {
        let params = SessionsListParams {
            limit: Some(1000),
            ..Default::default()
        };
        assert_eq!(params.get_limit(), 200); // Capped at 200
    }

    #[test]
    fn test_params_message_limit_bounds() {
        let params = SessionsListParams {
            message_limit: Some(100),
            ..Default::default()
        };
        assert_eq!(params.get_message_limit(), 20); // Capped at 20
    }

    #[test]
    fn test_params_kinds_filter() {
        let params = SessionsListParams {
            kinds: Some(vec!["main".to_string(), "dm".to_string(), "invalid".to_string()]),
            ..Default::default()
        };
        let kinds = params.get_kinds().unwrap();
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&SessionKind::Main));
        assert!(kinds.contains(&SessionKind::Dm));
    }
}
