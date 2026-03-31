//! Channel-agnostic streaming state machine.
//!
//! `StreamingController` manages buffer accumulation, threshold detection,
//! and debounce timing for real-time message streaming across any channel.

use std::time::{Duration, Instant};

use crate::gateway::channel::MessageId;

/// Configuration for streaming behavior.
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Minimum characters before sending the initial message.
    pub min_initial_chars: usize,
    /// Minimum interval between edits (debounce).
    pub debounce_interval: Duration,
    /// Whether streaming edits are enabled.
    pub enabled: bool,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            min_initial_chars: 30,
            debounce_interval: Duration::from_millis(300),
            enabled: false,
        }
    }
}

/// Action the caller should take after polling the controller.
#[derive(Debug)]
pub enum StreamAction {
    /// Not enough content or debounce not elapsed — do nothing.
    Wait,
    /// Buffer crossed threshold — send the initial message.
    SendInitial(String),
    /// Debounce elapsed with new content — edit the existing message.
    Edit(String),
    /// Stream ended, no prior send — send the full buffer as a new message.
    SendFinal(String),
    /// Stream ended with unsent edits — perform a final edit.
    EditFinal(String),
    /// Stream ended, nothing to do (empty buffer or already up-to-date).
    Done,
}

/// Pure-logic streaming state machine.
///
/// Tracks buffered text, whether an initial message has been sent,
/// and debounce timing for subsequent edits. The caller is responsible
/// for actually sending/editing messages and calling `record_sent` /
/// `record_edit` to advance the state.
pub struct StreamingController {
    buffer: String,
    sent_message_id: Option<MessageId>,
    /// Exposed as `pub(crate)` so tests can manipulate timing.
    pub(crate) last_edit_at: Instant,
    last_edit_len: usize,
    config: StreamingConfig,
}

impl StreamingController {
    /// Create a new controller with the given config.
    pub fn new(config: StreamingConfig) -> Self {
        Self {
            buffer: String::new(),
            sent_message_id: None,
            last_edit_at: Instant::now(),
            last_edit_len: 0,
            config,
        }
    }

    /// Append a token/chunk to the internal buffer.
    pub fn push_chunk(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    /// Record that the initial message was sent, storing its ID.
    pub fn record_sent(&mut self, msg_id: MessageId) {
        self.sent_message_id = Some(msg_id);
        self.last_edit_at = Instant::now();
        self.last_edit_len = self.buffer.len();
    }

    /// Record that a successful edit was performed.
    pub fn record_edit(&mut self) {
        self.last_edit_at = Instant::now();
        self.last_edit_len = self.buffer.len();
    }

    /// Returns the message ID of the sent message, if any.
    pub fn message_id(&self) -> Option<&MessageId> {
        self.sent_message_id.as_ref()
    }

    /// Returns the current buffer contents.
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Poll the controller to determine what action to take.
    ///
    /// Call this periodically (e.g. after each chunk) to decide whether
    /// to send, edit, or wait.
    pub fn poll_action(&mut self) -> StreamAction {
        if !self.config.enabled {
            return StreamAction::Wait;
        }

        if self.sent_message_id.is_none() {
            if self.buffer.chars().count() >= self.config.min_initial_chars {
                StreamAction::SendInitial(self.buffer.clone())
            } else {
                StreamAction::Wait
            }
        } else if self.last_edit_at.elapsed() >= self.config.debounce_interval
            && self.buffer.len() > self.last_edit_len
        {
            StreamAction::Edit(self.buffer.clone())
        } else {
            StreamAction::Wait
        }
    }

    /// Reset the controller for a new streaming iteration (e.g. after an
    /// intermediate boundary).  The current streamed message stays as-is;
    /// subsequent chunks will start a fresh message.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.sent_message_id = None;
        self.last_edit_at = Instant::now();
        self.last_edit_len = 0;
    }

    /// Finalize the stream — returns the appropriate action for any
    /// remaining unsent content.
    pub fn finalize(&mut self) -> StreamAction {
        if self.buffer.is_empty() {
            return StreamAction::Done;
        }

        if self.sent_message_id.is_none() {
            StreamAction::SendFinal(self.buffer.clone())
        } else if self.buffer.len() > self.last_edit_len {
            StreamAction::EditFinal(self.buffer.clone())
        } else {
            StreamAction::Done
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_controller(enabled: bool) -> StreamingController {
        StreamingController::new(StreamingConfig {
            min_initial_chars: 30,
            debounce_interval: Duration::from_millis(300),
            enabled,
        })
    }

    #[test]
    fn test_wait_until_threshold() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk("Hi");
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));
    }

    #[test]
    fn test_send_initial_at_threshold() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        match ctrl.poll_action() {
            StreamAction::SendInitial(text) => assert_eq!(text.len(), 35),
            other => panic!("Expected SendInitial, got {:?}", other),
        }
    }

    #[test]
    fn test_edit_after_debounce() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action(); // SendInitial
        ctrl.record_sent(MessageId::new("1"));
        ctrl.push_chunk("more");
        // Before debounce — should wait
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));
        // Simulate debounce elapsed
        ctrl.last_edit_at = Instant::now() - Duration::from_millis(400);
        match ctrl.poll_action() {
            StreamAction::Edit(text) => assert!(text.contains("more")),
            other => panic!("Expected Edit, got {:?}", other),
        }
    }

    #[test]
    fn test_finalize_without_send() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk("short");
        match ctrl.finalize() {
            StreamAction::SendFinal(text) => assert_eq!(text, "short"),
            other => panic!("Expected SendFinal, got {:?}", other),
        }
    }

    #[test]
    fn test_finalize_with_pending_edit() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action();
        ctrl.record_sent(MessageId::new("1"));
        ctrl.push_chunk("tail");
        match ctrl.finalize() {
            StreamAction::EditFinal(text) => assert!(text.ends_with("tail")),
            other => panic!("Expected EditFinal, got {:?}", other),
        }
    }

    #[test]
    fn test_disabled_mode() {
        let mut ctrl = make_controller(false);
        ctrl.push_chunk(&"x".repeat(100));
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));
        match ctrl.finalize() {
            StreamAction::SendFinal(text) => assert_eq!(text.len(), 100),
            other => panic!("Expected SendFinal, got {:?}", other),
        }
    }

    #[test]
    fn test_finalize_empty_buffer() {
        let mut ctrl = make_controller(true);
        assert!(matches!(ctrl.finalize(), StreamAction::Done));
    }

    #[test]
    fn test_finalize_no_pending_edit() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action();
        ctrl.record_sent(MessageId::new("1"));
        // No new content since record_sent
        assert!(matches!(ctrl.finalize(), StreamAction::Done));
    }
}
