//! Compression daemon implementation

use super::config::CompressionDaemonConfig;
use std::future::Future;
use std::pin::Pin;
use crate::sync_primitives::{AtomicBool, AtomicI64, Ordering};
use crate::sync_primitives::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Type alias for compression function
type CompressionFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// Compression daemon for background memory compression
pub struct CompressionDaemon {
    config: CompressionDaemonConfig,
    compress_fn: CompressionFn,
    is_running: AtomicBool,
    last_activity: Arc<AtomicI64>,
}

impl CompressionDaemon {
    /// Create a new compression daemon
    ///
    /// # Arguments
    /// * `config` - Daemon configuration
    /// * `compress_fn` - Function to call for compression
    pub fn new<F, Fut>(config: CompressionDaemonConfig, compress_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let compress_fn = Arc::new(move || {
            let fut = compress_fn();
            Box::pin(fut) as Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        });

        Self {
            config,
            compress_fn,
            is_running: AtomicBool::new(false),
            last_activity: Arc::new(AtomicI64::new(Self::now_timestamp())),
        }
    }

    /// Check if daemon is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Record user activity (resets idle timer)
    pub fn record_activity(&self) {
        self.last_activity.store(Self::now_timestamp(), Ordering::Relaxed);
    }

    /// Get current timestamp in seconds
    fn now_timestamp() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs() as i64
    }

    /// Get idle time in seconds
    fn idle_seconds(&self) -> i64 {
        let now = Self::now_timestamp();
        let last = self.last_activity.load(Ordering::Relaxed);
        (now - last).max(0)
    }

    /// Check if system is idle enough to run compression
    fn is_idle(&self) -> bool {
        self.idle_seconds() >= self.config.idle_threshold_seconds as i64
    }

    /// Start the daemon background task
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        if !self.config.enabled {
            warn!("CompressionDaemon is disabled, not starting");
            return tokio::spawn(async {});
        }

        self.is_running.store(true, Ordering::SeqCst);
        info!(
            check_interval = self.config.check_interval_seconds,
            idle_threshold = self.config.idle_threshold_seconds,
            "CompressionDaemon starting"
        );

        tokio::spawn(async move {
            self.run_scheduler().await;
        })
    }

    /// Stop the daemon
    pub fn stop(&self) {
        info!("CompressionDaemon stopping");
        self.is_running.store(false, Ordering::SeqCst);
    }

    /// Main scheduler loop
    async fn run_scheduler(&self) {
        let mut ticker = interval(Duration::from_secs(self.config.check_interval_seconds));

        loop {
            ticker.tick().await;

            if !self.is_running.load(Ordering::SeqCst) {
                debug!("CompressionDaemon stopped");
                break;
            }

            // Check if system is idle
            if !self.is_idle() {
                debug!(
                    idle_seconds = self.idle_seconds(),
                    threshold = self.config.idle_threshold_seconds,
                    "System not idle, skipping compression"
                );
                continue;
            }

            // Run compression
            info!("Running scheduled compression");
            match (self.compress_fn)().await {
                Ok(()) => {
                    info!("Compression completed successfully");
                }
                Err(e) => {
                    error!(error = %e, "Compression failed");
                }
            }
        }
    }
}

// Implement Send + Sync for CompressionDaemon
unsafe impl Send for CompressionDaemon {}
unsafe impl Sync for CompressionDaemon {}
