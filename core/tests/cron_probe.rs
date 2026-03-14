//! Cron probe integration tests.
//!
//! Tests the cron subsystem end-to-end using a FakeClock and MockExecutor.

mod cron_probe {
    pub mod harness;
    pub mod mock_executor;
}

use cron_probe::harness::CronTestHarness;

#[tokio::test]
async fn harness_smoke_test() {
    let h = CronTestHarness::new();
    h.add_every_job("test", 60_000).await;
    h.advance(60_000);
    h.tick().await;
    h.assert_executed("test");
    h.assert_execution_count("test", 1);
}
