//! Channel-agnostic streaming state machine.
//!
//! `StreamingController` manages buffer accumulation, threshold detection,
//! and debounce timing for real-time message streaming across any channel.
//!
//! ## Flood-control backoff
//!
//! Chat platforms (Telegram, Discord, …) rate-limit message *edits*. When an
//! edit is rejected the controller widens its debounce interval — honoring the
//! channel's `retry_after` hint when present, otherwise doubling — and after a
//! few consecutive rejections suppresses mid-stream edits entirely, falling
//! back to a single final flush. This prevents a throttled endpoint from being
//! hammered by the next chunk (which would otherwise see the debounce already
//! elapsed and retry immediately).

use std::time::{Duration, Instant};

use crate::gateway::channel::MessageId;

/// Hard ceiling for the adaptive edit-debounce interval under flood control.
const MAX_BACKOFF_INTERVAL: Duration = Duration::from_secs(10);

/// Consecutive edit rejections before mid-stream edits are suppressed and the
/// controller falls back to a single final flush.
const MAX_FLOOD_STRIKES: u32 = 3;

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
    /// Current debounce interval, widened by flood-control backoff. Starts at
    /// `config.debounce_interval` and recovers to it on a successful edit.
    effective_debounce: Duration,
    /// Consecutive edit rejections since the last successful edit.
    flood_strikes: u32,
    /// Once set, mid-stream edits are suppressed (final flush still happens via
    /// `finalize()`). Latched for the remainder of the stream to avoid thrashing.
    suppressed: bool,
}

impl StreamingController {
    /// Create a new controller with the given config.
    #[must_use]
    pub fn new(config: StreamingConfig) -> Self {
        let effective_debounce = config.debounce_interval;
        Self {
            buffer: String::new(),
            sent_message_id: None,
            last_edit_at: Instant::now(),
            last_edit_len: 0,
            config,
            effective_debounce,
            flood_strikes: 0,
            suppressed: false,
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
        self.reset_backoff();
    }

    /// Record that a successful edit was performed.
    ///
    /// A clean edit clears any accumulated flood backoff so the stream recovers
    /// its normal cadence once a platform stops throttling.
    pub fn record_edit(&mut self) {
        self.last_edit_at = Instant::now();
        self.last_edit_len = self.buffer.len();
        self.reset_backoff();
    }

    /// Record that an edit was rejected by the channel's rate limiter.
    ///
    /// Widens the debounce interval — honoring `retry_after` when the channel
    /// supplied one, otherwise doubling — capped at [`MAX_BACKOFF_INTERVAL`].
    /// The rejected attempt counts as the most recent edit so the next poll
    /// waits the full (now wider) interval instead of retrying immediately.
    /// After [`MAX_FLOOD_STRIKES`] consecutive rejections, mid-stream edits are
    /// suppressed and the controller falls back to a single final flush.
    pub fn record_edit_throttled(&mut self, retry_after: Option<Duration>) {
        self.flood_strikes = self.flood_strikes.saturating_add(1);
        let widened = match retry_after {
            Some(hint) => hint.max(self.effective_debounce),
            None => self.effective_debounce.saturating_mul(2),
        };
        self.effective_debounce = widened.min(MAX_BACKOFF_INTERVAL);
        self.last_edit_at = Instant::now();
        if self.flood_strikes >= MAX_FLOOD_STRIKES {
            self.suppressed = true;
        }
    }

    /// Whether mid-stream edits are currently suppressed (flood fallback mode).
    #[must_use]
    pub const fn is_suppressed(&self) -> bool {
        self.suppressed
    }

    /// Clear flood-control backoff state back to the configured baseline.
    const fn reset_backoff(&mut self) {
        self.flood_strikes = 0;
        self.effective_debounce = self.config.debounce_interval;
        self.suppressed = false;
    }

    /// Returns the message ID of the sent message, if any.
    #[must_use]
    pub const fn message_id(&self) -> Option<&MessageId> {
        self.sent_message_id.as_ref()
    }

    /// Returns the current buffer contents.
    #[must_use]
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
        } else if self.suppressed {
            // Flood fallback: hold all mid-stream edits; `finalize()` still
            // flushes the complete text once the stream ends.
            StreamAction::Wait
        } else if self.last_edit_at.elapsed() >= self.effective_debounce
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
        self.reset_backoff();
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

    /// A flood rejection without a hint doubles the effective debounce, so an
    /// edit that would otherwise fire immediately is held.
    #[test]
    fn test_throttle_doubles_debounce_no_hint() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action();
        ctrl.record_sent(MessageId::new("1"));
        ctrl.push_chunk("more");
        // Edit rejected, no hint → debounce 300ms doubles to 600ms.
        ctrl.record_edit_throttled(None);
        // 400ms would have been enough for the base 300ms interval...
        ctrl.last_edit_at = Instant::now() - Duration::from_millis(400);
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));
        // ...but 700ms clears the widened 600ms interval.
        ctrl.last_edit_at = Instant::now() - Duration::from_millis(700);
        assert!(matches!(ctrl.poll_action(), StreamAction::Edit(_)));
    }

    /// A `retry_after` hint is honored verbatim (clamped to the cap) when it
    /// exceeds the current interval.
    #[test]
    fn test_throttle_honors_retry_after_hint() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action();
        ctrl.record_sent(MessageId::new("1"));
        ctrl.push_chunk("more");
        ctrl.record_edit_throttled(Some(Duration::from_secs(2)));
        // 1s elapsed < 2s hint → still waiting.
        ctrl.last_edit_at = Instant::now() - Duration::from_millis(1000);
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));
        // 2.1s elapsed → edit fires.
        ctrl.last_edit_at = Instant::now() - Duration::from_millis(2100);
        assert!(matches!(ctrl.poll_action(), StreamAction::Edit(_)));
    }

    /// Backoff never exceeds the hard ceiling, even with a huge hint.
    #[test]
    fn test_throttle_caps_at_max_backoff() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action();
        ctrl.record_sent(MessageId::new("1"));
        ctrl.push_chunk("more");
        ctrl.record_edit_throttled(Some(Duration::from_secs(3600)));
        ctrl.last_edit_at = Instant::now() - (MAX_BACKOFF_INTERVAL + Duration::from_millis(100));
        assert!(matches!(ctrl.poll_action(), StreamAction::Edit(_)));
    }

    /// After MAX_FLOOD_STRIKES rejections, mid-stream edits are suppressed but
    /// the final flush still happens.
    #[test]
    fn test_throttle_falls_back_after_strikes() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action();
        ctrl.record_sent(MessageId::new("1"));
        ctrl.push_chunk("more");
        for _ in 0..MAX_FLOOD_STRIKES {
            ctrl.record_edit_throttled(None);
        }
        assert!(ctrl.is_suppressed());
        // Even with a long-elapsed interval, no mid-stream edit fires.
        ctrl.last_edit_at = Instant::now() - Duration::from_secs(60);
        assert!(matches!(ctrl.poll_action(), StreamAction::Wait));
        // The final flush is unaffected by suppression.
        match ctrl.finalize() {
            StreamAction::EditFinal(text) => assert!(text.ends_with("more")),
            other => panic!("Expected EditFinal, got {:?}", other),
        }
    }

    /// A successful edit clears accumulated backoff (recovery).
    #[test]
    fn test_successful_edit_recovers_from_backoff() {
        let mut ctrl = make_controller(true);
        ctrl.push_chunk(&"x".repeat(35));
        let _ = ctrl.poll_action();
        ctrl.record_sent(MessageId::new("1"));
        ctrl.push_chunk("more");
        ctrl.record_edit_throttled(None); // widen to 600ms, 1 strike
        ctrl.record_edit(); // success → recover
        assert!(!ctrl.is_suppressed());
        ctrl.push_chunk("again");
        // Base 300ms interval is back in force.
        ctrl.last_edit_at = Instant::now() - Duration::from_millis(400);
        assert!(matches!(ctrl.poll_action(), StreamAction::Edit(_)));
    }
}
