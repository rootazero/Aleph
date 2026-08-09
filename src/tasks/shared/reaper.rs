//! Periodic task reaper daemon.
//!
//! A single background tokio task that sweeps every `interval_secs`
//! across all task-subsystem persistence layers:
//!
//! - Cron `cron_job_runs` history (older than `cron_history_retention_days`)
//! - Heartbeat `heartbeat_runs` history (older than `heartbeat_history_retention_days`)
//! - Heartbeat `heartbeat_dedup` rows (older than `dedup_retention_secs`)
//!
//! Before this module landed, each of these cleanup functions existed but
//! had **zero production callers** (`CronStore::cleanup_old_runs`,
//! `HeartbeatStore::cleanup_old_runs`, `DedupEngine::cleanup`). The reaper
//! is the one consumer that lights them all up — see [[retry-hint]] commit
//! for the parallel discovery that `should_send_alert` was the same kind of
//! dead wire.
//!
//! ## Design
//!
//! Mirrors openclaw's `src/cron/session-reaper.ts` in spirit (periodic
//! cleanup loop) but unifies cron + heartbeat into one process so we don't
//! end up with one timer per table.
//!
//! - **Single-flight per process**: `spawn_task_reaper` returns immediately
//!   if a reaper is already running for this process (atomic-bool guard).
//! - **Resilient**: each cleanup step is wrapped in `try_*` — a failure on
//!   one subsystem does not skip the others, and errors are logged at
//!   `warn` level (not `error`) since data integrity is unaffected.
//! - **Cooperative shutdown**: respects `tokio::select!` on a shutdown
//!   signal so the daemon exits cleanly with the rest of `aleph-server`.

use crate::sync_primitives::{AtomicBool, Ordering};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::sync_primitives::Arc;

/// Default reap interval: 6 hours. Long enough that the per-tick I/O cost is
/// negligible; short enough that retention windows are honoured within the
/// same calendar day. Operators can lower for testing or raise for very
/// long-running deployments.
const DEFAULT_INTERVAL_SECS: u64 = 6 * 3600;
/// Default dedup retention: 14 days. Heartbeat dedup history is per-task,
/// so this controls how far back the cosine-similarity check looks.
const DEFAULT_DEDUP_RETENTION_SECS: u64 = 14 * 86_400;

/// Configuration for the periodic [[reaper]] daemon.
///
/// All fields have `serde(default)` so older config files (which never knew
/// about this section) deserialize unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReaperConfig {
    /// Whether the reaper is enabled. Default: `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Sweep interval in seconds. Default: `21_600` (6 hours).
    /// Clamped to `[60, 86_400]` at startup.
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,

    /// How far back to keep heartbeat dedup rows, in seconds.
    /// Default: `1_209_600` (14 days).
    #[serde(default = "default_dedup_retention_secs")]
    pub dedup_retention_secs: u64,
}

const fn default_true() -> bool {
    true
}
const fn default_interval_secs() -> u64 {
    DEFAULT_INTERVAL_SECS
}
const fn default_dedup_retention_secs() -> u64 {
    DEFAULT_DEDUP_RETENTION_SECS
}

impl Default for ReaperConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: DEFAULT_INTERVAL_SECS,
            dedup_retention_secs: DEFAULT_DEDUP_RETENTION_SECS,
        }
    }
}

impl ReaperConfig {
    /// Apply sanity bounds to user-supplied values.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.interval_secs = self.interval_secs.clamp(60, 86_400);
        // Dedup retention must be at least one hour or comparison loses meaning.
        if self.dedup_retention_secs < 3600 {
            self.dedup_retention_secs = 3600;
        }
        self
    }
}

/// Trait abstraction over cleanable subsystems so the reaper loop can be
/// driven in unit tests without standing up real services. Implementors are
/// the cron and heartbeat services (in production) and small fakes (in
/// tests).
#[async_trait::async_trait]
pub trait HistoryReap: Send + Sync {
    /// Subsystem name used in tracing output.
    fn name(&self) -> &'static str;

    /// Perform the periodic cleanup. Returns the number of rows deleted.
    async fn reap(&self) -> Result<u64, String>;
}

/// Singleton guard: prevents multiple reapers from running concurrently in
/// the same process (e.g. if startup wiring accidentally calls `spawn_task_reaper`
/// twice). The first call wins; subsequent calls are no-ops.
static REAPER_STARTED: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn reset_started_for_tests() {
    REAPER_STARTED.store(false, Ordering::SeqCst);
}

/// Spawn the reaper background task. Returns `Some(JoinHandle)` if this was
/// the first call in the process; `None` if a reaper is already running.
///
/// The `shutdown` watch channel is observed via `tokio::select!`; sending
/// `true` triggers a clean exit from the loop on the next tick.
pub fn spawn_task_reaper(
    config: ReaperConfig,
    subsystems: Vec<Arc<dyn HistoryReap>>,
    shutdown: watch::Receiver<bool>,
) -> Option<tokio::task::JoinHandle<()>> {
    if !config.enabled {
        info!("task reaper disabled via config");
        return None;
    }
    if REAPER_STARTED.swap(true, Ordering::AcqRel) {
        warn!("task reaper already running, refusing to spawn a second instance");
        return None;
    }
    let cfg = config.clamped();
    info!(
        interval_secs = cfg.interval_secs,
        dedup_retention_secs = cfg.dedup_retention_secs,
        subsystem_count = subsystems.len(),
        "spawning task reaper"
    );
    Some(tokio::spawn(async move {
        run_reaper_loop(cfg, subsystems, shutdown).await;
        // Release the slot if the loop exits cleanly so that a tooling-driven
        // restart (currently only test code) can spawn another.
        REAPER_STARTED.store(false, Ordering::Release);
    }))
}

/// Inner loop driven by `spawn_task_reaper`. Made public so tests can drive
/// it with a fake clock + manual tick.
pub async fn run_reaper_loop(
    config: ReaperConfig,
    subsystems: Vec<Arc<dyn HistoryReap>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let interval = tokio::time::Duration::from_secs(config.interval_secs);
    loop {
        tokio::select! {
            biased;
            res = shutdown.changed() => {
                if res.is_err() || *shutdown.borrow() {
                    info!("task reaper: shutdown signalled, exiting");
                    return;
                }
            }
            _ = tokio::time::sleep(interval) => {
                reap_once(&subsystems).await;
            }
        }
    }
}

/// Run one pass over every registered subsystem.
///
/// Exposed for tests + for callers that want to force a one-shot sweep at
/// boot (e.g. immediately after services come up, before the first sleep
/// interval elapses). Failures on one subsystem do not skip the others.
pub async fn reap_once(subsystems: &[Arc<dyn HistoryReap>]) {
    for sub in subsystems {
        match sub.reap().await {
            Ok(0) => debug!(subsystem = sub.name(), "reaper: nothing to clean"),
            Ok(n) => info!(subsystem = sub.name(), deleted = n, "reaper: swept rows"),
            Err(e) => warn!(subsystem = sub.name(), error = %e, "reaper: sweep failed"),
        }
    }
}

// ── Service adapters ─────────────────────────────────────────────────────
//
// Thin adapters that turn the long-lived service handles into [`HistoryReap`]
// trait objects. Kept here (rather than in `tasks::cron` / `tasks::heartbeat`)
// so the reaper is the single owner of cross-subsystem cleanup glue — the
// service modules continue to know nothing about the reaper.

use crate::tasks::cron::SharedCronService;
use crate::tasks::heartbeat::dedup::DedupEngine;
use crate::tasks::heartbeat::SharedHeartbeatService;

/// `HistoryReap` adapter that calls `CronService::reap_history`.
pub struct CronHistoryReaper(pub SharedCronService);

#[async_trait::async_trait]
impl HistoryReap for CronHistoryReaper {
    fn name(&self) -> &'static str {
        "cron_history"
    }

    async fn reap(&self) -> Result<u64, String> {
        let svc = self.0.lock().await;
        // The reaper's own contract stays `String`: every failure it can see
        // is a store failure, and its only consumer is a log line. Flattening
        // here is the classification, not a loss of one.
        svc.reap_history().await.map_err(|e| e.to_string())
    }
}

/// `HistoryReap` adapter that hard-deletes aged cron isolated-run session
/// directories.
///
/// [`CronHistoryReaper`] trims the `cron_job_runs` rows, but every
/// `SessionTarget::Isolated` run also materializes an
/// `agent:<id>:cron:<job>-<ts>` session directory that no other cleanup path
/// touched — they accumulated without bound (this adapter is the missing
/// consumer, mirroring how the reaper module first lit up the dead history
/// cleanups). Trims them on the same `history_retention_days` horizon as the
/// run-history rows so audit state and transcripts stay consistent.
pub struct CronSessionReaper {
    pub store: Arc<dyn crate::gateway::session_store::SessionStore>,
    pub retention_days: u32,
}

#[async_trait::async_trait]
impl HistoryReap for CronSessionReaper {
    fn name(&self) -> &'static str {
        "cron_sessions"
    }

    async fn reap(&self) -> Result<u64, String> {
        let cutoff_secs = chrono::Utc::now().timestamp() - i64::from(self.retention_days) * 86_400;
        self.store
            .reap_task_sessions("cron", cutoff_secs)
            .await
            .map(|n| n as u64)
            .map_err(|e| e.to_string())
    }
}

/// `HistoryReap` adapter that calls `HeartbeatService::reap_history`.
pub struct HeartbeatHistoryReaper(pub SharedHeartbeatService);

#[async_trait::async_trait]
impl HistoryReap for HeartbeatHistoryReaper {
    fn name(&self) -> &'static str {
        "heartbeat_history"
    }

    async fn reap(&self) -> Result<u64, String> {
        let svc = self.0.lock().await;
        svc.reap_history()
            .await
            .map(|n| n as u64)
            .map_err(|e| e.to_string())
    }
}

/// `HistoryReap` adapter for the heartbeat dedup table.
///
/// Holds the configured retention window — unlike the history tables which
/// derive retention from each service's `*Config`, dedup retention is owned
/// by [`ReaperConfig`] because dedup is shared infrastructure and its
/// retention is a reaper-wide policy decision.
pub struct DedupHistoryReaper {
    pub engine: Arc<DedupEngine>,
    pub retention_secs: u64,
}

#[async_trait::async_trait]
impl HistoryReap for DedupHistoryReaper {
    fn name(&self) -> &'static str {
        "heartbeat_dedup"
    }

    async fn reap(&self) -> Result<u64, String> {
        // `DedupEngine::cleanup` returns no count today, so we report 0;
        // callers see "swept" only if they observe row counts elsewhere.
        // Mirrors the openclaw cleanup contract.
        self.engine
            .cleanup(self.retention_secs.saturating_mul(1000))
            .await;
        Ok(0)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as AOrdering};

    struct FakeSubsystem {
        name: &'static str,
        call_count: AtomicU64,
        per_call_rows: u64,
        fail_after: Option<u64>,
    }

    impl FakeSubsystem {
        fn new(name: &'static str, rows: u64) -> Arc<Self> {
            Arc::new(Self {
                name,
                call_count: AtomicU64::new(0),
                per_call_rows: rows,
                fail_after: None,
            })
        }

        fn with_failure_after(name: &'static str, rows: u64, fail_after: u64) -> Arc<Self> {
            Arc::new(Self {
                name,
                call_count: AtomicU64::new(0),
                per_call_rows: rows,
                fail_after: Some(fail_after),
            })
        }

        fn calls(&self) -> u64 {
            self.call_count.load(AOrdering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl HistoryReap for FakeSubsystem {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn reap(&self) -> Result<u64, String> {
            let prev = self.call_count.fetch_add(1, AOrdering::SeqCst);
            if let Some(threshold) = self.fail_after {
                if prev >= threshold {
                    return Err(format!("{} synthetic failure", self.name));
                }
            }
            Ok(self.per_call_rows)
        }
    }

    #[test]
    fn config_clamps_out_of_range() {
        let too_short = ReaperConfig {
            interval_secs: 1,
            ..Default::default()
        }
        .clamped();
        assert_eq!(too_short.interval_secs, 60);

        let too_long = ReaperConfig {
            interval_secs: 999_999,
            ..Default::default()
        }
        .clamped();
        assert_eq!(too_long.interval_secs, 86_400);

        let dedup_floor = ReaperConfig {
            dedup_retention_secs: 5,
            ..Default::default()
        }
        .clamped();
        assert_eq!(dedup_floor.dedup_retention_secs, 3600);
    }

    #[test]
    fn config_default_serde_round_trip() {
        // Older configs without the section must deserialize cleanly.
        let cfg: ReaperConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.interval_secs, DEFAULT_INTERVAL_SECS);
        assert_eq!(cfg.dedup_retention_secs, DEFAULT_DEDUP_RETENTION_SECS);
    }

    #[tokio::test]
    async fn reap_once_calls_every_subsystem() {
        let a = FakeSubsystem::new("a", 3);
        let b = FakeSubsystem::new("b", 0);
        let c = FakeSubsystem::new("c", 17);
        let subs: Vec<Arc<dyn HistoryReap>> = vec![a.clone(), b.clone(), c.clone()];
        reap_once(&subs).await;
        assert_eq!(a.calls(), 1);
        assert_eq!(b.calls(), 1);
        assert_eq!(c.calls(), 1);
    }

    #[tokio::test]
    async fn one_subsystem_failure_does_not_skip_others() {
        let good_a = FakeSubsystem::new("good_a", 1);
        let bad = FakeSubsystem::with_failure_after("bad", 0, 0); // fails on first call
        let good_b = FakeSubsystem::new("good_b", 2);
        let subs: Vec<Arc<dyn HistoryReap>> = vec![good_a.clone(), bad.clone(), good_b.clone()];
        reap_once(&subs).await;
        assert_eq!(good_a.calls(), 1);
        assert_eq!(bad.calls(), 1);
        assert_eq!(
            good_b.calls(),
            1,
            "subsystem after failure must still be called"
        );
    }

    // The next three tests all touch the process-wide `REAPER_STARTED`
    // singleton flag; `serial_test::serial` keeps them from racing with each
    // other when the suite is run with the default parallel test runner.

    #[tokio::test]
    #[serial_test::serial(reaper_singleton)]
    async fn shutdown_signal_exits_loop_promptly() {
        reset_started_for_tests();
        let subs: Vec<Arc<dyn HistoryReap>> = vec![FakeSubsystem::new("svc", 0)];
        // Big interval so the only exit path is the shutdown signal.
        let cfg = ReaperConfig {
            interval_secs: 86_400,
            ..Default::default()
        };
        let (tx, rx) = watch::channel(false);
        let handle = spawn_task_reaper(cfg, subs, rx).expect("first reaper spawn");
        tx.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("reaper did not exit within 2s")
            .expect("reaper task panicked");
        // Give the loop's `REAPER_STARTED.store(false)` line a beat to run
        // before the next serial test asserts singleton behaviour.
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    #[serial_test::serial(reaper_singleton)]
    async fn spawn_is_singleton_per_process() {
        reset_started_for_tests();
        let subs1: Vec<Arc<dyn HistoryReap>> = vec![FakeSubsystem::new("s1", 0)];
        let subs2: Vec<Arc<dyn HistoryReap>> = vec![FakeSubsystem::new("s2", 0)];
        // Use a huge interval so the first reaper never actually fires before
        // we signal shutdown — keeps the test deterministic.
        let cfg = ReaperConfig {
            interval_secs: 86_400,
            ..Default::default()
        };
        let (tx, rx1) = watch::channel(false);
        let rx2 = rx1.clone();
        let h1 = spawn_task_reaper(cfg.clone(), subs1, rx1).expect("first spawn returns a handle");
        let h2 = spawn_task_reaper(cfg, subs2, rx2);
        assert!(h2.is_none(), "second spawn must be rejected as duplicate");

        tx.send(true).unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), h1).await;
    }

    #[tokio::test]
    #[serial_test::serial(reaper_singleton)]
    async fn disabled_config_does_not_spawn() {
        reset_started_for_tests();
        let cfg = ReaperConfig {
            enabled: false,
            ..Default::default()
        };
        let (_tx, rx) = watch::channel(false);
        let subs: Vec<Arc<dyn HistoryReap>> = vec![FakeSubsystem::new("s", 0)];
        assert!(spawn_task_reaper(cfg, subs, rx).is_none());
    }
}
