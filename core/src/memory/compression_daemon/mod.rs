//! Compression Daemon Module
//!
//! Provides background scheduling for memory compression tasks.

pub mod daemon;
pub mod config;

pub use daemon::CompressionDaemon;
pub use config::CompressionDaemonConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::{AtomicUsize, Ordering};
    use crate::sync_primitives::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn test_daemon_config_default() {
        let config = CompressionDaemonConfig::default();
        assert_eq!(config.check_interval_seconds, 3600);  // 1 hour
        assert_eq!(config.idle_threshold_seconds, 300);   // 5 minutes
        assert!(config.enabled);
    }

    #[tokio::test]
    async fn test_daemon_creation() {
        let config = CompressionDaemonConfig {
            check_interval_seconds: 1,
            idle_threshold_seconds: 0,
            enabled: true,
        };

        // Create a mock compression function
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let compress_fn = move || {
            let counter = counter_clone.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok::<(), String>(())
            })
        };

        let daemon = CompressionDaemon::new(config, compress_fn);
        assert!(daemon.is_enabled());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_daemon_runs_compression() {
        let config = CompressionDaemonConfig {
            check_interval_seconds: 1,  // Check every second
            idle_threshold_seconds: 0,  // No idle requirement
            enabled: true,
        };

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let compress_fn = move || {
            let counter = counter_clone.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok::<(), String>(())
            })
        };

        let daemon = Arc::new(CompressionDaemon::new(config, compress_fn));

        // Start daemon
        let handle = daemon.clone().start();

        // Wait for at least 2 compressions
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Stop daemon
        daemon.stop();
        handle.abort();

        // Should have run at least once
        let count = counter.load(Ordering::SeqCst);
        assert!(count >= 1, "Expected at least 1 compression, got {}", count);
    }
}
