use crate::tasks::cron::config::{CronJob, FailureAlertConfig};
use crate::tasks::shared::alert::FailureStreak;

/// Render the noun phrase that opens a cron failure alert.
///
/// Shared with the permanent-failure message built by `phase3_writeback`, so
/// both variants of the alert name the job the same way.
#[must_use]
pub fn alert_subject(job: &CronJob) -> String {
    format!("Cron job '{}' ({})", job.name, job.id)
}

/// Check if a failure alert should be sent. Returns alert message if conditions met.
///
/// Thin adapter over [`crate::tasks::shared::alert::should_send_alert`] — the
/// gate itself is shared with heartbeat; only the projection from `CronJob`
/// lives here.
#[must_use]
pub fn should_send_alert(
    job: &CronJob,
    alert_config: &FailureAlertConfig,
    now_ms: i64,
) -> Option<String> {
    crate::tasks::shared::alert::should_send_alert(
        &alert_subject(job),
        FailureStreak {
            consecutive_errors: job.state.consecutive_errors,
            last_alert_at_ms: job.state.last_failure_alert_at_ms,
            last_error: job.state.last_error.as_deref(),
        },
        alert_config,
        now_ms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::cron::config::{DeliveryTargetConfig, FailureAlertConfig, ScheduleKind};

    fn make_test_job(id: &str) -> CronJob {
        let mut job = CronJob::new(
            id.to_string(),
            "agent".to_string(),
            "prompt".to_string(),
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.id = id.to_string();
        job
    }

    fn make_alert_config() -> FailureAlertConfig {
        FailureAlertConfig {
            after: 2,
            cooldown_ms: 3_600_000,
            target: DeliveryTargetConfig::Webhook {
                url: "https://example.com".to_string(),
                method: None,
                headers: None,
            },
        }
    }

    #[test]
    fn no_alert_below_threshold() {
        let mut job = make_test_job("job-1");
        job.state.consecutive_errors = 1;
        let config = make_alert_config();
        assert!(should_send_alert(&job, &config, 1_000_000).is_none());
    }

    #[test]
    fn alert_at_threshold() {
        let mut job = make_test_job("job-2");
        job.state.consecutive_errors = 2;
        job.state.last_error = Some("connection timeout".to_string());
        let config = make_alert_config();

        let msg = should_send_alert(&job, &config, 1_000_000);
        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert!(msg.contains("job-2"));
        assert!(msg.contains("2 times"));
        assert!(msg.contains("connection timeout"));
    }

    #[test]
    fn alert_respects_cooldown() {
        let mut job = make_test_job("job-3");
        job.state.consecutive_errors = 5;
        job.state.last_error = Some("server error".to_string());
        job.state.last_failure_alert_at_ms = Some(1_000_000);
        let config = make_alert_config();

        // Within cooldown (1h = 3_600_000ms)
        assert!(should_send_alert(&job, &config, 2_000_000).is_none());

        // After cooldown
        let msg = should_send_alert(&job, &config, 5_000_000);
        assert!(msg.is_some());
    }
}
