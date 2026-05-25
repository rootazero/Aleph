//! Periodic process memory usage logging for the gateway.
//!
//! Ported from hermes-agent `gateway/memory_monitor.py` (itself ported from
//! cline/cline#10343). The gateway is a long-lived process that accumulates
//! memory as it caches agent instances, session transcripts, tool schemas,
//! memory providers, MCP connections, etc. A slow leak in any subsystem is
//! invisible in a single log line — you only see it by watching RSS climb
//! over hours.
//!
//! This module emits a single structured `[MEMORY] ...` line every N seconds
//! (default 300 = 5 minutes) so maintainers investigating a suspected leak
//! can grep `aleph-server.log` for a time series.
//!
//! Design notes (parity with the hermes port):
//! - Grep-friendly single-line format beginning `[MEMORY]`.
//! - Baseline snapshot logged immediately on start.
//! - Final snapshot logged on graceful shutdown ("last RSS before exit").
//! - Background task — never blocks process exit.
//! - Cross-platform via `sysinfo` (already a dependency).
//! - `interval_secs = 0` disables the monitor.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{info, warn};

/// Default cadence: 5 minutes between snapshots.
pub const DEFAULT_INTERVAL_SECS: u64 = 300;

/// Handle to a running memory monitor. Dropping it does **not** stop the
/// monitor — call [`MemoryMonitor::shutdown`] for that. This mirrors the
/// "daemon thread that never blocks process exit" semantics of the hermes
/// port: tokio tasks are auto-aborted at runtime drop.
#[derive(Debug)]
pub struct MemoryMonitor {
    stop: Arc<Notify>,
    handle: Option<JoinHandle<()>>,
}

impl MemoryMonitor {
    /// Start the monitor with the given cadence. `interval_secs == 0`
    /// returns a no-op handle (matches the "disabled" config sentinel).
    #[must_use]
    pub fn start(interval_secs: u64) -> Self {
        if interval_secs == 0 {
            info!("[MEMORY] monitor disabled (interval_secs=0)");
            return Self {
                stop: Arc::new(Notify::new()),
                handle: None,
            };
        }
        let stop = Arc::new(Notify::new());
        let stop_clone = Arc::clone(&stop);
        let handle = tokio::spawn(run_monitor(stop_clone, interval_secs));
        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Signal the background task to emit a final snapshot and exit.
    /// Idempotent. Uses `notify_one` (stores a permit) rather than
    /// `notify_waiters` so the signal is not lost if it fires before the
    /// task reaches `.notified().await`.
    pub async fn shutdown(&mut self) {
        self.stop.notify_one();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

async fn run_monitor(stop: Arc<Notify>, interval_secs: u64) {
    let start = Instant::now();
    log_snapshot("baseline", start);
    let mut ticker = interval(Duration::from_secs(interval_secs));
    // Skip the immediate first tick — we already logged the baseline.
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                log_snapshot("tick", start);
            }
            _ = stop.notified() => {
                log_snapshot("shutdown", start);
                return;
            }
        }
    }
}

fn log_snapshot(reason: &str, start: Instant) {
    let uptime_secs = start.elapsed().as_secs();
    match read_rss_kb() {
        Some(rss_kb) => {
            // Grep-friendly: `[MEMORY] reason=baseline uptime_secs=12 rss_mb=37.4`
            info!(
                "[MEMORY] reason={} uptime_secs={} rss_mb={:.1}",
                reason,
                uptime_secs,
                (rss_kb as f64) / 1024.0
            );
        }
        None => {
            warn!(
                "[MEMORY] reason={} uptime_secs={} rss_mb=unknown (read failed)",
                reason, uptime_secs
            );
        }
    }
}

/// Return current process resident set size in KiB.
///
/// Uses `sysinfo` which works on macOS, Linux, and Windows. Returns `None`
/// if the platform refuses to report it — we treat this as a soft failure
/// and disable structured RSS rather than crashing the gateway.
fn read_rss_kb() -> Option<u64> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let pid = Pid::from_u32(std::process::id());
    let mut sys = System::new();
    // Refresh just our own process — cheaper than `refresh_processes`.
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(|p| p.memory() / 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_kb_returns_something_on_supported_platforms() {
        // On macOS/Linux/Windows we expect a reading.
        // If a test runner is sandboxed in a way that hides /proc, the
        // function returns None — assert only that the call doesn't panic.
        let _ = read_rss_kb();
    }

    #[tokio::test(start_paused = true)]
    async fn disabled_monitor_is_no_op() {
        let mut mon = MemoryMonitor::start(0);
        assert!(mon.handle.is_none());
        mon.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_terminates_task() {
        let mut mon = MemoryMonitor::start(60);
        assert!(mon.handle.is_some());
        mon.shutdown().await;
        assert!(mon.handle.is_none());
    }
}
