//! Aleph Protocol Tokens for structured LLM-to-system communication
//!
//! Defines protocol tokens that LLM returns as its ENTIRE response in
//! background/automated scenarios. These are intercepted by the DecisionParser
//! before JSON parsing, enabling minimal-cost responses.

/// Protocol tokens for structured LLM-to-system communication.
///
/// When the LLM returns one of these tokens as its entire response,
/// the system intercepts it and converts to the appropriate Decision
/// variant without requiring JSON parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolToken {
    /// Heartbeat check: nothing to report
    HeartbeatOk,
    /// Background task completed, no user notification needed
    SilentComplete,
    /// No meaningful response to give
    NoReply,
    /// Something requires user attention (with brief description)
    NeedsAttention(String),
}

impl ProtocolToken {
    pub const HEARTBEAT_OK: &'static str = "ALEPH_HEARTBEAT_OK";
    pub const SILENT_COMPLETE: &'static str = "ALEPH_SILENT_COMPLETE";
    pub const NO_REPLY: &'static str = "ALEPH_NO_REPLY";
    pub const NEEDS_ATTENTION_PREFIX: &'static str = "ALEPH_NEEDS_ATTENTION:";

    /// Parse raw LLM output into a protocol token.
    ///
    /// Returns `Some(token)` if the entire (trimmed) response is a valid
    /// protocol token. Returns `None` for normal responses.
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        match trimmed {
            Self::HEARTBEAT_OK => Some(Self::HeartbeatOk),
            Self::SILENT_COMPLETE => Some(Self::SilentComplete),
            Self::NO_REPLY => Some(Self::NoReply),
            s if s.starts_with(Self::NEEDS_ATTENTION_PREFIX) => {
                let msg = s[Self::NEEDS_ATTENTION_PREFIX.len()..].trim().to_string();
                if msg.is_empty() {
                    None
                } else {
                    Some(Self::NeedsAttention(msg))
                }
            }
            _ => None,
        }
    }

    /// Generate the prompt section that teaches LLM about protocol tokens.
    pub fn to_prompt_section() -> String {
        let mut s = String::new();
        s.push_str("## Response Protocol Tokens\n\n");
        s.push_str("When operating in background mode, use these exact tokens as your ENTIRE response:\n\n");
        s.push_str(&format!(
            "- `{}` — Heartbeat check found nothing to report.\n",
            Self::HEARTBEAT_OK
        ));
        s.push_str(&format!(
            "- `{}` — Background task completed successfully, no user notification needed.\n",
            Self::SILENT_COMPLETE
        ));
        s.push_str(&format!(
            "- `{}` — No meaningful response to give.\n",
            Self::NO_REPLY
        ));
        s.push_str(&format!(
            "- `{} <brief description>` — Something requires user attention.\n\n",
            Self::NEEDS_ATTENTION_PREFIX
        ));
        s.push_str("Rules:\n");
        s.push_str("- Token must be the ENTIRE message. Never mix with normal text.\n");
        s.push_str(
            "- Use ALEPH_HEARTBEAT_OK for routine heartbeat checks with no findings.\n",
        );
        s.push_str("- Use ALEPH_NEEDS_ATTENTION only when there is genuinely actionable information.\n\n");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_heartbeat_ok() {
        assert!(matches!(
            ProtocolToken::parse("ALEPH_HEARTBEAT_OK"),
            Some(ProtocolToken::HeartbeatOk)
        ));
    }

    #[test]
    fn test_parse_heartbeat_ok_with_whitespace() {
        assert!(matches!(
            ProtocolToken::parse("  ALEPH_HEARTBEAT_OK  \n"),
            Some(ProtocolToken::HeartbeatOk)
        ));
    }

    #[test]
    fn test_parse_silent_complete() {
        assert!(matches!(
            ProtocolToken::parse("ALEPH_SILENT_COMPLETE"),
            Some(ProtocolToken::SilentComplete)
        ));
    }

    #[test]
    fn test_parse_no_reply() {
        assert!(matches!(
            ProtocolToken::parse("ALEPH_NO_REPLY"),
            Some(ProtocolToken::NoReply)
        ));
    }

    #[test]
    fn test_parse_needs_attention() {
        let result = ProtocolToken::parse("ALEPH_NEEDS_ATTENTION: Database disk usage at 95%");
        match result {
            Some(ProtocolToken::NeedsAttention(msg)) => {
                assert_eq!(msg, "Database disk usage at 95%")
            }
            _ => panic!("Expected NeedsAttention"),
        }
    }

    #[test]
    fn test_parse_needs_attention_empty_message() {
        // Empty message after prefix should return None
        assert!(ProtocolToken::parse("ALEPH_NEEDS_ATTENTION:").is_none());
        assert!(ProtocolToken::parse("ALEPH_NEEDS_ATTENTION:   ").is_none());
    }

    #[test]
    fn test_parse_normal_json_returns_none() {
        assert!(ProtocolToken::parse(
            r#"{"reasoning": "hello", "action": {"type": "complete"}}"#
        )
        .is_none());
    }

    #[test]
    fn test_parse_mixed_content_returns_none() {
        assert!(ProtocolToken::parse("All good. ALEPH_HEARTBEAT_OK").is_none());
    }

    #[test]
    fn test_to_prompt_section_contains_all_tokens() {
        let section = ProtocolToken::to_prompt_section();
        assert!(section.contains("ALEPH_HEARTBEAT_OK"));
        assert!(section.contains("ALEPH_SILENT_COMPLETE"));
        assert!(section.contains("ALEPH_NO_REPLY"));
        assert!(section.contains("ALEPH_NEEDS_ATTENTION"));
        assert!(section.contains("## Response Protocol Tokens"));
    }
}
