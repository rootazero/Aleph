//! Daemon Event Types
//!
//! Defines the event type system for the Perception Layer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

/// Derived events from pattern matching and inference
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DerivedEvent {
    /// User activity type changed
    ActivityChanged {
        timestamp: DateTime<Utc>,
        old_activity: crate::daemon::worldmodel::ActivityType,
        new_activity: crate::daemon::worldmodel::ActivityType,
        confidence: f64,
    },

    /// Programming session started
    ProgrammingSessionStarted {
        timestamp: DateTime<Utc>,
        session_id: String,
        language: Option<String>,
        project_root: Option<PathBuf>,
    },

    /// Programming session ended
    ProgrammingSessionEnded {
        timestamp: DateTime<Utc>,
        session_id: String,
        duration: chrono::Duration,
        language: Option<String>,
    },

    /// System resource pressure changed
    ResourcePressureChanged {
        timestamp: DateTime<Utc>,
        pressure_type: PressureType,
        old_level: PressureLevel,
        new_level: PressureLevel,
    },

    /// Meeting state changed
    MeetingStateChanged {
        timestamp: DateTime<Utc>,
        is_in_meeting: bool,
        participants: Option<usize>,
    },

    /// Idle state changed
    IdleStateChanged {
        timestamp: DateTime<Utc>,
        is_idle: bool,
        idle_duration: Option<chrono::Duration>,
    },

    /// Aggregated event over time window
    Aggregated {
        timestamp: DateTime<Utc>,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
        event_type: String,
        event_count: usize,
        summary: serde_json::Value,
    },
}

/// Resource pressure types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PressureType {
    Cpu,
    Memory,
    Battery,
}

/// Resource pressure levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PressureLevel {
    Normal,
    Warning,
    Critical,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derived_event_serialization() {
        let event = DerivedEvent::IdleStateChanged {
            timestamp: Utc::now(),
            is_idle: true,
            idle_duration: Some(chrono::Duration::seconds(60)),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: DerivedEvent = serde_json::from_str(&json).unwrap();

        match deserialized {
            DerivedEvent::IdleStateChanged { is_idle, .. } => {
                assert!(is_idle);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_pressure_type_serialization() {
        let pressure = PressureType::Cpu;
        let json = serde_json::to_string(&pressure).unwrap();
        let deserialized: PressureType = serde_json::from_str(&json).unwrap();
        assert_eq!(pressure, deserialized);
    }

    #[test]
    fn test_pressure_level_serialization() {
        let level = PressureLevel::Warning;
        let json = serde_json::to_string(&level).unwrap();
        let deserialized: PressureLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(level, deserialized);
    }

    #[test]
    fn test_daemon_event_derived_variant() {
        let event = DaemonEvent::Derived(DerivedEvent::MeetingStateChanged {
            timestamp: Utc::now(),
            is_in_meeting: true,
            participants: Some(5),
        });

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();

        match deserialized {
            DaemonEvent::Derived(DerivedEvent::MeetingStateChanged { is_in_meeting, .. }) => {
                assert!(is_in_meeting);
            }
            _ => panic!("Wrong event type"),
        }
    }
}
