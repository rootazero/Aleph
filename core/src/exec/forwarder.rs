//! Exec approval forwarder for chat channels.
//!
//! Forwards approval requests to configured chat channels (Telegram, Discord, etc.)
//! and handles approval responses from users.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::manager::{ExecApprovalManager, ExecApprovalRecord};
use super::socket::ApprovalDecisionType;

/// Forwarding mode for approval requests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// Forward target specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardTarget {
    /// Channel type (telegram, discord, imessage, etc.)
    pub channel: String,
    /// Target identifier (chat_id, channel_id, phone, etc.)
    pub target: String,
}

/// Configuration for the approval forwarder
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForwarderConfig {
    /// Forwarding mode
    #[serde(default)]
    pub mode: ForwardMode,

    /// Explicit forward targets
    #[serde(default)]
    pub targets: Vec<ForwardTarget>,

    /// Only forward for specific agents
    #[serde(default)]
    pub agent_filter: Option<Vec<String>>,

    /// Session key pattern filter (regex)
    #[serde(default)]
    pub session_filter: Option<String>,

    /// Message template for approval request
    #[serde(default)]
    pub request_template: Option<String>,

    /// Message template for approval resolved
    #[serde(default)]
    pub resolved_template: Option<String>,
}

/// Formatted approval message for chat channels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalMessage {
    /// Message text
    pub text: String,
    /// Approval ID for reference
    pub approval_id: String,
    /// Whether this is a request or resolution
    pub is_request: bool,
    /// Suggested reply commands
    pub reply_hints: Vec<String>,
}

/// Approval forwarder that sends approval requests to chat channels
pub struct ExecApprovalForwarder {
    config: ForwarderConfig,
    manager: Arc<ExecApprovalManager>,
}

impl ExecApprovalForwarder {
    /// Create a new forwarder
    pub fn new(config: ForwarderConfig, manager: Arc<ExecApprovalManager>) -> Self {
        Self { config, manager }
    }

    /// Check if this approval should be forwarded based on filters
    pub fn should_forward(&self, record: &ExecApprovalRecord) -> bool {
        // Check agent filter
        if let Some(ref agents) = self.config.agent_filter {
            if !agents.contains(&record.agent_id) {
                return false;
            }
        }

        // Check session filter
        if let Some(ref pattern) = self.config.session_filter {
            if let Ok(re) = regex::Regex::new(pattern) {
                if !re.is_match(&record.session_key) {
                    return false;
                }
            }
        }

        true
    }

    /// Format an approval request message
    pub fn format_request(&self, record: &ExecApprovalRecord) -> ApprovalMessage {
        let template = self.config.request_template.as_deref().unwrap_or(
            "🔒 **Exec approval required**\n\n\
             **ID:** `{id}`\n\
             **Command:** `{command}`\n\
             **CWD:** `{cwd}`\n\
             **Agent:** `{agent_id}`\n\
             **Expires in:** {remaining}s\n\n\
             Reply with:\n\
             `/approve {id} allow-once`\n\
             `/approve {id} allow-always`\n\
             `/approve {id} deny`",
        );

        let remaining = record
            .expires_at_ms
            .saturating_sub(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            )
            / 1000;

        let text = template
            .replace("{id}", &record.id)
            .replace("{command}", &record.command)
            .replace("{cwd}", record.cwd.as_deref().unwrap_or("-"))
            .replace("{agent_id}", &record.agent_id)
            .replace("{session_key}", &record.session_key)
            .replace("{executable}", &record.executable)
            .replace(
                "{resolved_path}",
                record.resolved_path.as_deref().unwrap_or("-"),
            )
            .replace("{remaining}", &remaining.to_string());

        ApprovalMessage {
            text,
            approval_id: record.id.clone(),
            is_request: true,
            reply_hints: vec![
                format!("/approve {} allow-once", record.id),
                format!("/approve {} allow-always", record.id),
                format!("/approve {} deny", record.id),
            ],
        }
    }

    /// Format an approval resolved message
    pub fn format_resolved(
        &self,
        record: &ExecApprovalRecord,
        decision: ApprovalDecisionType,
        resolved_by: Option<&str>,
    ) -> ApprovalMessage {
        let template = self.config.resolved_template.as_deref().unwrap_or(
            "{emoji} **Exec approval {decision}**\n\n\
             **ID:** `{id}`\n\
             **Command:** `{command}`\n\
             **Resolved by:** {resolved_by}",
        );

        let (emoji, decision_str) = match decision {
            ApprovalDecisionType::AllowOnce => ("✅", "allowed (once)"),
            ApprovalDecisionType::AllowAlways => ("✅", "allowed (always)"),
            ApprovalDecisionType::Deny => ("❌", "denied"),
        };

        let text = template
            .replace("{emoji}", emoji)
            .replace("{decision}", decision_str)
            .replace("{id}", &record.id)
            .replace("{command}", &record.command)
            .replace("{resolved_by}", resolved_by.unwrap_or("unknown"));

        ApprovalMessage {
            text,
            approval_id: record.id.clone(),
            is_request: false,
            reply_hints: vec![],
        }
    }

    /// Parse an approval command from user input
    ///
    /// Supports formats:
    /// - `/approve <id> allow-once`
    /// - `/approve <id> allow-always`
    /// - `/approve <id> deny`
    pub fn parse_approve_command(input: &str) -> Option<(String, ApprovalDecisionType)> {
        let input = input.trim();

        // Check for /approve prefix
        if !input.starts_with("/approve ") {
            return None;
        }

        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }

        let id = parts[1].to_string();
        let decision = match parts[2] {
            "allow-once" | "allow" | "yes" | "y" => ApprovalDecisionType::AllowOnce,
            "allow-always" | "always" => ApprovalDecisionType::AllowAlways,
            "deny" | "no" | "n" | "reject" => ApprovalDecisionType::Deny,
            _ => return None,
        };

        Some((id, decision))
    }

    /// Handle an incoming message that might be an approval response
    pub fn handle_message(&self, message: &str, sender: &str) -> Option<bool> {
        let (id, decision) = Self::parse_approve_command(message)?;

        let resolved = self.manager.resolve(&id, decision, Some(sender.to_string()));

        if resolved {
            info!(id = %id, ?decision, sender = %sender, "Approval resolved via chat");
        } else {
            warn!(id = %id, sender = %sender, "Approval not found or already resolved");
        }

        Some(resolved)
    }

    /// Get forward targets based on mode and record
    pub fn get_targets(&self, record: &ExecApprovalRecord) -> Vec<ForwardTarget> {
        match self.config.mode {
            ForwardMode::Session => {
                // Parse session key to extract channel info
                // Format: agent:main:telegram:dm:user123
                self.parse_session_target(&record.session_key)
                    .into_iter()
                    .collect()
            }
            ForwardMode::Targets => self.config.targets.clone(),
            ForwardMode::Both => {
                let mut targets = self.config.targets.clone();
                if let Some(session_target) = self.parse_session_target(&record.session_key) {
                    if !targets
                        .iter()
                        .any(|t| t.channel == session_target.channel && t.target == session_target.target)
                    {
                        targets.push(session_target);
                    }
                }
                targets
            }
        }
    }

    /// Parse session key to extract forward target
    fn parse_session_target(&self, session_key: &str) -> Option<ForwardTarget> {
        // Session key format: agent:main:telegram:dm:user123
        let parts: Vec<&str> = session_key.split(':').collect();
        if parts.len() >= 5 {
            let channel = parts[2].to_string();
            let target = parts[4..].join(":");
            Some(ForwardTarget { channel, target })
        } else {
            None
        }
    }
}

/// Event types for the forwarder
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForwarderEvent {
    /// Approval request created
    ApprovalRequested {
        record: ExecApprovalRecord,
        message: ApprovalMessage,
        targets: Vec<ForwardTarget>,
    },
    /// Approval resolved
    ApprovalResolved {
        record: ExecApprovalRecord,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
        message: ApprovalMessage,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_record() -> ExecApprovalRecord {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        ExecApprovalRecord {
            id: "test-123".to_string(),
            command: "npm install".to_string(),
            cwd: Some("/project".to_string()),
            host: None,
            agent_id: "main".to_string(),
            session_key: "agent:main:telegram:dm:user123".to_string(),
            executable: "npm".to_string(),
            resolved_path: Some("/usr/bin/npm".to_string()),
            created_at_ms: now,
            expires_at_ms: now + 120_000,
            resolved_at_ms: None,
            decision: None,
            resolved_by: None,
        }
    }

    #[test]
    fn test_format_request() {
        let forwarder = ExecApprovalForwarder::new(
            ForwarderConfig::default(),
            Arc::new(ExecApprovalManager::new()),
        );

        let record = mock_record();
        let message = forwarder.format_request(&record);

        assert!(message.text.contains("test-123"));
        assert!(message.text.contains("npm install"));
        assert!(message.is_request);
        assert_eq!(message.reply_hints.len(), 3);
    }

    #[test]
    fn test_format_resolved() {
        let forwarder = ExecApprovalForwarder::new(
            ForwarderConfig::default(),
            Arc::new(ExecApprovalManager::new()),
        );

        let record = mock_record();
        let message = forwarder.format_resolved(&record, ApprovalDecisionType::AllowOnce, Some("alice"));

        assert!(message.text.contains("allowed"));
        assert!(message.text.contains("alice"));
        assert!(!message.is_request);
    }

    #[test]
    fn test_parse_approve_command() {
        assert_eq!(
            ExecApprovalForwarder::parse_approve_command("/approve abc123 allow-once"),
            Some(("abc123".to_string(), ApprovalDecisionType::AllowOnce))
        );

        assert_eq!(
            ExecApprovalForwarder::parse_approve_command("/approve abc123 allow-always"),
            Some(("abc123".to_string(), ApprovalDecisionType::AllowAlways))
        );

        assert_eq!(
            ExecApprovalForwarder::parse_approve_command("/approve abc123 deny"),
            Some(("abc123".to_string(), ApprovalDecisionType::Deny))
        );

        assert_eq!(
            ExecApprovalForwarder::parse_approve_command("/approve abc123 yes"),
            Some(("abc123".to_string(), ApprovalDecisionType::AllowOnce))
        );

        assert_eq!(
            ExecApprovalForwarder::parse_approve_command("not a command"),
            None
        );

        assert_eq!(
            ExecApprovalForwarder::parse_approve_command("/approve abc123"),
            None
        );
    }

    #[test]
    fn test_should_forward_no_filter() {
        let forwarder = ExecApprovalForwarder::new(
            ForwarderConfig::default(),
            Arc::new(ExecApprovalManager::new()),
        );

        let record = mock_record();
        assert!(forwarder.should_forward(&record));
    }

    #[test]
    fn test_should_forward_agent_filter() {
        let config = ForwarderConfig {
            agent_filter: Some(vec!["main".to_string()]),
            ..Default::default()
        };
        let forwarder = ExecApprovalForwarder::new(config, Arc::new(ExecApprovalManager::new()));

        let mut record = mock_record();
        assert!(forwarder.should_forward(&record));

        record.agent_id = "other".to_string();
        assert!(!forwarder.should_forward(&record));
    }

    #[test]
    fn test_parse_session_target() {
        let forwarder = ExecApprovalForwarder::new(
            ForwarderConfig::default(),
            Arc::new(ExecApprovalManager::new()),
        );

        let target = forwarder.parse_session_target("agent:main:telegram:dm:user123");
        assert!(target.is_some());
        let t = target.unwrap();
        assert_eq!(t.channel, "telegram");
        assert_eq!(t.target, "user123");

        let target = forwarder.parse_session_target("agent:main:main");
        assert!(target.is_none());
    }

    #[test]
    fn test_get_targets_session_mode() {
        let forwarder = ExecApprovalForwarder::new(
            ForwarderConfig {
                mode: ForwardMode::Session,
                ..Default::default()
            },
            Arc::new(ExecApprovalManager::new()),
        );

        let record = mock_record();
        let targets = forwarder.get_targets(&record);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].channel, "telegram");
        assert_eq!(targets[0].target, "user123");
    }

    #[test]
    fn test_get_targets_explicit_mode() {
        let forwarder = ExecApprovalForwarder::new(
            ForwarderConfig {
                mode: ForwardMode::Targets,
                targets: vec![ForwardTarget {
                    channel: "discord".to_string(),
                    target: "channel123".to_string(),
                }],
                ..Default::default()
            },
            Arc::new(ExecApprovalManager::new()),
        );

        let record = mock_record();
        let targets = forwarder.get_targets(&record);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].channel, "discord");
    }
}
