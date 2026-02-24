//! Resource-aware scheduling for cron jobs.
//!
//! Adjusts concurrency based on system CPU load to prevent
//! AI tasks from overwhelming the host machine.

/// Resolve effective concurrency based on system load.
///
/// - CPU > 80%: limit to 1 (only highest priority jobs)
/// - CPU > 60%: half of configured max
/// - Otherwise: full configured max
///
/// Result is clamped to available semaphore permits.
pub fn resolve_effective_concurrency(
    config_max: usize,
    available_permits: usize,
) -> usize {
    let cpu = get_cpu_usage();

    let limit = if cpu > 0.8 {
        1
    } else if cpu > 0.6 {
        (config_max / 2).max(1)
    } else {
        config_max
    };

    limit.max(1).min(available_permits)
}

/// Get current CPU usage as a fraction (0.0 - 1.0).
#[cfg(feature = "cron")]
fn get_cpu_usage() -> f64 {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_cpu_usage();
    std::thread::sleep(std::time::Duration::from_millis(200));
    sys.refresh_cpu_usage();

    let cpus = sys.cpus();
    if cpus.is_empty() {
        return 0.0;
    }

    let usage: f64 = cpus.iter().map(|c| c.cpu_usage() as f64).sum::<f64>()
        / cpus.len() as f64
        / 100.0;

    usage.clamp(0.0, 1.0)
}

#[cfg(not(feature = "cron"))]
fn get_cpu_usage() -> f64 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concurrency_minimum_one() {
        let result = resolve_effective_concurrency(1, 1);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_concurrency_clamped_to_permits() {
        let result = resolve_effective_concurrency(10, 2);
        assert!(result <= 2);
    }

    #[test]
    fn test_concurrency_returns_at_least_one() {
        let result = resolve_effective_concurrency(5, 10);
        assert!(result >= 1);
    }
}
