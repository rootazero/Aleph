//! Heartbeat Configuration
//!
//! Configuration and task definition types for the heartbeat monitoring system.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── HeartbeatConfig ──────────────────────────────────────────────────────────

/// Heartbeat service configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HeartbeatConfig {
    /// Whether the heartbeat service is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// How often to tick and check due heartbeats (seconds)
    #[serde(default = "default_tick_interval")]
    pub tick_interval_secs: u64,

    /// Maximum concurrent heartbeat executions
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,

    /// Per-job execution timeout in seconds
    #[serde(default = "default_job_timeout")]
    pub job_timeout_secs: u64,

    /// Retain heartbeat history for this many days
    #[serde(default = "default_history_retention")]
    pub history_retention_days: u32,

    /// Deduplication configuration
    #[serde(default)]
    pub dedup: DedupConfig,

    /// When `true`, a task with no explicit `failure_alert` but with a
    /// `delivery_config` gets a default alert (after=1, cooldown=1h, first
    /// delivery target) so a monitor that has broken does not fail silently.
    /// Mirrors `CronConfig::notify_on_failure_default`.
    ///
    /// Note the coupling this fallback carries: the alert target is the same
    /// object whose failure is being reported. See
    /// `heartbeat::service::timer` for the log-and-carry fallback that keeps a
    /// dead channel from swallowing the news of its own death.
    #[serde(default = "default_true")]
    pub notify_on_failure_default: bool,
}

const fn default_true() -> bool {
    true
}

const fn default_tick_interval() -> u64 {
    10
}

const fn default_max_concurrent() -> usize {
    3
}

const fn default_job_timeout() -> u64 {
    120
}

const fn default_history_retention() -> u32 {
    30
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            tick_interval_secs: default_tick_interval(),
            max_concurrent: default_max_concurrent(),
            job_timeout_secs: default_job_timeout(),
            history_retention_days: default_history_retention(),
            dedup: DedupConfig::default(),
            notify_on_failure_default: default_true(),
        }
    }
}

// ── DedupConfig ──────────────────────────────────────────────────────────────

/// Deduplication configuration for suppressing repeated notifications
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DedupConfig {
    /// Time window in milliseconds within which duplicate outputs are suppressed
    pub window_ms: u64,
    /// Minimum cosine similarity (0.0–1.0) to consider two outputs as duplicates
    pub similarity_threshold: f32,
    /// Maximum number of historical outputs to compare against
    pub max_history: usize,
}

impl Default for DedupConfig {
    fn default() -> Self {
        Self {
            window_ms: 86_400_000, // 24 hours
            similarity_threshold: 0.85,
            max_history: 10,
        }
    }
}

// ── HeartbeatTask ────────────────────────────────────────────────────────────

/// A heartbeat task definition — a recurring monitoring check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatTask {
    /// Unique task identifier (UUID)
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Agent that handles L2 analysis when triggered
    pub agent_id: String,
    /// Whether this task is active
    pub enabled: bool,
    /// How often to run the probe, in milliseconds
    pub interval_ms: u64,
    /// L1 probe configuration
    pub probe: ProbeConfig,
    /// Optional delivery configuration for results
    pub delivery_config: Option<crate::tasks::shared::delivery::DeliveryConfig>,
    /// Optional failure alerting. Uses the same shared gate as cron
    /// (`tasks::shared::alert`); `HeartbeatConfig::notify_on_failure_default`
    /// synthesizes one from `delivery_config` when this is absent.
    #[serde(default)]
    pub failure_alert: Option<crate::tasks::shared::alert::FailureAlertConfig>,
    /// Per-task dedup configuration (overrides service defaults)
    pub dedup: DedupConfig,
    /// Optional weekly time-window gate. When set, the timer skips ticks
    /// outside the window and advances `next_due_ms` to the next window
    /// start (computed in the task's timezone, defaulting to UTC if absent).
    /// Mirrors openclaw's `src/infra/heartbeat-active-hours.ts`.
    #[serde(default)]
    pub active_hours: Option<crate::tasks::shared::active_hours::ActiveHoursSchedule>,
    /// Runtime state (mutable, persisted separately)
    pub state: HeartbeatState,
    /// Creation timestamp (Unix ms)
    pub created_at: i64,
    /// Last update timestamp (Unix ms)
    pub updated_at: i64,
}

impl HeartbeatTask {
    /// Create a new heartbeat task with defaults
    #[must_use]
    pub fn new(name: String, agent_id: String, interval_ms: u64, probe: ProbeConfig) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or_else(
                |_| {
                    tracing::warn!("system clock before Unix epoch, using 0");
                    0
                },
                |d| d.as_millis() as i64,
            );
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            agent_id,
            enabled: true,
            interval_ms,
            probe,
            delivery_config: None,
            failure_alert: None,
            dedup: DedupConfig::default(),
            active_hours: None,
            state: HeartbeatState::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Convenience accessor for consecutive error count
    #[must_use]
    pub const fn consecutive_errors(&self) -> u32 {
        self.state.consecutive_errors
    }
}

// ── ProbeConfig ──────────────────────────────────────────────────────────────

/// L1 probe configuration — defines what to check and when to trigger
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeConfig {
    /// Tool name to call for the probe (e.g. "`gmail.unread_count`")
    pub tool_name: String,
    /// Optional parameters to pass to the tool
    pub tool_params: Option<serde_json::Value>,
    /// Condition that must be true for L2 analysis to trigger
    pub trigger_condition: TriggerCondition,
}

// ── TriggerCondition ─────────────────────────────────────────────────────────

/// Trigger condition evaluated against the L1 probe output
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum TriggerCondition {
    /// Trigger if the output is non-empty
    NonEmpty,
    /// Trigger if the numeric output is greater than the given value
    GreaterThan(f64),
    /// Trigger if the output contains the given substring
    Contains(String),
    /// Trigger if the output has changed since the last probe
    Changed,
    /// Always trigger (every probe fires L2)
    Always,
}

// ── HeartbeatState ───────────────────────────────────────────────────────────

/// Runtime state for a heartbeat task (persisted in the store)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeartbeatState {
    /// When this task is next due to run (Unix ms)
    pub next_due_ms: Option<i64>,
    /// If currently running, when execution started (Unix ms)
    pub running_at_ms: Option<i64>,
    /// When the last L1 probe ran (Unix ms)
    pub last_probe_at_ms: Option<i64>,
    /// Raw output from the last L1 probe
    pub last_probe_result: Option<String>,
    /// When the last L2 agent analysis ran (Unix ms)
    pub last_l2_at_ms: Option<i64>,
    /// Status of the last L2 run
    pub last_l2_status: Option<crate::tasks::cron::config::RunStatus>,
    /// Number of consecutive errors without a success
    pub consecutive_errors: u32,
    /// Description of the last error
    pub last_error: Option<String>,
    /// When the last failure alert was *sent* (Unix ms) — the cooldown anchor
    /// for `tasks::shared::alert::should_send_alert`.
    #[serde(default)]
    pub last_failure_alert_at_ms: Option<i64>,
    /// An alert whose own delivery failed.
    ///
    /// The alert target is frequently the same channel whose failure is being
    /// reported, so "alert the dead channel that it is dead" is the normal
    /// case, not the edge case. Rather than drop the news, it is parked here
    /// and prepended to the next alert that does get through. Cleared on the
    /// first successful send.
    #[serde(default)]
    pub undelivered_alert: Option<String>,
}

// ── HeartbeatTaskView ────────────────────────────────────────────────────────

/// Read-only view of a `HeartbeatTask` for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatTaskView {
    pub id: String,
    pub name: String,
    pub agent_id: String,
    pub enabled: bool,
    pub interval_ms: u64,
    pub probe: ProbeConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_hours: Option<crate::tasks::shared::active_hours::ActiveHoursSchedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_alert: Option<crate::tasks::shared::alert::FailureAlertConfig>,
    /// Where findings are delivered. Surfaced because it is the switch that
    /// decides whether this monitor can report anything at all — a task with
    /// `None` here runs its probe, spends an L2 turn, and drops the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_config: Option<crate::tasks::shared::delivery::DeliveryConfig>,
    pub state: HeartbeatState,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&HeartbeatTask> for HeartbeatTaskView {
    fn from(task: &HeartbeatTask) -> Self {
        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            agent_id: task.agent_id.clone(),
            enabled: task.enabled,
            interval_ms: task.interval_ms,
            probe: task.probe.clone(),
            active_hours: task.active_hours.clone(),
            failure_alert: task.failure_alert.clone(),
            delivery_config: task.delivery_config.clone(),
            state: task.state.clone(),
            created_at: task.created_at,
            updated_at: task.updated_at,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_condition_serde_roundtrip() {
        let conditions = vec![
            TriggerCondition::NonEmpty,
            TriggerCondition::GreaterThan(5.0),
            TriggerCondition::Contains("error".to_string()),
            TriggerCondition::Changed,
            TriggerCondition::Always,
        ];
        for cond in conditions {
            let json = serde_json::to_string(&cond).unwrap();
            let back: TriggerCondition = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{:?}", cond), format!("{:?}", back));
        }
    }

    #[test]
    fn test_heartbeat_config_defaults() {
        let config = HeartbeatConfig::default();
        assert!(config.enabled);
        assert_eq!(config.tick_interval_secs, 10);
        assert_eq!(config.max_concurrent, 3);
        assert_eq!(config.job_timeout_secs, 120);
    }

    #[test]
    fn test_heartbeat_task_new() {
        let task = HeartbeatTask::new(
            "Test Task".to_string(),
            "main".to_string(),
            300_000,
            ProbeConfig {
                tool_name: "gmail.unread_count".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::GreaterThan(0.0),
            },
        );
        assert!(task.enabled);
        assert_eq!(task.interval_ms, 300_000);
        assert_eq!(task.consecutive_errors(), 0);
    }
}
