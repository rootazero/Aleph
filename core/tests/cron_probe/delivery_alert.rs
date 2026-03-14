//! P5 Delivery & Alert Probes — 6 scenarios covering delivery skip logic
//! and failure alert threshold/cooldown behavior.

use alephcore::cron::alert::should_send_alert;
use alephcore::cron::config::{
    CronJob, DeliveryMode, DeliveryStatus, DeliveryTargetConfig, FailureAlertConfig, ScheduleKind,
};
use alephcore::cron::delivery::should_skip_delivery;

// ── Helpers ─────────────────────────────────────────────────────────────

fn make_test_job(id: &str) -> CronJob {
    let mut job = CronJob::new(
        id,
        "test-agent",
        "test prompt",
        ScheduleKind::Every {
            every_ms: 60_000,
            anchor_ms: None,
        },
    );
    job.id = id.to_string();
    job
}

fn make_alert_config(after: u32, cooldown_ms: i64) -> FailureAlertConfig {
    FailureAlertConfig {
        after,
        cooldown_ms,
        target: DeliveryTargetConfig::Webhook {
            url: "https://example.com/alert".to_string(),
            method: None,
            headers: None,
        },
    }
}

// ── 1. delivery_dedup_agent_sent ────────────────────────────────────────

/// When the agent already sent results, delivery should return AlreadySentByAgent.
#[test]
fn delivery_dedup_agent_sent() {
    let status = should_skip_delivery(true, &DeliveryMode::Primary);
    assert_eq!(
        status,
        DeliveryStatus::AlreadySentByAgent,
        "should return AlreadySentByAgent when agent already sent"
    );
}

// ── 2. delivery_none_mode ───────────────────────────────────────────────

/// When delivery mode is None, should return NotRequested regardless of agent state.
#[test]
fn delivery_none_mode() {
    let status = should_skip_delivery(false, &DeliveryMode::None);
    assert_eq!(
        status,
        DeliveryStatus::NotRequested,
        "DeliveryMode::None should always return NotRequested"
    );
}

// ── 3. delivery_normal_proceeds ─────────────────────────────────────────

/// Normal delivery (agent hasn't sent, mode is Primary) should return Delivered.
#[test]
fn delivery_normal_proceeds() {
    let status = should_skip_delivery(false, &DeliveryMode::Primary);
    assert_eq!(
        status,
        DeliveryStatus::Delivered,
        "normal delivery should return Delivered"
    );
}

// ── 4. alert_fires_after_threshold ──────────────────────────────────────

/// Alert should fire when consecutive_errors >= threshold.
#[test]
fn alert_fires_after_threshold() {
    let mut job = make_test_job("alert-threshold");
    job.state.consecutive_errors = 2;
    job.state.last_error = Some("connection timeout".to_string());

    let config = make_alert_config(2, 3_600_000);
    let now = 1_000_000;

    let msg = should_send_alert(&job, &config, now);
    assert!(msg.is_some(), "alert should fire at threshold");
    let msg = msg.unwrap();
    assert!(
        msg.contains("alert-threshold"),
        "alert message should contain job id"
    );
    assert!(
        msg.contains("2 times"),
        "alert message should mention error count"
    );
    assert!(
        msg.contains("connection timeout"),
        "alert message should contain last error"
    );
}

// ── 5. alert_cooldown_blocks ────────────────────────────────────────────

/// Alert should be suppressed during cooldown period.
#[test]
fn alert_cooldown_blocks() {
    let mut job = make_test_job("alert-cooldown");
    job.state.consecutive_errors = 5;
    job.state.last_error = Some("server error".to_string());

    let cooldown_ms = 3_600_000; // 1 hour
    let config = make_alert_config(2, cooldown_ms);

    // Last alert was 30 minutes ago — within cooldown
    let now = 5_000_000;
    let thirty_min_ms = 30 * 60 * 1_000;
    job.state.last_failure_alert_at_ms = Some(now - thirty_min_ms);

    let msg = should_send_alert(&job, &config, now);
    assert!(
        msg.is_none(),
        "alert should be blocked during cooldown (30min < 1h)"
    );
}

// ── 6. alert_cooldown_expires ───────────────────────────────────────────

/// Alert should fire again after cooldown expires.
#[test]
fn alert_cooldown_expires() {
    let mut job = make_test_job("alert-expired");
    job.state.consecutive_errors = 5;
    job.state.last_error = Some("persistent failure".to_string());

    let cooldown_ms = 3_600_000; // 1 hour
    let config = make_alert_config(2, cooldown_ms);

    // Last alert was >1 hour ago — cooldown expired
    let now = 10_000_000;
    let two_hours_ms = 2 * 3_600_000;
    job.state.last_failure_alert_at_ms = Some(now - two_hours_ms);

    let msg = should_send_alert(&job, &config, now);
    assert!(
        msg.is_some(),
        "alert should fire after cooldown expires (2h > 1h)"
    );
    let msg = msg.unwrap();
    assert!(
        msg.contains("persistent failure"),
        "alert message should contain the last error"
    );
}
