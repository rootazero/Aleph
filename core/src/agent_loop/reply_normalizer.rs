//! Reply Normalizer
//!
//! Detects and handles protocol tokens in LLM responses before JSON parsing.
//! Ensures silent replies (heartbeat OK, no-reply, silent complete) are properly
//! intercepted without wasteful processing.

use crate::thinker::protocol_tokens::ProtocolToken;

/// Result of normalizing an LLM response.
#[derive(Debug, Clone, PartialEq)]
pub enum NormalizedReply {
    /// Regular content that should be processed normally.
    Content(String),
    /// Silent response — no user-visible output.
    Silent(SilentReason),
    /// Alert that needs user attention.
    Alert(String),
}

/// Reason for a silent reply.
#[derive(Debug, Clone, PartialEq)]
pub enum SilentReason {
    HeartbeatOk,
    NoReply,
    TaskComplete,
}

/// Normalize an LLM response, detecting protocol tokens.
///
/// Protocol tokens must be the ENTIRE response (after trimming whitespace).
/// Tokens embedded within other text are treated as regular content.
pub fn normalize_reply(raw_response: &str) -> NormalizedReply {
    let trimmed = raw_response.trim();

    // Try parsing as a protocol token first
    if let Some(token) = ProtocolToken::parse(trimmed) {
        return match token {
            ProtocolToken::HeartbeatOk => NormalizedReply::Silent(SilentReason::HeartbeatOk),
            ProtocolToken::SilentComplete => NormalizedReply::Silent(SilentReason::TaskComplete),
            ProtocolToken::NoReply => NormalizedReply::Silent(SilentReason::NoReply),
            ProtocolToken::NeedsAttention(msg) => NormalizedReply::Alert(msg),
        };
    }

    NormalizedReply::Content(raw_response.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heartbeat_ok() {
        assert_eq!(
            normalize_reply("ALEPH_HEARTBEAT_OK"),
            NormalizedReply::Silent(SilentReason::HeartbeatOk)
        );
    }

    #[test]
    fn test_heartbeat_ok_with_whitespace() {
        assert_eq!(
            normalize_reply("  ALEPH_HEARTBEAT_OK  \n"),
            NormalizedReply::Silent(SilentReason::HeartbeatOk)
        );
    }

    #[test]
    fn test_no_reply() {
        assert_eq!(
            normalize_reply("ALEPH_NO_REPLY"),
            NormalizedReply::Silent(SilentReason::NoReply)
        );
    }

    #[test]
    fn test_silent_complete() {
        assert_eq!(
            normalize_reply("ALEPH_SILENT_COMPLETE"),
            NormalizedReply::Silent(SilentReason::TaskComplete)
        );
    }

    #[test]
    fn test_needs_attention() {
        assert_eq!(
            normalize_reply("ALEPH_NEEDS_ATTENTION: Server is down"),
            NormalizedReply::Alert("Server is down".to_string())
        );
    }

    #[test]
    fn test_needs_attention_with_whitespace() {
        assert_eq!(
            normalize_reply("  ALEPH_NEEDS_ATTENTION:   disk full  \n"),
            NormalizedReply::Alert("disk full".to_string())
        );
    }

    #[test]
    fn test_regular_content() {
        let content = r#"{"reasoning": "thinking", "action": {"type": "complete"}}"#;
        assert_eq!(
            normalize_reply(content),
            NormalizedReply::Content(content.to_string())
        );
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(
            normalize_reply(""),
            NormalizedReply::Content("".to_string())
        );
    }

    #[test]
    fn test_partial_token_is_content() {
        assert_eq!(
            normalize_reply("ALEPH_HEART"),
            NormalizedReply::Content("ALEPH_HEART".to_string())
        );
    }

    #[test]
    fn test_token_embedded_in_text_is_content() {
        let content = "I found ALEPH_HEARTBEAT_OK in the logs";
        assert_eq!(
            normalize_reply(content),
            NormalizedReply::Content(content.to_string())
        );
    }
}
