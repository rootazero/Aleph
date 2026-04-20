use tracing::warn;

use super::types::ApprovalDecision;

const APPROVAL_OPEN_TAG: &str = "<exec-approval>";
const APPROVAL_CLOSE_TAG: &str = "</exec-approval>";

pub fn parse_approval(text: &Option<String>) -> ApprovalDecision {
    let text = match text {
        Some(t) => t,
        None => {
            warn!("parse_approval called with None text, defaulting to ask_user");
            return ApprovalDecision::default();
        }
    };

    let start = match text.find(APPROVAL_OPEN_TAG) {
        Some(idx) => idx + APPROVAL_OPEN_TAG.len(),
        None => {
            warn!("No <exec-approval> tag found in text, defaulting to ask_user");
            return ApprovalDecision::default();
        }
    };

    let end = match text.find(APPROVAL_CLOSE_TAG) {
        Some(idx) => idx,
        None => {
            warn!(
                "Found <exec-approval> tag but no closing </exec-approval> tag, defaulting to ask_user"
            );
            return ApprovalDecision::default();
        }
    };

    if start >= end {
        warn!("Invalid exec-approval tag positions, defaulting to ask_user");
        return ApprovalDecision::default();
    }

    let json_str = &text[start..end];

    match serde_json::from_str::<ApprovalDecision>(json_str) {
        Ok(decision) => decision,
        Err(e) => {
            warn!(
                "Failed to parse exec-approval JSON: {}, raw content: {}, defaulting to ask_user",
                e,
                &json_str[..json_str.len().min(100)]
            );
            ApprovalDecision::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::exec_approval::types::{ApprovalAction, BlockAction};

    #[test]
    fn test_auto_execute_parsing() {
        let text = Some(
            "Some reasoning text <exec-approval>{\"action\":\"auto_execute\",\"reason\":\"safe read\"}</exec-approval> more text".to_string()
        );
        let decision = parse_approval(&text);
        assert!(matches!(decision.action, ApprovalAction::AutoExecute));
        assert_eq!(decision.reason, "safe read");
    }

    #[test]
    fn test_ask_user_parsing() {
        let text = Some(
            "<exec-approval>{\"action\":\"ask_user\",\"reason\":\"uncertain about parameters\"}</exec-approval>".to_string()
        );
        let decision = parse_approval(&text);
        assert!(matches!(decision.action, ApprovalAction::AskUser));
        assert_eq!(decision.reason, "uncertain about parameters");
    }

    #[test]
    fn test_block_notify_parsing() {
        let text = Some(
            "<exec-approval>{\"action\":\"block\",\"block_action\":\"notify\",\"reason\":\"dangerous operation\"}</exec-approval>".to_string()
        );
        let decision = parse_approval(&text);
        assert!(matches!(
            decision.action,
            ApprovalAction::Block {
                action: BlockAction::Notify
            }
        ));
        assert_eq!(decision.reason, "dangerous operation");
    }

    #[test]
    fn test_block_retry_parsing() {
        let text = Some(
            "<exec-approval>{\"action\":\"block\",\"block_action\":\"retry\",\"reason\":\"alternative available\"}</exec-approval>".to_string()
        );
        let decision = parse_approval(&text);
        assert!(matches!(
            decision.action,
            ApprovalAction::Block {
                action: BlockAction::Retry
            }
        ));
        assert_eq!(decision.reason, "alternative available");
    }

    #[test]
    fn test_multiple_tags_uses_first() {
        let text = Some(
            "<exec-approval>{\"action\":\"auto_execute\",\"reason\":\"first\"}</exec-approval> ignore <exec-approval>{\"action\":\"ask_user\",\"reason\":\"second\"}</exec-approval>".to_string()
        );
        let decision = parse_approval(&text);
        assert!(matches!(decision.action, ApprovalAction::AutoExecute));
    }

    #[test]
    fn test_no_tags_defaults_to_ask_user() {
        let text = Some("No approval tags in this text".to_string());
        let decision = parse_approval(&text);
        assert!(matches!(decision.action, ApprovalAction::AskUser));
    }

    #[test]
    fn test_none_text_defaults_to_ask_user() {
        let decision = parse_approval(&None);
        assert!(matches!(decision.action, ApprovalAction::AskUser));
    }

    #[test]
    fn test_empty_reason_accepted() {
        let text = Some(
            "<exec-approval>{\"action\":\"ask_user\",\"reason\":\"\"}</exec-approval>".to_string(),
        );
        let decision = parse_approval(&text);
        assert!(matches!(decision.action, ApprovalAction::AskUser));
        assert_eq!(decision.reason, "");
    }
}
