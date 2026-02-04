use crate::daemon::{DaemonError, Result};
use sysinfo::{System, RefreshKind, CpuRefreshKind};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Resource limits configuration
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// CPU usage threshold (percentage)
    pub cpu_threshold: f32,

    /// Memory usage threshold (bytes)
    pub mem_threshold: u64,

    /// Battery level threshold (percentage)
    pub battery_threshold: f32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_threshold: 20.0,
            mem_threshold: 512 * 1024 * 1024, // 512MB
            battery_threshold: 20.0,
        }
    }
}

/// Governor decision on whether to proceed with proactive tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernorDecision {
    /// System resources are available, proceed with tasks
    Proceed,

    /// System is under stress, throttle proactive tasks
    Throttle,
}

/// Resource Governor monitors system resources and throttles operations
pub struct ResourceGovernor {
    limits: ResourceLimits,
    system: Arc<RwLock<System>>,
}

impl ResourceGovernor {
    /// Create a new ResourceGovernor with specified limits
    pub fn new(limits: ResourceLimits) -> Self {
        let refresh_kind = RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage());

        Self {
            limits,
            system: Arc::new(RwLock::new(System::new_with_specifics(refresh_kind))),
        }
    }

    /// Get current resource limits
    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }

    /// Check system resources and return decision
    pub async fn check(&self) -> Result<GovernorDecision> {
        let mut system = self.system.write().await;

        // Refresh system information
        system.refresh_cpu_usage();
        system.refresh_memory();

        // Check CPU usage
        let cpu_usage = system.global_cpu_usage();
        if cpu_usage > self.limits.cpu_threshold {
            debug!(
                "CPU usage ({:.1}%) exceeds threshold ({:.1}%)",
                cpu_usage, self.limits.cpu_threshold
            );
            return Ok(GovernorDecision::Throttle);
        }

        // Check memory usage (process-specific)
        let pid = sysinfo::get_current_pid()
            .map_err(|e| DaemonError::ResourceGovernor(format!("Failed to get PID: {}", e)))?;

        if let Some(process) = system.process(pid) {
            let mem_usage = process.memory();
            if mem_usage > self.limits.mem_threshold {
                warn!(
                    "Memory usage ({} bytes) exceeds threshold ({} bytes)",
                    mem_usage, self.limits.mem_threshold
                );
                return Ok(GovernorDecision::Throttle);
            }
        }

        // Check battery level (if on battery power)
        if let Ok(manager) = battery::Manager::new() {
            if let Ok(batteries) = manager.batteries() {
                for battery_result in batteries {
                    if let Ok(battery) = battery_result {
                        let state = battery.state();
                        let level = battery.state_of_charge().value * 100.0;

                        // Only throttle if on battery and below threshold
                        if matches!(state, battery::State::Discharging)
                            && level < self.limits.battery_threshold
                        {
                            debug!(
                                "Battery level ({:.1}%) below threshold ({:.1}%)",
                                level, self.limits.battery_threshold
                            );
                            return Ok(GovernorDecision::Throttle);
                        }
                    }
                }
            }
        }

        Ok(GovernorDecision::Proceed)
    }

    /// Check if it's safe to run proactive tasks
    pub async fn is_safe_to_run(&self) -> bool {
        matches!(self.check().await, Ok(GovernorDecision::Proceed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resource_governor_default_limits() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.cpu_threshold, 20.0);
        assert_eq!(limits.mem_threshold, 512 * 1024 * 1024);
        assert_eq!(limits.battery_threshold, 20.0);
    }
}
