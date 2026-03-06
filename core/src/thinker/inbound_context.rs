//! Inbound context — per-request dynamic context for the prompt pipeline.
//!
//! Captures sender, channel, session, and message metadata that varies
//! per request and is injected into the system prompt at render time.

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Who sent the message.
#[derive(Debug, Clone, Default)]
pub struct SenderInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub is_owner: bool,
}

/// Which channel the message arrived on and its capabilities.
#[derive(Debug, Clone)]
pub struct ChannelContext {
    /// e.g. "telegram", "discord", "cli", "websocket"
    pub kind: String,
    /// e.g. ["inline_buttons", "reactions", "threads"]
    pub capabilities: Vec<String>,
    pub is_group_chat: bool,
    pub is_mentioned: bool,
}

impl Default for ChannelContext {
    fn default() -> Self {
        Self {
            kind: "unknown".to_string(),
            capabilities: Vec::new(),
            is_group_chat: false,
            is_mentioned: false,
        }
    }
}

/// Session-level metadata.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub session_key: String,
    pub active_agent: Option<String>,
}

/// Metadata about the inbound message itself.
#[derive(Debug, Clone, Default)]
pub struct MessageMetadata {
    pub has_attachments: bool,
    pub attachment_types: Vec<String>,
    pub reply_to: Option<String>,
}

/// Aggregated per-request context injected into the prompt pipeline.
#[derive(Debug, Clone, Default)]
pub struct InboundContext {
    pub sender: SenderInfo,
    pub channel: ChannelContext,
    pub session: SessionContext,
    pub message: MessageMetadata,
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

impl InboundContext {
    /// Render all fields into a compact text block suitable for system prompt
    /// injection.
    pub fn format_for_prompt(&self) -> String {
        let mut lines: Vec<String> = Vec::new();

        // Sender
        let sender_name = self
            .sender
            .display_name
            .as_deref()
            .unwrap_or(&self.sender.id);
        let role = if self.sender.is_owner {
            " (owner)"
        } else {
            ""
        };
        lines.push(format!("Sender: {}{}", sender_name, role));

        // Channel
        let mut channel_parts = vec![self.channel.kind.clone()];
        if self.channel.is_group_chat {
            channel_parts.push("group_chat".to_string());
        }
        if self.channel.is_mentioned {
            channel_parts.push("mentioned".to_string());
        }
        lines.push(format!("Channel: {}", channel_parts.join(" | ")));

        // Capabilities
        if !self.channel.capabilities.is_empty() {
            lines.push(format!(
                "Capabilities: {}",
                self.channel.capabilities.join(", ")
            ));
        }

        // Session
        if !self.session.session_key.is_empty() {
            lines.push(format!("Session: {}", self.session.session_key));
        }

        // Active agent
        if let Some(agent) = &self.session.active_agent {
            lines.push(format!("Active Agent: {}", agent));
        }

        // Attachments
        if self.message.has_attachments && !self.message.attachment_types.is_empty() {
            let count = self.message.attachment_types.len();
            // Group by type and show counts
            let summary = format!(
                "{} ({})",
                self.message.attachment_types.join(", "),
                count
            );
            lines.push(format!("Attachments: {}", summary));
        }

        // Reply-to
        if let Some(reply) = &self.message.reply_to {
            lines.push(format!("Reply To: {}", reply));
        }

        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sender_is_not_owner() {
        let sender = SenderInfo::default();
        assert!(!sender.is_owner);
        assert!(sender.display_name.is_none());
        assert!(sender.id.is_empty());
    }

    #[test]
    fn channel_defaults() {
        let ch = ChannelContext::default();
        assert_eq!(ch.kind, "unknown");
        assert!(ch.capabilities.is_empty());
        assert!(!ch.is_group_chat);
        assert!(!ch.is_mentioned);
    }

    #[test]
    fn format_for_prompt_basic() {
        let ctx = InboundContext {
            sender: SenderInfo {
                id: "u123".to_string(),
                display_name: Some("Alice".to_string()),
                is_owner: true,
            },
            channel: ChannelContext {
                kind: "telegram".to_string(),
                capabilities: vec!["reactions".to_string(), "inline_buttons".to_string()],
                is_group_chat: true,
                is_mentioned: true,
            },
            session: SessionContext {
                session_key: "tg:dm:123".to_string(),
                active_agent: Some("default".to_string()),
            },
            message: MessageMetadata::default(),
        };

        let output = ctx.format_for_prompt();
        assert!(output.contains("Sender: Alice (owner)"));
        assert!(output.contains("Channel: telegram | group_chat | mentioned"));
        assert!(output.contains("Capabilities: reactions, inline_buttons"));
        assert!(output.contains("Session: tg:dm:123"));
        assert!(output.contains("Active Agent: default"));
        // No attachments or reply
        assert!(!output.contains("Attachments:"));
        assert!(!output.contains("Reply To:"));
    }

    #[test]
    fn format_for_prompt_with_attachments_and_reply() {
        let ctx = InboundContext {
            sender: SenderInfo {
                id: "u456".to_string(),
                display_name: None,
                is_owner: false,
            },
            channel: ChannelContext {
                kind: "discord".to_string(),
                capabilities: vec![],
                is_group_chat: false,
                is_mentioned: false,
            },
            session: SessionContext {
                session_key: "dc:dm:456".to_string(),
                active_agent: None,
            },
            message: MessageMetadata {
                has_attachments: true,
                attachment_types: vec!["image".to_string()],
                reply_to: Some("msg_789".to_string()),
            },
        };

        let output = ctx.format_for_prompt();
        // Falls back to id when display_name is None
        assert!(output.contains("Sender: u456"));
        // Not owner
        assert!(!output.contains("(owner)"));
        // No capabilities line
        assert!(!output.contains("Capabilities:"));
        // Attachments
        assert!(output.contains("Attachments: image (1)"));
        // Reply
        assert!(output.contains("Reply To: msg_789"));
    }
}
