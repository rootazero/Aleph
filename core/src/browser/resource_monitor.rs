//! Resource Monitor
//!
//! Monitors system resources to enable adaptive browser instance allocation.

use std::sync::Arc;
use tokio::sync::RwLock;

/// System load information
#[derive(Debug, Clone, Copy)]
pub struct SystemLoad {
    /// Available RAM in bytes
    pub available_ram: u64,
    /// Total RAM in bytes
    pub total_ram: u64,
    /// CPU usage percentage (0-100)
    pub cpu_usage: f32,
    /// Number of active browser instances
    pub active_instances: usize,
}

impl SystemLoad {
    /// Check if system is under high load
    pub fn is_high_load(&self) -> bool {
        // Consider high load if:
        // - Less than 20% RAM available
        // - CPU usage > 80%
        let ram_usage_percent = ((self.total_ram - self.available_ram) as f64 / self.total_ram as f64) * 100.0;
        ram_usage_percent > 80.0 || self.cpu_usage > 80.0
    }

    /// Check if system can handle multi-instance mode
    pub fn can_handle_multi_instance(&self) -> bool {
        // Need at least 1GB available RAM and CPU < 70%
        self.available_ram > 1_000_000_000 && self.cpu_usage < 70.0
    }

    /// Get recommended max instances based on available resources
    pub fn recommended_max_instances(&self) -> usize {
        // Rough estimate: 400MB per instance
        let ram_based = (self.available_ram / 400_000_000) as usize;
        let cpu_based = if self.cpu_usage < 50.0 { 5 } else if self.cpu_usage < 70.0 { 3 } else { 1 };

        ram_based.min(cpu_based).max(1)
    }
}

/// Resource monitor for tracking system resources
pub struct ResourceMonitor {
    /// Current system load
    current_load: Arc<RwLock<SystemLoad>>,
}

impl ResourceMonitor {
    /// Create a new resource monitor
    pub fn new() -> Self {
        Self {
            current_load: Arc::new(RwLock::new(SystemLoad {
                available_ram: 0,
                total_ram: 0,
                cpu_usage: 0.0,
                active_instances: 0,
            })),
        }
    }

    /// Update system load information
    pub async fn update(&self) {
        let mut load = self.current_load.write().await;

        // Get system info using sysinfo crate (if available)
        // For now, use placeholder values
        #[cfg(feature = "sysinfo")]
        {
            use sysinfo::{System, SystemExt};
            let mut sys = System::new_all();
            sys.refresh_all();

            load.total_ram = sys.total_memory();
            load.available_ram = sys.available_memory();
            load.cpu_usage = sys.global_cpu_info().cpu_usage();
        }

        #[cfg(not(feature = "sysinfo"))]
        {
            // Fallback: assume reasonable defaults
            load.total_ram = 8_000_000_000; // 8GB
            load.available_ram = 4_000_000_000; // 4GB available
            load.cpu_usage = 30.0; // 30% CPU usage
        }
    }

    /// Get current system load
    pub async fn get_load(&self) -> SystemLoad {
        *self.current_load.read().await
    }

    /// Update active instance count
    pub async fn set_active_instances(&self, count: usize) {
        let mut load = self.current_load.write().await;
        load.active_instances = count;
    }

    /// Check if system is under high load
    pub async fn is_high_load(&self) -> bool {
        let load = self.current_load.read().await;
        load.is_high_load()
    }

    /// Check if system can handle multi-instance mode
    pub async fn can_handle_multi_instance(&self) -> bool {
        let load = self.current_load.read().await;
        load.can_handle_multi_instance()
    }

    /// Get recommended max instances
    pub async fn recommended_max_instances(&self) -> usize {
        let load = self.current_load.read().await;
        load.recommended_max_instances()
    }

    /// Get current active instance count
    pub async fn active_instances(&self) -> usize {
        let load = self.current_load.read().await;
        load.active_instances
    }
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_load_high_load_detection() {
        let load = SystemLoad {
            available_ram: 500_000_000, // 500MB
            total_ram: 8_000_000_000,   // 8GB
            cpu_usage: 85.0,
            active_instances: 2,
        };

        assert!(load.is_high_load());
    }

    #[test]
    fn test_system_load_normal_load() {
        let load = SystemLoad {
            available_ram: 4_000_000_000, // 4GB
            total_ram: 8_000_000_000,     // 8GB
            cpu_usage: 30.0,
            active_instances: 1,
        };

        assert!(!load.is_high_load());
        assert!(load.can_handle_multi_instance());
    }

    #[test]
    fn test_recommended_max_instances() {
        let load = SystemLoad {
            available_ram: 2_000_000_000, // 2GB
            total_ram: 8_000_000_000,     // 8GB
            cpu_usage: 40.0,
            active_instances: 0,
        };

        let max = load.recommended_max_instances();
        assert!(max >= 1 && max <= 5);
    }

    #[tokio::test]
    async fn test_resource_monitor_creation() {
        let monitor = ResourceMonitor::new();
        let load = monitor.get_load().await;
        assert_eq!(load.active_instances, 0);
    }

    #[tokio::test]
    async fn test_resource_monitor_update_instances() {
        let monitor = ResourceMonitor::new();
        monitor.set_active_instances(3).await;

        let load = monitor.get_load().await;
        assert_eq!(load.active_instances, 3);
    }
}
