use crate::cron::config::{CronJob, FailureAlertConfig};

/// Check if a failure alert should be sent. Returns alert message if conditions met.
pub fn should_send_alert(
    job: &CronJob,
    alert_config: &FailureAlertConfig,
    now_ms: i64,
) -> Option<String> {
    if job.state.consecutive_errors < alert_config.after {
        return None;
    }
    if let Some(last_alert) = job.state.last_failure_alert_at_ms {
        if now_ms - last_alert < alert_config.cooldown_ms {
            return None; // Cooldown
        }
    }
    Some(format!(
        "Cron job '{}' ({}) failed {} times consecutively. Last error: {}",
        job.name,
        job.id,
        job.state.consecutive_errors,
        job.state.last_error.as_deref().unwrap_or("unknown")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::{DeliveryTargetConfig, FailureAlertConfig, ScheduleKind};

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
