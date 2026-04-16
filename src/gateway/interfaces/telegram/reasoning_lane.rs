use crate::gateway::channel::MessageId;
use crate::gateway::streaming::{StreamAction, StreamingConfig, StreamingController};
use std::time::Duration;

/// Lane for streaming reasoning/thinking content separately from the answer.
pub struct ReasoningLane {
    controller: StreamingController,
}

impl ReasoningLane {
    pub fn new(enabled: bool, debounce_ms: u64, min_initial_chars: usize) -> Self {
        let config = StreamingConfig {
            enabled,
            debounce_interval: Duration::from_millis(debounce_ms),
            min_initial_chars,
        };
        Self {
            controller: StreamingController::new(config),
        }
    }

    pub fn push_chunk(&mut self, text: &str) {
        self.controller.push_chunk(text);
    }

    pub fn poll_action(&mut self) -> StreamAction {
        self.controller.poll_action()
    }

    pub fn record_sent(&mut self, msg_id: MessageId) {
        self.controller.record_sent(msg_id);
    }

    pub fn record_edit(&mut self) {
        self.controller.record_edit();
    }

    pub fn message_id(&self) -> Option<&MessageId> {
        self.controller.message_id()
    }

    pub fn buffer(&self) -> &str {
        self.controller.buffer()
    }

    pub fn reset(&mut self) {
        self.controller.reset();
    }
}
