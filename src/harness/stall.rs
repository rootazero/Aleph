use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;

const DEFAULT_STALL_TIMEOUT_SECS: u64 = 300;
const DEFAULT_STALL_CHECK_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct StallConfig {
    pub timeout: Duration,
    pub check_interval: Duration,
}

impl Default for StallConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_STALL_TIMEOUT_SECS),
            check_interval: Duration::from_secs(DEFAULT_STALL_CHECK_INTERVAL_SECS),
        }
    }
}

impl StallConfig {
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_check_interval(mut self, interval: Duration) -> Self {
        self.check_interval = interval;
        self
    }
}

#[derive(Debug)]
pub struct StallTracker {
    last_activity: Arc<TokioMutex<Instant>>,
    config: StallConfig,
    cancel: CancellationToken,
}

impl StallTracker {
    pub fn new(config: StallConfig, cancel: CancellationToken) -> Self {
        Self {
            last_activity: Arc::new(TokioMutex::new(Instant::now())),
            config,
            cancel,
        }
    }

    /// Record activity (async, for use in async context)
    pub async fn record_activity(&self) {
        *self.last_activity.lock().await = Instant::now();
    }

    /// Get elapsed time since last activity (async)
    pub async fn elapsed(&self) -> Duration {
        self.last_activity.lock().await.elapsed()
    }

    pub fn is_stalled(&self) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        if let Ok(guard) = self.last_activity.try_lock() {
            guard.elapsed() > self.config.timeout
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_not_stalled_when_recent_activity() {
        let cancel = CancellationToken::new();
        let config = StallConfig::default();
        let tracker = StallTracker::new(config, cancel);

        tracker.record_activity().await;

        assert!(!tracker.is_stalled());
    }

    #[tokio::test]
    async fn test_stalled_when_timeout_exceeded() {
        let cancel = CancellationToken::new();
        let config = StallConfig {
            timeout: Duration::from_millis(10),
            check_interval: Duration::from_millis(1),
        };
        let tracker = StallTracker::new(config, cancel);

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(tracker.is_stalled());
    }

    #[tokio::test]
    async fn test_cancel_takes_precedence() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let config = StallConfig {
            timeout: Duration::ZERO,
            check_interval: Duration::from_millis(1),
        };
        let tracker = StallTracker::new(config, cancel);

        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(!tracker.is_stalled());
    }
}
