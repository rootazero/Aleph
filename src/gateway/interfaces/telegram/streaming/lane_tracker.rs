use std::collections::HashMap;
use std::time::Instant;

/// Identifies a logical message lane within a single streaming run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaneId {
    Answer,
    Reasoning,
    Draft,
}

/// Per-lane delivery state.
#[derive(Debug)]
pub struct LaneState {
    pub preview_message_id: Option<i64>,
    pub final_message_id: Option<i64>,
    pub is_streaming: bool,
    pub last_update: Instant,
    /// Cumulative text delivered on this lane so far. Deltas are appended here
    /// and the preview is edited with the full accumulated text (an edit with a
    /// bare delta would replace the preview with just the latest fragment). A
    /// throttled edit is not lost: the next delta past the debounce interval (or
    /// `finalize`) flushes whatever has accumulated by then.
    pub accumulated: String,
}

impl LaneState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            preview_message_id: None,
            final_message_id: None,
            is_streaming: false,
            last_update: Instant::now(),
            accumulated: String::new(),
        }
    }
}

impl Default for LaneState {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks message IDs for each lane in a single conversation.
pub struct LaneDeliveryTracker {
    lanes: HashMap<LaneId, LaneState>,
    chat_id: i64,
    thread_id: Option<i64>,
}

impl LaneDeliveryTracker {
    #[must_use]
    pub fn new(chat_id: i64, thread_id: Option<i64>) -> Self {
        let mut lanes = HashMap::new();
        lanes.insert(LaneId::Answer, LaneState::new());
        lanes.insert(LaneId::Reasoning, LaneState::new());
        lanes.insert(LaneId::Draft, LaneState::new());
        Self {
            lanes,
            chat_id,
            thread_id,
        }
    }

    #[must_use]
    pub const fn chat_id(&self) -> i64 {
        self.chat_id
    }

    #[must_use]
    pub const fn thread_id(&self) -> Option<i64> {
        self.thread_id
    }

    #[must_use]
    pub fn get(&self, lane_id: LaneId) -> Option<&LaneState> {
        self.lanes.get(&lane_id)
    }

    pub fn get_mut(&mut self, lane_id: LaneId) -> Option<&mut LaneState> {
        self.lanes.get_mut(&lane_id)
    }

    pub fn set_preview_message_id(&mut self, lane_id: LaneId, message_id: i64) {
        if let Some(state) = self.lanes.get_mut(&lane_id) {
            state.preview_message_id = Some(message_id);
            state.is_streaming = true;
            state.last_update = Instant::now();
        }
    }

    pub fn set_final_message_id(&mut self, lane_id: LaneId, message_id: i64) {
        if let Some(state) = self.lanes.get_mut(&lane_id) {
            state.final_message_id = Some(message_id);
            state.is_streaming = false;
            state.last_update = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lane_state_transitions() {
        let mut tracker = LaneDeliveryTracker::new(123, Some(42));
        assert_eq!(tracker.chat_id(), 123);
        assert_eq!(tracker.thread_id(), Some(42));

        let state = tracker.get(LaneId::Answer).unwrap();
        assert!(state.preview_message_id.is_none());
        assert!(!state.is_streaming);

        tracker.set_preview_message_id(LaneId::Answer, 1001);
        let state = tracker.get(LaneId::Answer).unwrap();
        assert_eq!(state.preview_message_id, Some(1001));
        assert!(state.is_streaming);

        tracker.set_final_message_id(LaneId::Answer, 1001);
        let state = tracker.get(LaneId::Answer).unwrap();
        assert_eq!(state.final_message_id, Some(1001));
        assert!(!state.is_streaming);
    }

    #[test]
    fn test_multiple_lanes_independent() {
        let mut tracker = LaneDeliveryTracker::new(456, None);
        tracker.set_preview_message_id(LaneId::Answer, 100);
        tracker.set_preview_message_id(LaneId::Reasoning, 200);

        assert_eq!(
            tracker.get(LaneId::Answer).unwrap().preview_message_id,
            Some(100)
        );
        assert_eq!(
            tracker.get(LaneId::Reasoning).unwrap().preview_message_id,
            Some(200)
        );
        assert!(tracker
            .get(LaneId::Draft)
            .unwrap()
            .preview_message_id
            .is_none());
    }
}
