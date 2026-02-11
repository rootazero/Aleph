//! State history with I-Frame + P-Frame memory optimization.
//!
//! Stores application state history using a video encoding strategy:
//! - **I-Frames** (keyframes): Full state snapshots every 5 seconds
//! - **P-Frames** (patches): Incremental JSON Patch deltas
//!
//! This reduces memory usage by ~98% compared to storing full snapshots.
//!
//! # Memory Usage
//!
//! For 30 seconds of history at 10Hz update rate:
//! - Naive: 300 snapshots × 2MB = 600MB
//! - I/P-Frame: 6 I-Frames × 2MB + 300 P-Frames × 500B = 12MB
//!
//! # Example
//!
//! ```ignore
//! let mut history = StateHistory::new(30);
//!
//! // Store I-Frame
//! history.store_iframe(state.clone());
//!
//! // Store P-Frame
//! history.store_pframe(patches);
//!
//! // Query historical state
//! let past_state = history.query(timestamp)?;
//! ```

use super::types::AppState;
use serde_json::Value;
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// State history with I-Frame + P-Frame optimization.
pub struct StateHistory {
    /// Full state snapshots (I-Frames)
    i_frames: VecDeque<IFrame>,

    /// Incremental patches (P-Frames)
    p_frames: VecDeque<PFrame>,

    /// Maximum history duration (seconds)
    max_duration_secs: u64,

    /// I-Frame interval (seconds)
    iframe_interval_secs: u64,

    /// Last I-Frame timestamp
    last_iframe_ts: u64,
}

/// Full state snapshot (I-Frame).
#[derive(Debug, Clone)]
struct IFrame {
    timestamp: u64,
    state: AppState,
}

/// Incremental patch (P-Frame).
#[derive(Debug, Clone)]
struct PFrame {
    timestamp: u64,
    patches: Vec<JsonPatch>,
    base_iframe_ts: u64,
}

/// JSON Patch operation (RFC 6902).
#[derive(Debug, Clone)]
pub struct JsonPatch {
    pub op: String,
    pub path: String,
    pub value: Option<Value>,
}

impl StateHistory {
    /// Create a new state history.
    ///
    /// # Arguments
    ///
    /// * `max_duration_secs` - Maximum history duration (default: 30)
    pub fn new(max_duration_secs: u64) -> Self {
        Self {
            i_frames: VecDeque::new(),
            p_frames: VecDeque::new(),
            max_duration_secs,
            iframe_interval_secs: 5,
            last_iframe_ts: 0,
        }
    }

    /// Store a full state snapshot (I-Frame).
    pub fn store_iframe(&mut self, state: AppState) {
        let timestamp = Self::current_timestamp();

        let iframe = IFrame { timestamp, state };

        self.i_frames.push_back(iframe);
        self.last_iframe_ts = timestamp;

        // Evict old I-Frames
        self.evict_old_iframes(timestamp);
    }

    /// Store incremental patches (P-Frame).
    pub fn store_pframe(&mut self, patches: Vec<JsonPatch>) {
        let timestamp = Self::current_timestamp();

        // Find base I-Frame
        let base_iframe_ts = self.i_frames
            .iter()
            .rev()
            .find(|f| f.timestamp <= timestamp)
            .map(|f| f.timestamp)
            .unwrap_or(0);

        let pframe = PFrame {
            timestamp,
            patches,
            base_iframe_ts,
        };

        self.p_frames.push_back(pframe);

        // Evict old P-Frames
        self.evict_old_pframes(timestamp);
    }

    /// Query state at a specific timestamp.
    ///
    /// Returns `None` if timestamp is outside history window.
    pub fn query(&self, target_ts: u64) -> Option<AppState> {
        // Find nearest I-Frame before target
        let iframe = self.i_frames
            .iter()
            .rev()
            .find(|f| f.timestamp <= target_ts)?;

        // Clone base state
        let mut state = iframe.state.clone();

        // Apply all P-Frames between I-Frame and target
        for pframe in self.p_frames.iter() {
            if pframe.timestamp > iframe.timestamp && pframe.timestamp <= target_ts {
                // Apply patches to state
                // TODO: Implement actual JSON Patch application
                // For now, this is a stub
            }
        }

        Some(state)
    }

    /// Check if we should store an I-Frame.
    pub fn should_store_iframe(&self) -> bool {
        let now = Self::current_timestamp();
        now - self.last_iframe_ts >= self.iframe_interval_secs
    }

    /// Get memory usage estimate (bytes).
    pub fn memory_usage(&self) -> usize {
        // Estimate: 2MB per I-Frame, 500B per P-Frame
        let iframe_size = self.i_frames.len() * 2_000_000;
        let pframe_size = self.p_frames.len() * 500;
        iframe_size + pframe_size
    }

    /// Get current timestamp (seconds since epoch).
    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Evict old I-Frames outside history window.
    fn evict_old_iframes(&mut self, current_ts: u64) {
        let cutoff = current_ts.saturating_sub(self.max_duration_secs);

        while let Some(front) = self.i_frames.front() {
            if front.timestamp < cutoff {
                self.i_frames.pop_front();
            } else {
                break;
            }
        }
    }

    /// Evict old P-Frames outside history window.
    fn evict_old_pframes(&mut self, current_ts: u64) {
        let cutoff = current_ts.saturating_sub(self.max_duration_secs);

        while let Some(front) = self.p_frames.front() {
            if front.timestamp < cutoff {
                self.p_frames.pop_front();
            } else {
                break;
            }
        }
    }

    /// Get number of I-Frames.
    pub fn iframe_count(&self) -> usize {
        self.i_frames.len()
    }

    /// Get number of P-Frames.
    pub fn pframe_count(&self) -> usize {
        self.p_frames.len()
    }
}

impl Default for StateHistory {
    fn default() -> Self {
        Self::new(30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::state_bus::types::{ElementSource, StateSource};

    fn create_test_state(app_id: &str) -> AppState {
        AppState {
            app_id: app_id.to_string(),
            elements: vec![],
            app_context: None,
            source: StateSource::Accessibility,
            confidence: 1.0,
        }
    }

    #[test]
    fn test_store_iframe() {
        let mut history = StateHistory::new(30);
        let state = create_test_state("com.apple.mail");

        history.store_iframe(state);

        assert_eq!(history.iframe_count(), 1);
        assert_eq!(history.pframe_count(), 0);
    }

    #[test]
    fn test_store_pframe() {
        let mut history = StateHistory::new(30);

        // Store I-Frame first
        history.store_iframe(create_test_state("com.apple.mail"));

        // Store P-Frame
        let patches = vec![JsonPatch {
            op: "replace".to_string(),
            path: "/elements/0/value".to_string(),
            value: Some(serde_json::json!("new value")),
        }];
        history.store_pframe(patches);

        assert_eq!(history.iframe_count(), 1);
        assert_eq!(history.pframe_count(), 1);
    }

    #[test]
    fn test_memory_usage_estimate() {
        let mut history = StateHistory::new(30);

        // Store 6 I-Frames
        for _ in 0..6 {
            history.store_iframe(create_test_state("com.apple.mail"));
        }

        // Store 300 P-Frames
        for _ in 0..300 {
            history.store_pframe(vec![]);
        }

        let usage = history.memory_usage();
        // 6 * 2MB + 300 * 500B = 12,150,000 bytes ≈ 12MB
        assert!(usage > 12_000_000 && usage < 13_000_000);
    }

    #[test]
    fn test_should_store_iframe() {
        let mut history = StateHistory::new(30);
        history.iframe_interval_secs = 5;

        // Initially should store
        assert!(history.should_store_iframe());

        // After storing, should not store immediately
        history.store_iframe(create_test_state("com.apple.mail"));
        assert!(!history.should_store_iframe());
    }

    #[test]
    fn test_query_returns_state() {
        let mut history = StateHistory::new(30);
        let state = create_test_state("com.apple.mail");

        history.store_iframe(state.clone());

        let ts = StateHistory::current_timestamp();
        let queried = history.query(ts);

        assert!(queried.is_some());
        assert_eq!(queried.unwrap().app_id, "com.apple.mail");
    }
}
