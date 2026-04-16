//! Health Monitoring and Auto-Recovery for Microsoft Teams Channel
//!
//! Provides health status tracking, stale detection, and automatic recovery
//! for Teams channel connections.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::gateway::channel::ChannelError;

/// Health status of a channel
#[derive(Debug, Clone)]
pub enum HealthStatus {
    /// Healthy: receiving messages normally
    Healthy,
    /// Degraded: missed some health checks
    Degraded { missed_checks: u32 },
    /// Stale: no messages for threshold duration
    Stale { last_activity: Instant },
    /// Restarting: recovering from stale state
    Restarting,
}

impl HealthStatus {
    /// Check if channel is healthy
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }

    /// Check if channel is stale
    pub fn is_stale(&self) -> bool {
        matches!(self, HealthStatus::Stale { .. })
    }

    /// Check if channel is restarting
    pub fn is_restarting(&self) -> bool {
        matches!(self, HealthStatus::Restarting)
    }
}

/// Channel with health monitoring capability
pub trait HealthyChannel: Send + Sync {
    /// Check if channel is stale
    async fn is_stale(&self) -> bool;

    /// Graceful restart
    async fn restart(&mut self) -> Result<(), ChannelError>;

    /// Get current health status
    async fn health_status(&self) -> HealthStatus;
}

/// Monitor with auto-recovery
pub struct ChannelHealthMonitor {
    check_interval: Duration,
    stale_threshold: Duration,
    max_restart_attempts: u32,
    restart_attempts: AtomicU32,
}

impl ChannelHealthMonitor {
    /// Create a new ChannelHealthMonitor
    pub fn new(
        check_interval: Duration,
        stale_threshold: Duration,
        max_restart_attempts: u32,
    ) -> Self {
        Self {
            check_interval,
            stale_threshold,
            max_restart_attempts,
            restart_attempts: AtomicU32::new(0),
        }
    }

    /// Create with sensible defaults: check every 60s, stale after 5min, max 3 restart attempts
    pub fn default_monitor() -> Self {
        Self::new(
            Duration::from_secs(60),
            Duration::from_secs(300),
            3,
        )
    }

    /// Check channel health and recover if stale
    pub async fn check_and_recover<C: HealthyChannel>(
        &self,
        channel: &mut C,
    ) -> Result<RecoveryAction, ChannelError> {
        if channel.is_stale().await {
            let attempts = self.restart_attempts.load(Ordering::SeqCst);
            if attempts >= self.max_restart_attempts {
                return Err(ChannelError::Internal(
                    "Max restart attempts exceeded for health recovery".into(),
                ));
            }

            self.restart_attempts.fetch_add(1, Ordering::SeqCst);
            channel.restart().await?;
            self.restart_attempts.store(0, Ordering::SeqCst);
            return Ok(RecoveryAction::Restarted);
        }
        Ok(RecoveryAction::Healthy)
    }

    /// Get current restart attempts
    pub fn restart_attempts(&self) -> u32 {
        self.restart_attempts.load(Ordering::SeqCst)
    }

    /// Get configured stale threshold
    pub fn stale_threshold(&self) -> Duration {
        self.stale_threshold
    }

    /// Get configured check interval
    pub fn check_interval(&self) -> Duration {
        self.check_interval
    }

    /// Get max restart attempts
    pub fn max_restart_attempts(&self) -> u32 {
        self.max_restart_attempts
    }
}

/// Result of a recovery check
#[derive(Debug, Clone, Copy)]
pub enum RecoveryAction {
    /// Channel is healthy, no action needed
    Healthy,
    /// Channel was restarted
    Restarted,
    /// Channel is stale but restart would exceed max attempts
    WouldExceedMaxAttempts,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_is_healthy() {
        assert!(HealthStatus::Healthy.is_healthy());
        assert!(!HealthStatus::Healthy.is_stale());
        assert!(!HealthStatus::Healthy.is_restarting());
    }

    #[test]
    fn test_health_status_degraded() {
        let status = HealthStatus::Degraded { missed_checks: 3 };
        assert!(!status.is_healthy());
        assert!(!status.is_stale());
        assert!(!status.is_restarting());
    }

    #[test]
    fn test_health_status_stale() {
        let status = HealthStatus::Stale {
            last_activity: Instant::now(),
        };
        assert!(!status.is_healthy());
        assert!(status.is_stale());
        assert!(!status.is_restarting());
    }

    #[test]
    fn test_health_status_restarting() {
        let status = HealthStatus::Restarting;
        assert!(!status.is_healthy());
        assert!(!status.is_stale());
        assert!(status.is_restarting());
    }

    #[test]
    fn test_channel_health_monitor_default() {
        let monitor = ChannelHealthMonitor::default_monitor();
        assert_eq!(monitor.check_interval(), Duration::from_secs(60));
        assert_eq!(monitor.stale_threshold(), Duration::from_secs(300));
        assert_eq!(monitor.max_restart_attempts(), 3);
        assert_eq!(monitor.restart_attempts(), 0);
    }

    #[test]
    fn test_channel_health_monitor_custom() {
        let monitor = ChannelHealthMonitor::new(
            Duration::from_secs(30),
            Duration::from_secs(120),
            5,
        );
        assert_eq!(monitor.check_interval(), Duration::from_secs(30));
        assert_eq!(monitor.stale_threshold(), Duration::from_secs(120));
        assert_eq!(monitor.max_restart_attempts(), 5);
    }

    #[test]
    fn test_recovery_action_healthy() {
        assert!(matches!(RecoveryAction::Healthy, RecoveryAction::Healthy));
    }

    #[test]
    fn test_recovery_action_restarted() {
        assert!(matches!(RecoveryAction::Restarted, RecoveryAction::Restarted));
    }
}