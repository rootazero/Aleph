//! CronTestHarness — ergonomic wrapper around ServiceState<FakeClock>
//! for cron probe integration tests.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

use alephcore::cron::clock::Clock;
use alephcore::cron::clock::testing::FakeClock;
use alephcore::cron::config::{
    CronConfig, CronJob, CronJobView, JobStateV2, ScheduleKind,
};
use alephcore::cron::service::catchup::run_startup_catchup;
use alephcore::cron::service::concurrency::phase1_mark_manual;
use alephcore::cron::service::ops::{self, CronJobUpdates};
use alephcore::cron::service::state::ServiceState;
use alephcore::cron::service::timer::{on_timer_tick, JobExecutorFn};
use alephcore::cron::store::CronStore;

use super::mock_executor::MockExecutor;

/// Test harness wrapping the cron ServiceState with a FakeClock.
pub struct CronTestHarness {
    pub state: Arc<ServiceState<FakeClock>>,
    pub clock: Arc<FakeClock>,
    pub executor: MockExecutor,
    pub store_path: PathBuf,
    _temp_dir: TempDir,
    executor_fn: JobExecutorFn,
}

impl CronTestHarness {
    /// Create a new harness with default CronConfig.
    pub fn new() -> Self {
        Self::with_config(CronConfig::default())
    }

    /// Create a new harness with a custom CronConfig.
    pub fn with_config(config: CronConfig) -> Self {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let store_path = temp_dir.path().join("cron.json");
        let store = CronStore::load(store_path.clone()).expect("failed to create store");

        let clock = Arc::new(FakeClock::new(1_000_000));
        let store = Arc::new(tokio::sync::Mutex::new(store));
        let state = Arc::new(ServiceState::new(store, Arc::clone(&clock), config));

        let executor = MockExecutor::new();
        let executor_fn = executor.into_executor_fn();

        Self {
            state,
            clock,
            executor,
            store_path,
            _temp_dir: temp_dir,
            executor_fn,
        }
    }

    // ── Job creation helpers ────────────────────────────────────────

    /// Add an interval-based job.
    pub async fn add_every_job(&self, id: &str, interval_ms: i64) {
        let mut job = CronJob::new(
            id,
            "test-agent",
            format!("prompt for {id}"),
            ScheduleKind::Every {
                every_ms: interval_ms,
                anchor_ms: None,
            },
        );
        job.id = id.to_string();
        self.add_job(job).await;
    }

    /// Add a cron-expression-based job.
    pub async fn add_cron_job(&self, id: &str, expr: &str) {
        let mut job = CronJob::new(
            id,
            "test-agent",
            format!("prompt for {id}"),
            ScheduleKind::Cron {
                expr: expr.to_string(),
                tz: None,
                stagger_ms: None,
            },
        );
        job.id = id.to_string();
        self.add_job(job).await;
    }

    /// Add a one-shot job.
    pub async fn add_at_job(&self, id: &str, at_ms: i64) {
        let mut job = CronJob::new(
            id,
            "test-agent",
            format!("prompt for {id}"),
            ScheduleKind::At {
                at: at_ms,
                delete_after_run: false,
            },
        );
        job.id = id.to_string();
        self.add_job(job).await;
    }

    /// Add a fully-configured CronJob.
    pub async fn add_job(&self, job: CronJob) {
        let mut store = self.state.store.lock().await;
        ops::add_job(&mut store, job, self.clock.as_ref());
        store.persist().expect("failed to persist after add_job");
    }

    // ── Job mutation helpers ────────────────────────────────────────

    /// Update a job with partial changes.
    pub async fn update_job(&self, id: &str, updates: CronJobUpdates) {
        let mut store = self.state.store.lock().await;
        ops::update_job(&mut store, id, updates, self.clock.as_ref())
            .expect("update_job failed");
        store.persist().expect("failed to persist after update_job");
    }

    /// Delete a job by ID.
    pub async fn delete_job(&self, id: &str) {
        let mut store = self.state.store.lock().await;
        ops::delete_job(&mut store, id).expect("delete_job failed");
        store.persist().expect("failed to persist after delete_job");
    }

    /// Toggle a job's enabled state. Returns the new enabled state.
    pub async fn toggle_job(&self, id: &str) -> bool {
        let mut store = self.state.store.lock().await;
        let result = ops::toggle_job(&mut store, id, self.clock.as_ref())
            .expect("toggle_job failed");
        store.persist().expect("failed to persist after toggle_job");
        result
    }

    // ── Time control ────────────────────────────────────────────────

    /// Advance the fake clock by `ms` milliseconds.
    pub fn advance(&self, ms: i64) {
        self.clock.advance(ms);
    }

    /// Set the fake clock to an absolute value.
    pub fn advance_to(&self, ms: i64) {
        self.clock.set(ms);
    }

    /// Get the current fake clock time.
    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    // ── Execution ───────────────────────────────────────────────────

    /// Run a single timer tick (mark due + execute + writeback).
    pub async fn tick(&self) {
        on_timer_tick(&self.state, &self.executor_fn)
            .await
            .expect("tick failed");
    }

    /// Run N timer ticks.
    pub async fn tick_n(&self, n: usize) {
        for _ in 0..n {
            self.tick().await;
        }
    }

    /// Run startup catchup.
    pub async fn run_catchup(&self) {
        run_startup_catchup(
            &self.state.store,
            self.clock.as_ref(),
            self.state.config.max_missed_jobs_per_restart,
            self.state.config.catchup_stagger_ms,
        )
        .await
        .expect("catchup failed");
    }

    /// Manually trigger a job.
    pub async fn manual_run(&self, id: &str) {
        let snapshot = phase1_mark_manual(&self.state.store, self.clock.as_ref(), id)
            .await
            .expect("manual_run failed");

        if let Some(snap) = snapshot {
            let result = (self.executor_fn)(snap.clone()).await;
            alephcore::cron::service::concurrency::phase3_writeback(
                &self.state.store,
                self.clock.as_ref(),
                &[(snap.id, result)],
            )
            .await
            .expect("manual_run writeback failed");
        }
    }

    // ── Assertions ──────────────────────────────────────────────────

    /// Assert that a job was executed at least once.
    pub fn assert_executed(&self, id: &str) {
        assert!(
            self.executor.was_executed(id),
            "expected job '{id}' to have been executed, but it was not"
        );
    }

    /// Assert that a job was NOT executed.
    pub fn assert_not_executed(&self, id: &str) {
        assert!(
            !self.executor.was_executed(id),
            "expected job '{id}' NOT to be executed, but it was"
        );
    }

    /// Assert exact execution count for a job.
    pub fn assert_execution_count(&self, id: &str, n: usize) {
        let actual = self.executor.call_count(id);
        assert_eq!(
            actual, n,
            "expected job '{id}' to be executed {n} times, but was executed {actual} times"
        );
    }

    /// Assert a job's enabled state.
    pub async fn assert_job_enabled(&self, id: &str, expected: bool) {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{id}' not found"));
        assert_eq!(
            job.enabled, expected,
            "expected job '{id}' enabled={expected}, got enabled={}", job.enabled
        );
    }

    /// Assert a job's consecutive error count.
    pub async fn assert_consecutive_errors(&self, id: &str, expected: u32) {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{id}' not found"));
        assert_eq!(
            job.state.consecutive_errors, expected,
            "expected job '{id}' consecutive_errors={expected}, got {}",
            job.state.consecutive_errors
        );
    }

    /// Assert a job's next_run_at_ms is after the given timestamp.
    pub async fn assert_next_run_after(&self, id: &str, ms: i64) {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{id}' not found"));
        let next = job.state.next_run_at_ms
            .expect(&format!("job '{id}' has no next_run_at_ms"));
        assert!(
            next > ms,
            "expected job '{id}' next_run_at_ms > {ms}, got {next}"
        );
    }

    /// Assert a job's running state.
    pub async fn assert_running(&self, id: &str, expected: bool) {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{id}' not found"));
        let is_running = job.state.running_at_ms.is_some();
        assert_eq!(
            is_running, expected,
            "expected job '{id}' running={expected}, got running={is_running}"
        );
    }

    // ── Queries ─────────────────────────────────────────────────────

    /// Get a job's state.
    pub async fn job_state(&self, id: &str) -> JobStateV2 {
        let store = self.state.store.lock().await;
        let job = store.get_job(id).expect(&format!("job '{id}' not found"));
        job.state.clone()
    }

    /// Check if a job exists.
    pub async fn job_exists(&self, id: &str) -> bool {
        let store = self.state.store.lock().await;
        store.get_job(id).is_some()
    }

    /// List all jobs as views.
    pub async fn list_jobs(&self) -> Vec<CronJobView> {
        let store = self.state.store.lock().await;
        ops::list_jobs(&store)
    }

    /// Read the raw store file content.
    pub fn store_file_content(&self) -> String {
        std::fs::read_to_string(&self.store_path)
            .unwrap_or_else(|_| String::new())
    }
}
