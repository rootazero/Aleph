use crate::providers::message::UnifiedMessage;

pub struct RetryHandler {
    attempts: u8,
}

impl RetryHandler {
    pub fn new() -> Self {
        Self { attempts: 0 }
    }

    pub fn attempts(&self) -> u8 {
        self.attempts
    }

    pub fn should_retry(&self) -> bool {
        self.attempts < 2
    }

    pub fn record_attempt(&mut self) {
        self.attempts += 1;
    }

    pub fn reset(&mut self) {
        self.attempts = 0;
    }

    pub fn handle_retry(
        &mut self,
        messages: &mut Vec<UnifiedMessage>,
        reason: &str,
    ) -> RetryResult {
        if !self.should_retry() {
            return RetryResult::EscalateToNotify;
        }

        self.record_attempt();

        drain_last_assistant_round(messages);

        let system_message = format!(
            "[SYSTEM] Your previous tool call was blocked. Reason: {}. Please generate an alternative approach.",
            reason
        );
        messages.push(UnifiedMessage::user(system_message));

        tracing::info!(
            "Retry attempt {} processed, {} assistant turns in history",
            self.attempts,
            messages
                .iter()
                .filter(|m| matches!(m, UnifiedMessage::Assistant { .. }))
                .count()
        );

        RetryResult::RetryNextTurn
    }
}

impl Default for RetryHandler {
    fn default() -> Self {
        Self::new()
    }
}

pub enum RetryResult {
    RetryNextTurn,
    EscalateToNotify,
}

fn drain_last_assistant_round(messages: &mut Vec<UnifiedMessage>) {
    if messages.is_empty() {
        return;
    }

    let Some(last_assistant_pos) = messages
        .iter()
        .rposition(|m| matches!(m, UnifiedMessage::Assistant { .. }))
    else {
        return;
    };

    messages.drain(last_assistant_pos..);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::ContentBlock;

    fn make_assistant_message(content: &str) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::Text {
                text: content.to_string(),
            }],
        }
    }

    fn make_tool_result(tool_name: &str) -> UnifiedMessage {
        UnifiedMessage::ToolResult {
            tool_call_id: "test-id".to_string(),
            tool_name: tool_name.to_string(),
            content: vec![ContentBlock::Text {
                text: "result".to_string(),
            }],
            is_error: false,
        }
    }

    #[test]
    fn retry_handler_initial_state() {
        let handler = RetryHandler::new();
        assert_eq!(handler.attempts(), 0);
        assert!(handler.should_retry());
    }

    #[test]
    fn retry_handler_allows_two_attempts() {
        let mut handler = RetryHandler::new();
        assert!(handler.should_retry());
        handler.record_attempt();
        assert_eq!(handler.attempts(), 1);
        assert!(handler.should_retry());
        handler.record_attempt();
        assert_eq!(handler.attempts(), 2);
        assert!(!handler.should_retry());
    }

    #[test]
    fn retry_handler_reset() {
        let mut handler = RetryHandler::new();
        handler.record_attempt();
        handler.record_attempt();
        assert!(!handler.should_retry());
        handler.reset();
        assert_eq!(handler.attempts(), 0);
        assert!(handler.should_retry());
    }

    #[test]
    fn drain_last_assistant_round_removes_after_assistant() {
        let mut messages = vec![
            make_assistant_message("Hello"),
            make_tool_result("tool1"),
            make_assistant_message("Another turn"),
            make_tool_result("tool2"),
        ];

        drain_last_assistant_round(&mut messages);

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0], UnifiedMessage::Assistant { .. }));
        assert!(matches!(messages[1], UnifiedMessage::ToolResult { .. }));
    }

    #[test]
    fn drain_last_assistant_round_handles_empty() {
        let mut messages: Vec<UnifiedMessage> = vec![];
        drain_last_assistant_round(&mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn handle_retry_first_attempt() {
        let mut handler = RetryHandler::new();
        let mut messages = vec![make_assistant_message("Hello"), make_tool_result("tool1")];

        let result = handler.handle_retry(&mut messages, "dangerous operation");

        assert!(matches!(result, RetryResult::RetryNextTurn));
        assert_eq!(handler.attempts(), 1);
        // drain removes assistant + tool_result (2), then we push system message (1)
        assert_eq!(messages.len(), 1);
        assert!(matches!(&messages[0], UnifiedMessage::User { .. }));
    }

    #[test]
    fn handle_retry_escalates_after_two_attempts() {
        let mut handler = RetryHandler::new();
        handler.record_attempt();
        handler.record_attempt();
        let mut messages = vec![make_assistant_message("Hello")];

        let result = handler.handle_retry(&mut messages, "still dangerous");

        assert!(matches!(result, RetryResult::EscalateToNotify));
        assert_eq!(handler.attempts(), 2);
    }
}
