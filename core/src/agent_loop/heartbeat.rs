//! Heartbeat System
//!
//! Configuration and state models for periodic background checks.
//! The heartbeat runner periodically invokes the agent loop in a
//! lightweight mode to check for actionable items.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Frequency at which a heartbeat task should run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatFrequency {
    /// Run every heartbeat interval.
    Every,
    /// Run every N heartbeat intervals.
    EveryN(u32),
    /// Run once per day.
    Daily,
}

impl Default for HeartbeatFrequency {
    fn default() -> Self {
        Self::Every
    }
}

/// A specific task to perform during heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatTask {
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub frequency: HeartbeatFrequency,
    #[serde(default)]
    pub last_run: Option<DateTime<Utc>>,
}

/// Time range for active hours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_hour: u8, // 0-23
    pub end_hour: u8,   // 0-23
}

impl TimeRange {
    pub fn new(start_hour: u8, end_hour: u8) -> Self {
        Self {
            start_hour,
            end_hour,
        }
    }

    /// Check if a given hour falls within this range.
    /// Handles overnight ranges (e.g., 22:00 - 06:00).
    pub fn contains_hour(&self, hour: u8) -> bool {
        if self.start_hour <= self.end_hour {
            hour >= self.start_hour && hour < self.end_hour
        } else {
            // Overnight range
            hour >= self.start_hour || hour < self.end_hour
        }
    }
}

/// Default interval in seconds (30 minutes).
fn default_interval_secs() -> u64 {
    1800
}

/// Heartbeat runner configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// Interval between heartbeats in seconds.
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    /// Active hours (None = always active).
    #[serde(default)]
    pub active_hours: Option<TimeRange>,
    /// Target channel for heartbeat messages.
    #[serde(default)]
    pub target_channel: Option<String>,
    /// Model override for heartbeat (use cheaper model).
    #[serde(default)]
    pub model_override: Option<String>,
    /// Tasks to check during heartbeat.
    #[serde(default)]
    pub tasks: Vec<HeartbeatTask>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_interval_secs(),
            active_hours: None,
            target_channel: None,
            model_override: None,
            tasks: Vec::new(),
        }
    }
}

impl HeartbeatConfig {
    /// Get the interval as a `Duration`.
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_secs)
    }
}

/// Mutable state tracked during heartbeat execution.
#[derive(Debug, Clone, Default)]
pub struct HeartbeatState {
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub last_text: Option<String>,
    pub consecutive_ok_count: u32,
}

impl HeartbeatState {
    /// Check if a heartbeat message is a duplicate of the last one.
    pub fn is_duplicate(&self, text: &str) -> bool {
        self.last_text.as_deref() == Some(text)
    }

    /// Record a successful heartbeat.
    pub fn record_ok(&mut self) {
        self.last_heartbeat = Some(Utc::now());
        self.consecutive_ok_count += 1;
    }

    /// Record a heartbeat with content.
    pub fn record_content(&mut self, text: String) {
        self.last_heartbeat = Some(Utc::now());
        self.last_text = Some(text);
        self.consecutive_ok_count = 0;
    }
}

/// Result of a heartbeat run.
#[derive(Debug, Clone, PartialEq)]
pub enum HeartbeatResult {
    /// Nothing to report.
    NothingToReport,
    /// Something needs user attention.
    Alert(String),
    /// Content to deliver.
    Content(String),
    /// Skipped (not within active hours, duplicate, etc.).
    Skipped(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HeartbeatConfig::default();
        assert_eq!(config.interval(), Duration::from_secs(30 * 60));
        assert!(config.active_hours.is_none());
        assert!(config.tasks.is_empty());
    }

    #[test]
    fn test_time_range_normal() {
        let range = TimeRange::new(9, 17); // 9 AM - 5 PM
        assert!(range.contains_hour(9));
        assert!(range.contains_hour(12));
        assert!(range.contains_hour(16));
        assert!(!range.contains_hour(8));
        assert!(!range.contains_hour(17));
        assert!(!range.contains_hour(23));
    }

    #[test]
    fn test_time_range_overnight() {
        let range = TimeRange::new(22, 6); // 10 PM - 6 AM
        assert!(range.contains_hour(22));
        assert!(range.contains_hour(23));
        assert!(range.contains_hour(0));
        assert!(range.contains_hour(3));
        assert!(range.contains_hour(5));
        assert!(!range.contains_hour(6));
        assert!(!range.contains_hour(12));
        assert!(!range.contains_hour(21));
    }

    #[test]
    fn test_heartbeat_state_default() {
        let state = HeartbeatState::default();
        assert!(state.last_heartbeat.is_none());
        assert!(state.last_text.is_none());
        assert_eq!(state.consecutive_ok_count, 0);
    }

    #[test]
    fn test_heartbeat_state_record_ok() {
        let mut state = HeartbeatState::default();
        state.record_ok();
        assert!(state.last_heartbeat.is_some());
        assert_eq!(state.consecutive_ok_count, 1);
        state.record_ok();
        assert_eq!(state.consecutive_ok_count, 2);
    }

    #[test]
    fn test_heartbeat_state_record_content_resets_ok_count() {
        let mut state = HeartbeatState::default();
        state.record_ok();
        state.record_ok();
        assert_eq!(state.consecutive_ok_count, 2);
        state.record_content("alert".to_string());
        assert_eq!(state.consecutive_ok_count, 0);
        assert_eq!(state.last_text.as_deref(), Some("alert"));
    }

    #[test]
    fn test_is_duplicate() {
        let mut state = HeartbeatState::default();
        assert!(!state.is_duplicate("hello"));
        state.record_content("hello".to_string());
        assert!(state.is_duplicate("hello"));
        assert!(!state.is_duplicate("world"));
    }

    #[test]
    fn test_heartbeat_frequency_default() {
        assert!(matches!(
            HeartbeatFrequency::default(),
            HeartbeatFrequency::Every
        ));
    }
}
