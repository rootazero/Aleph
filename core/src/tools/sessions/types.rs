//! Shared types for session tools.

use serde::{Deserialize, Serialize};

/// Session kind for filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Main,
    Dm,
    Group,
    Task,
    Subagent,
    Ephemeral,
}

impl SessionKind {
    /// Parse from string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "main" => Some(Self::Main),
            "dm" | "direct" | "directmessage" => Some(Self::Dm),
            "group" | "channel" => Some(Self::Group),
            "task" | "cron" | "webhook" | "scheduled" => Some(Self::Task),
            "subagent" | "sub" => Some(Self::Subagent),
            "ephemeral" => Some(Self::Ephemeral),
            _ => None,
        }
    }

    /// Get kind from session key string
    pub fn from_session_key(key: &str) -> Self {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() < 3 {
            return Self::Main;
        }

        match parts.get(2..) {
            Some(["main"]) | Some([]) => Self::Main,
            Some(["dm", ..]) => Self::Dm,
            Some([_, "dm", ..]) => Self::Dm,
            Some([_, "group", ..]) | Some([_, "channel", ..]) => Self::Group,
            Some(["cron", ..]) | Some(["webhook", ..]) | Some(["scheduled", ..]) => Self::Task,
            Some(["subagent", ..]) | Some([_, "subagent", ..]) => Self::Subagent,
            Some(["ephemeral", ..]) => Self::Ephemeral,
            _ => Self::Main,
        }
    }
}

/// A message in a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<i64>,
}

/// Session list row for sessions_list result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListRow {
    pub key: String,
    pub kind: SessionKind,
    pub agent_id: String,
    pub channel: Option<String>,
    pub label: Option<String>,
    pub updated_at: Option<i64>,
    pub model: Option<String>,
    pub messages: Option<Vec<SessionMessage>>,
    pub spawned_by: Option<String>,
}

/// Send message status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SendStatus {
    /// Message sent and reply received
    Ok,
    /// Message accepted (fire-and-forget)
    Accepted,
    /// Timeout waiting for reply
    Timeout,
    /// Permission denied
    Forbidden,
    /// Error occurred
    #[default]
    Error,
}

/// Spawn status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SpawnStatus {
    /// Spawn accepted
    Accepted,
    /// Permission denied
    Forbidden,
    /// Error occurred
    #[default]
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_kind_parse() {
        assert_eq!(SessionKind::parse("main"), Some(SessionKind::Main));
        assert_eq!(SessionKind::parse("dm"), Some(SessionKind::Dm));
        assert_eq!(SessionKind::parse("direct"), Some(SessionKind::Dm));
        assert_eq!(SessionKind::parse("group"), Some(SessionKind::Group));
        assert_eq!(SessionKind::parse("cron"), Some(SessionKind::Task));
        assert_eq!(SessionKind::parse("subagent"), Some(SessionKind::Subagent));
        assert_eq!(SessionKind::parse("invalid"), None);
    }

    #[test]
    fn test_session_kind_from_key() {
        assert_eq!(SessionKind::from_session_key("agent:main:main"), SessionKind::Main);
        assert_eq!(SessionKind::from_session_key("agent:main:dm:user1"), SessionKind::Dm);
        assert_eq!(SessionKind::from_session_key("agent:main:telegram:dm:user1"), SessionKind::Dm);
        assert_eq!(SessionKind::from_session_key("agent:main:discord:group:guild1"), SessionKind::Group);
        assert_eq!(SessionKind::from_session_key("agent:main:cron:daily"), SessionKind::Task);
        assert_eq!(SessionKind::from_session_key("agent:main:subagent:task1"), SessionKind::Subagent);
        assert_eq!(SessionKind::from_session_key("agent:main:ephemeral:uuid"), SessionKind::Ephemeral);
    }
}
