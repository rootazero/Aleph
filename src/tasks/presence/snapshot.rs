//! PresenceSnapshot — the periodic broadcast payload.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Topic published on the Gateway event bus.
pub const PRESENCE_TOPIC: &str = "host.presence.update";

/// Idle-state classification derived from `idle_seconds`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IdleState {
    /// Recent input (< 60 s).
    Active,
    /// No input for 60 s – 5 min.
    Idle,
    /// No input for ≥ 5 min.
    Away,
    /// Idle detection is not implemented on this platform.
    Unknown,
}

impl IdleState {
    /// Classify by elapsed idle time. Mirrors openclaw's PresenceReporter thresholds.
    #[must_use]
    pub fn classify(idle_seconds: Option<f64>) -> Self {
        match idle_seconds {
            None => IdleState::Unknown,
            // A non-finite reading (NaN/Inf from a glitched idle probe) compares
            // false against every threshold and would otherwise fall through to
            // the maximally-idle `Away` bucket — report it as `Unknown` instead.
            Some(s) if !s.is_finite() => IdleState::Unknown,
            Some(s) if s < 60.0 => IdleState::Active,
            Some(s) if s < 300.0 => IdleState::Idle,
            Some(_) => IdleState::Away,
        }
    }
}

/// A single presence snapshot — what this host looks like at `captured_at`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PresenceSnapshot {
    /// Reporting host's name (from `SystemInfo::hostname`).
    pub hostname: String,
    /// Logged-in username (from `SystemInfo::username`).
    pub username: String,
    /// Platform string ("macos" / "linux" / "windows").
    pub platform: String,
    /// Seconds since last keyboard/mouse input, when detectable.
    pub idle_seconds: Option<f64>,
    /// Categorical idle state derived from `idle_seconds`.
    pub idle_state: IdleState,
    /// Snapshot capture timestamp (UTC).
    pub captured_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_state_classifies_thresholds() {
        assert_eq!(IdleState::classify(None), IdleState::Unknown);
        assert_eq!(IdleState::classify(Some(0.0)), IdleState::Active);
        assert_eq!(IdleState::classify(Some(59.9)), IdleState::Active);
        assert_eq!(IdleState::classify(Some(60.0)), IdleState::Idle);
        assert_eq!(IdleState::classify(Some(299.9)), IdleState::Idle);
        assert_eq!(IdleState::classify(Some(300.0)), IdleState::Away);
        assert_eq!(IdleState::classify(Some(3_600.0)), IdleState::Away);
    }

    #[test]
    fn idle_state_non_finite_is_unknown() {
        // NaN/Inf compare false against every threshold; must not fall through
        // to the maximally-idle `Away` bucket.
        assert_eq!(IdleState::classify(Some(f64::NAN)), IdleState::Unknown);
        assert_eq!(IdleState::classify(Some(f64::INFINITY)), IdleState::Unknown);
        assert_eq!(
            IdleState::classify(Some(f64::NEG_INFINITY)),
            IdleState::Unknown
        );
    }

    #[test]
    fn snapshot_serializes_with_snake_case_idle_state() {
        let snap = PresenceSnapshot {
            hostname: "host".into(),
            username: "user".into(),
            platform: "macos".into(),
            idle_seconds: Some(120.0),
            idle_state: IdleState::Idle,
            captured_at: Utc::now(),
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["hostname"], "host");
        assert_eq!(v["idle_state"], "idle");
        assert_eq!(v["platform"], "macos");
        assert_eq!(v["idle_seconds"], 120.0);
    }

    #[test]
    fn snapshot_idle_seconds_serializes_null_when_none() {
        let snap = PresenceSnapshot {
            hostname: "h".into(),
            username: "u".into(),
            platform: "linux".into(),
            idle_seconds: None,
            idle_state: IdleState::Unknown,
            captured_at: Utc::now(),
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert!(v["idle_seconds"].is_null());
        assert_eq!(v["idle_state"], "unknown");
    }
}
