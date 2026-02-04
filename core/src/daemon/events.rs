//! Daemon Event Types
//!
//! Defines the event type system for the Perception Layer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level event classification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonEvent {
    /// Raw events from watchers (unprocessed)
    Raw(RawEvent),
    /// Derived events after pattern matching
    Derived(DerivedEvent),
    /// System-level control events
    System(SystemEvent),
}

/// Raw events emitted by watchers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RawEvent {
    /// Heartbeat to ensure daemon is alive
    Heartbeat {
        timestamp: DateTime<Utc>,
    },

    /// Time-based event
    TimeEvent {
        timestamp: DateTime<Utc>,
        trigger_type: TimeTrigger,
    },

    /// Process lifecycle event
    ProcessEvent {
        timestamp: DateTime<Utc>,
        pid: u32,
        name: String,
        event_type: ProcessEventType,
    },

    /// Filesystem change event
    FsEvent {
        timestamp: DateTime<Utc>,
        path: PathBuf,
        event_type: FsEventType,
    },

    /// System state change event
    SystemStateEvent {
        timestamp: DateTime<Utc>,
        state_type: SystemStateType,
        old_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
    },
}

/// Time-based triggers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeTrigger {
    /// Cron schedule match
    Cron { expression: String },
    /// Interval tick
    Interval { seconds: u64 },
    /// Specific time reached
    Absolute { time: DateTime<Utc> },
}

/// Process event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessEventType {
    Started,
    Stopped,
    CpuThresholdExceeded { usage: f64 },
    MemoryThresholdExceeded { bytes: u64 },
}

/// Filesystem event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsEventType {
    Created,
    Modified,
    Deleted,
    Renamed { from: PathBuf },
}

/// System state types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemStateType {
    BatteryLevel,
    NetworkStatus,
    DisplaySleep,
    UserActivity,
    SystemLoad,
}

/// Derived events from pattern matching
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DerivedEvent {
    /// Pattern matched event
    PatternMatched {
        timestamp: DateTime<Utc>,
        pattern_id: String,
        raw_events: Vec<RawEvent>,
        metadata: HashMap<String, serde_json::Value>,
    },

    /// Aggregated event over time window
    Aggregated {
        timestamp: DateTime<Utc>,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
        event_count: usize,
        summary: serde_json::Value,
    },
}

/// System control events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemEvent {
    /// Watcher started
    WatcherStarted {
        timestamp: DateTime<Utc>,
        watcher_id: String,
    },

    /// Watcher stopped
    WatcherStopped {
        timestamp: DateTime<Utc>,
        watcher_id: String,
        reason: String,
    },

    /// Configuration reloaded
    ConfigReloaded {
        timestamp: DateTime<Utc>,
    },

    /// System error occurred
    Error {
        timestamp: DateTime<Utc>,
        error: String,
    },
}
