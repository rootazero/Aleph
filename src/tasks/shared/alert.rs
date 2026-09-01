//! Shared failure-alert gate for the tasks subsystem.
//!
//! Both cron and heartbeat need the same three-part decision — "has the streak
//! reached the threshold", "are we still inside the cooldown", "what do we tell
//! the operator" — so the gate lives here and each subsystem passes its own
//! state in. Copying the gate per subsystem is how heartbeat ended up with a
//! `consecutive_errors` counter that drove backoff and nothing else: the
//! monitoring layer could fail forever without anyone hearing about it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tasks::shared::delivery::{DeliveryPayload, DeliveryTargetConfig};

const fn default_alert_after() -> u32 {
    2
}

const fn default_alert_cooldown() -> i64 {
    3_600_000 // 1 hour in ms
}

/// Configuration for alerting on repeated task failures.
///
/// Wire form is the single source of truth for every surface that edits it
/// (Panel form, `cron_manage` tool, `heartbeat_manage` tool, CLI): the field
/// names below are what the RPC handlers read. A client that invents its own
/// spelling produces a payload the server silently ignores.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FailureAlertConfig {
    /// Alert after this many consecutive failures
    #[serde(default = "default_alert_after")]
    pub after: u32,

    /// Cooldown between alerts in milliseconds
    #[serde(default = "default_alert_cooldown")]
    pub cooldown_ms: i64,

    /// Where to send the alert
    pub target: DeliveryTargetConfig,
}

/// The three pieces of task state the alert gate actually reads.
///
/// Lifted out of `CronJob` (where the gate used to live) so heartbeat — whose
/// state type is unrelated — can feed the very same function instead of
/// growing a second copy that drifts.
#[derive(Debug, Clone, Copy)]
pub struct FailureStreak<'a> {
    pub consecutive_errors: u32,
    pub last_alert_at_ms: Option<i64>,
    pub last_error: Option<&'a str>,
}

/// Decide whether a failure alert is due, and render its message.
///
/// `subject` is the caller-rendered noun phrase that opens the message, e.g.
/// `Cron job 'daily' (job-1)`. Keeping it a parameter is what lets the two
/// subsystems share the gate without the gate knowing either domain type.
#[must_use]
pub fn should_send_alert(
    subject: &str,
    streak: FailureStreak<'_>,
    alert: &FailureAlertConfig,
    now_ms: i64,
) -> Option<String> {
    if streak.consecutive_errors < alert.after {
        return None;
    }
    if let Some(last_alert) = streak.last_alert_at_ms {
        if now_ms.saturating_sub(last_alert) < alert.cooldown_ms {
            return None; // Cooldown
        }
    }
    Some(format!(
        "{subject} failed {} times consecutively. Last error: {}",
        streak.consecutive_errors,
        streak.last_error.unwrap_or("unknown")
    ))
}

/// Build the [`DeliveryPayload`] for a failure alert, shared by the cron and
/// heartbeat dispatchers.
///
/// Both subsystems emit the same operator-facing message ("⚠️ {name}: {message}")
/// and previously re-stated it inline — but the metadata key diverged (cron
/// used `job_id`, heartbeat used `task_id`), so the next field addition would
/// silently land in only one site. This helper is the single source of truth.
#[must_use]
pub fn failure_alert_payload(
    source_type: &'static str,
    task_name: &str,
    entity_id: &str,
    message: &str,
) -> DeliveryPayload {
    DeliveryPayload {
        source_type: source_type.to_string(),
        task_name: task_name.to_string(),
        // Empty placeholder required by DeliveryPayload; String::new() has no
        // heap allocation. Failure alerts are operator-targeted, not agent-
        // routed.
        agent_id: String::new(),
        output: format!("⚠️ {task_name}: {message}"),
        channel_id: None,
        metadata: serde_json::json!({
            "entity_id": entity_id,
            "kind": "failure_alert",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(after: u32, cooldown_ms: i64) -> FailureAlertConfig {
        FailureAlertConfig {
            after,
            cooldown_ms,
            target: DeliveryTargetConfig::Webhook {
                url: "https://example.com".to_string(),
                method: None,
                headers: None,
            },
        }
    }

    #[test]
    fn below_threshold_is_silent() {
        let streak = FailureStreak {
            consecutive_errors: 1,
            last_alert_at_ms: None,
            last_error: Some("boom"),
        };
        assert!(should_send_alert("Thing 'x'", streak, &config(2, 1000), 10).is_none());
    }

    #[test]
    fn threshold_reached_renders_subject_and_error() {
        let streak = FailureStreak {
            consecutive_errors: 2,
            last_alert_at_ms: None,
            last_error: Some("connection timeout"),
        };
        let msg = should_send_alert("Thing 'x'", streak, &config(2, 1000), 10).unwrap();
        assert!(msg.starts_with("Thing 'x' failed 2 times consecutively."));
        assert!(msg.contains("connection timeout"));
    }

    #[test]
    fn cooldown_suppresses_then_releases() {
        let cfg = config(1, 1000);
        let streak = |last: Option<i64>| FailureStreak {
            consecutive_errors: 5,
            last_alert_at_ms: last,
            last_error: None,
        };
        assert!(should_send_alert("Thing", streak(Some(500)), &cfg, 1000).is_none());
        assert!(should_send_alert("Thing", streak(Some(500)), &cfg, 1600).is_some());
    }

    /// The defaults are part of the wire contract — a payload that only names
    /// a target must still get a usable threshold and cooldown.
    #[test]
    fn deserializes_with_only_a_target() {
        let cfg: FailureAlertConfig =
            serde_json::from_str(r#"{"target":{"kind":"Webhook","url":"https://x"}}"#).unwrap();
        assert_eq!(cfg.after, 2);
        assert_eq!(cfg.cooldown_ms, 3_600_000);
    }
}
