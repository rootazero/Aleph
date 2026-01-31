//! Gateway handler for approval bridge events.
//!
//! Manages forwarding approval requests to configured chat channels
//! and handling callback responses from users.

use serde::{Deserialize, Serialize};

use crate::exec::ForwardTarget;

/// Forward mode for approval requests
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
    /// Forward to the session where the command originated
    #[default]
    Session,
    /// Forward to explicit configured targets only
    Targets,
    /// Forward to both session and targets
    Both,
}

/// Parse session key to extract channel and target
///
/// Examples:
/// - "agent:main:telegram:dm:12345" -> ("telegram", "12345")
/// - "agent:main:discord:group:guild123" -> ("discord", "guild123")
/// - "agent:main:main" -> None
pub fn parse_session_target(session_key: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = session_key.split(':').collect();

    // Look for known channel types in session key
    for (i, part) in parts.iter().enumerate() {
        match *part {
            "telegram" | "discord" | "imessage" | "slack" | "webchat" => {
                // Next parts should be type (dm/group) and target
                if i + 2 < parts.len() {
                    return Some((part.to_string(), parts[i + 2].to_string()));
                }
            }
            _ => continue,
        }
    }

    None
}

/// Get forward targets based on session key and configured targets
pub fn get_forward_targets(
    session_key: &str,
    configured_targets: &[ForwardTarget],
    mode: ForwardMode,
) -> Vec<ForwardTarget> {
    let mut targets = Vec::new();

    match mode {
        ForwardMode::Session | ForwardMode::Both => {
            // Parse session key for channel/target
            if let Some((channel, target)) = parse_session_target(session_key) {
                targets.push(ForwardTarget { channel, target });
            }
        }
        ForwardMode::Targets => {}
    }

    match mode {
        ForwardMode::Targets | ForwardMode::Both => {
            targets.extend(configured_targets.iter().cloned());
        }
        ForwardMode::Session => {}
    }

    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_session_target_telegram_dm() {
        let result = parse_session_target("agent:main:telegram:dm:12345");
        assert_eq!(result, Some(("telegram".into(), "12345".into())));
    }

    #[test]
    fn test_parse_session_target_discord_group() {
        let result = parse_session_target("agent:main:discord:group:guild123");
        assert_eq!(result, Some(("discord".into(), "guild123".into())));
    }

    #[test]
    fn test_parse_session_target_imessage() {
        let result = parse_session_target("agent:main:imessage:dm:+1234567890");
        assert_eq!(result, Some(("imessage".into(), "+1234567890".into())));
    }

    #[test]
    fn test_parse_session_target_no_channel() {
        let result = parse_session_target("agent:main:main");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_session_target_short() {
        let result = parse_session_target("agent:telegram");
        assert_eq!(result, None);
    }

    #[test]
    fn test_get_forward_targets_session_mode() {
        let targets = get_forward_targets("agent:main:telegram:dm:12345", &[], ForwardMode::Session);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].channel, "telegram");
        assert_eq!(targets[0].target, "12345");
    }

    #[test]
    fn test_get_forward_targets_targets_mode() {
        let configured = vec![ForwardTarget {
            channel: "telegram".into(),
            target: "admin_chat".into(),
        }];
        let targets = get_forward_targets(
            "agent:main:telegram:dm:12345",
            &configured,
            ForwardMode::Targets,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target, "admin_chat");
    }

    #[test]
    fn test_get_forward_targets_both_mode() {
        let configured = vec![ForwardTarget {
            channel: "telegram".into(),
            target: "admin_chat".into(),
        }];
        let targets = get_forward_targets(
            "agent:main:telegram:dm:12345",
            &configured,
            ForwardMode::Both,
        );
        assert_eq!(targets.len(), 2);
    }
}
