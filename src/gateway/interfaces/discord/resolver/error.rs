//! Discord Channel Resolution Errors

use serde::{Deserialize, Serialize};

/// Errors that can occur during channel resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChannelResolutionError {
    /// No channel matched the input.
    NotFound(String),
    /// Multiple matches found for the input.
    Ambiguous(Vec<Candidate>),
    /// Bot lacks permission to access the channel.
    NoPermission(String),
}

/// A channel candidate when resolution is ambiguous.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub channel_id: String,
    pub channel_name: String,
    pub guild_id: String,
    pub guild_name: String,
}

impl std::fmt::Display for ChannelResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(input) => write!(f, "No channel found matching '{}'", input),
            Self::Ambiguous(candidates) => {
                write!(f, "Multiple channels matched: ")?;
                for (i, c) in candidates.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} in {}", c.channel_name, c.guild_name)?;
                }
                Ok(())
            }
            Self::NoPermission(ch) => write!(f, "No permission to access channel: {}", ch),
        }
    }
}

impl std::error::Error for ChannelResolutionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_not_found() {
        let err = ChannelResolutionError::NotFound("general".to_string());
        assert_eq!(err.to_string(), "No channel found matching 'general'");
    }

    #[test]
    fn test_error_display_ambiguous() {
        let candidates = vec![
            Candidate {
                channel_id: "1".to_string(),
                channel_name: "general".to_string(),
                guild_id: "100".to_string(),
                guild_name: "Guild A".to_string(),
            },
            Candidate {
                channel_id: "2".to_string(),
                channel_name: "general".to_string(),
                guild_id: "200".to_string(),
                guild_name: "Guild B".to_string(),
            },
        ];
        let err = ChannelResolutionError::Ambiguous(candidates);
        assert_eq!(
            err.to_string(),
            "Multiple channels matched: general in Guild A, general in Guild B"
        );
    }

    #[test]
    fn test_error_display_no_permission() {
        let err = ChannelResolutionError::NoPermission("channel:123".to_string());
        assert_eq!(
            err.to_string(),
            "No permission to access channel: channel:123"
        );
    }

    #[test]
    fn test_candidate_serde() {
        let candidate = Candidate {
            channel_id: "123".to_string(),
            channel_name: "general".to_string(),
            guild_id: "456".to_string(),
            guild_name: "Test Guild".to_string(),
        };
        let json = serde_json::to_string(&candidate).unwrap();
        let back: Candidate = serde_json::from_str(&json).unwrap();
        assert_eq!(back.channel_id, "123");
    }
}
