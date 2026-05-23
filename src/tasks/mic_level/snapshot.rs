//! `MicLevelSnapshot` — payload published by the mic-level reporter.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Topic published on the Gateway event bus.
pub const MIC_LEVEL_TOPIC: &str = "host.mic_level.update";

/// Classification of the current mic level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MicLevelState {
    /// Sound above the active threshold — someone is likely speaking or
    /// there's significant ambient noise.
    Active,
    /// Below the active threshold — room is quiet.
    Quiet,
    /// The meter session could not start (no permission, no input device,
    /// platform unsupported, …). `reason` carries the human-readable cause.
    Unavailable,
}

/// One mic-level sample as published on the Gateway event bus.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MicLevelSnapshot {
    /// EMA-smoothed RMS level (0.0..=1.0) when the session is active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
    /// Categorical classification derived from `level` + threshold.
    pub state: MicLevelState,
    /// Reason the meter is unavailable, when `state == Unavailable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Snapshot capture timestamp (UTC).
    pub captured_at: DateTime<Utc>,
}

impl MicLevelSnapshot {
    /// Build a snapshot from a level reading + threshold.
    pub fn from_level(level: f32, active_threshold: f32, captured_at: DateTime<Utc>) -> Self {
        let state = if level >= active_threshold {
            MicLevelState::Active
        } else {
            MicLevelState::Quiet
        };
        Self {
            level: Some(level),
            state,
            reason: None,
            captured_at,
        }
    }

    /// Build an unavailable snapshot with a textual reason.
    pub fn unavailable(reason: impl Into<String>, captured_at: DateTime<Utc>) -> Self {
        Self {
            level: None,
            state: MicLevelState::Unavailable,
            reason: Some(reason.into()),
            captured_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_above_threshold_as_active() {
        let now = Utc::now();
        let s = MicLevelSnapshot::from_level(0.10, 0.05, now);
        assert_eq!(s.state, MicLevelState::Active);
        assert_eq!(s.level, Some(0.10));
        assert!(s.reason.is_none());
    }

    #[test]
    fn classifies_below_threshold_as_quiet() {
        let now = Utc::now();
        let s = MicLevelSnapshot::from_level(0.02, 0.05, now);
        assert_eq!(s.state, MicLevelState::Quiet);
    }

    #[test]
    fn equal_to_threshold_is_active() {
        let now = Utc::now();
        let s = MicLevelSnapshot::from_level(0.05, 0.05, now);
        assert_eq!(s.state, MicLevelState::Active);
    }

    #[test]
    fn unavailable_skips_level_in_json() {
        let now = Utc::now();
        let s = MicLevelSnapshot::unavailable("permission denied", now);
        let v = serde_json::to_value(&s).unwrap();
        assert!(v["level"].is_null() || v.get("level").is_none());
        assert_eq!(v["state"], "unavailable");
        assert_eq!(v["reason"], "permission denied");
    }

    #[test]
    fn active_snapshot_serializes_with_level() {
        let now = Utc::now();
        let s = MicLevelSnapshot::from_level(0.42, 0.05, now);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "active");
        assert!((v["level"].as_f64().unwrap() - 0.42).abs() < 1e-5);
    }
}
