//! P9 — Gateway handler probes.
//!
//! Tests CronService operations (list, get, manual_run) through the harness,
//! exercising the same code paths the gateway handlers use.

use alephcore::cron::config::{ScheduleKind, TriggerSource};
use alephcore::cron::service::ops;

use super::harness::CronTestHarness;

/// Add job via harness → list_jobs → verify returned CronJobView has
/// correct id, name, schedule_kind.
#[tokio::test]
async fn service_create_and_list() {
    let h = CronTestHarness::new();
    h.add_every_job("gateway-list-1", 30_000).await;

    let views = h.list_jobs().await;
    assert_eq!(views.len(), 1);

    let v = &views[0];
    assert_eq!(v.id, "gateway-list-1");
    assert_eq!(v.name, "gateway-list-1");
    assert!(
        matches!(v.schedule_kind, ScheduleKind::Every { every_ms: 30_000, .. }),
        "expected Every(30_000), got {:?}",
        v.schedule_kind,
    );
}

/// Add job (not due) → manual_run → verify executed with TriggerSource::Manual.
#[tokio::test]
async fn service_manual_run_executes() {
    let h = CronTestHarness::new();
    // Job scheduled far in the future — won't fire on its own.
    h.add_every_job("manual-gw", 999_999_999).await;

    // Verify it hasn't executed yet.
    h.assert_not_executed("manual-gw");

    // Manually trigger it.
    h.manual_run("manual-gw").await;

    h.assert_executed("manual-gw");
    h.assert_execution_count("manual-gw", 1);

    // Verify the trigger source is Manual.
    let calls = h.executor.calls_for("manual-gw");
    assert_eq!(calls.len(), 1);
    assert!(
        matches!(calls[0].trigger_source, TriggerSource::Manual),
        "expected Manual trigger, got {:?}",
        calls[0].trigger_source,
    );
}

/// Call ops::get_job with a nonexistent ID → verify None.
#[tokio::test]
async fn service_get_nonexistent_returns_none() {
    let h = CronTestHarness::new();
    let store = h.state.store.lock().await;
    let result = ops::get_job(&store, "nonexistent");
    assert!(result.is_none(), "expected None for nonexistent job");
}
