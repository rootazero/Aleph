//! Resource quota tracking and enforcement

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use crate::dispatcher::DEFAULT_MAX_FILE_SIZE;

/// Resource quota limits
#[derive(Debug, Clone)]
pub struct ResourceQuota {
    /// Maximum single file size (bytes)
    pub max_file_size: u64,
    /// Maximum total read (bytes)
    pub max_total_read: u64,
    /// Maximum total write (bytes)
    pub max_total_write: u64,
    /// Maximum file count
    pub max_file_count: u32,
    /// Operation timeout
    pub operation_timeout: Duration,
}

impl Default for ResourceQuota {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_total_read: 100 * 1024 * 1024, // 100 MB
            max_total_write: 50 * 1024 * 1024, // 50 MB
            max_file_count: 1000,
            operation_timeout: Duration::from_secs(30),
        }
    }
}

/// Error when quota is exceeded
#[derive(Debug, Clone)]
pub enum QuotaExceeded {
    FileTooLarge { size: u64, max: u64 },
    TotalReadExceeded { used: u64, requested: u64, max: u64 },
    TotalWriteExceeded { used: u64, requested: u64, max: u64 },
    FileCountExceeded { count: u32, max: u32 },
}

impl std::fmt::Display for QuotaExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuotaExceeded::FileTooLarge { size, max } => {
                write!(f, "File too large: {} bytes (max: {})", size, max)
            }
            QuotaExceeded::TotalReadExceeded {
                used,
                requested,
                max,
            } => {
                write!(f, "Total read exceeded: {} + {} > {}", used, requested, max)
            }
            QuotaExceeded::TotalWriteExceeded {
                used,
                requested,
                max,
            } => {
                write!(
                    f,
                    "Total write exceeded: {} + {} > {}",
                    used, requested, max
                )
            }
            QuotaExceeded::FileCountExceeded { count, max } => {
                write!(f, "File count exceeded: {} >= {}", count, max)
            }
        }
    }
}

impl std::error::Error for QuotaExceeded {}

/// Tracks resource usage against quotas
#[derive(Debug)]
pub struct QuotaTracker {
    quota: ResourceQuota,
    used_read: AtomicU64,
    used_write: AtomicU64,
    file_count: AtomicU32,
}

impl QuotaTracker {
    /// Create a new tracker with the given quota
    pub fn new(quota: ResourceQuota) -> Self {
        Self {
            quota,
            used_read: AtomicU64::new(0),
            used_write: AtomicU64::new(0),
            file_count: AtomicU32::new(0),
        }
    }

    /// Check if a read operation is allowed
    pub fn check_read(&self, size: u64) -> Result<(), QuotaExceeded> {
        // Check single file size
        if size > self.quota.max_file_size {
            return Err(QuotaExceeded::FileTooLarge {
                size,
                max: self.quota.max_file_size,
            });
        }

        // Check total read
        let used = self.used_read.load(Ordering::Relaxed);
        if used + size > self.quota.max_total_read {
            return Err(QuotaExceeded::TotalReadExceeded {
                used,
                requested: size,
                max: self.quota.max_total_read,
            });
        }

        Ok(())
    }

    /// Check if a write operation is allowed
    pub fn check_write(&self, size: u64) -> Result<(), QuotaExceeded> {
        // Check single file size
        if size > self.quota.max_file_size {
            return Err(QuotaExceeded::FileTooLarge {
                size,
                max: self.quota.max_file_size,
            });
        }

        // Check total write
        let used = self.used_write.load(Ordering::Relaxed);
        if used + size > self.quota.max_total_write {
            return Err(QuotaExceeded::TotalWriteExceeded {
                used,
                requested: size,
                max: self.quota.max_total_write,
            });
        }

        Ok(())
    }

    /// Check if file count is within limit
    pub fn check_file_count(&self) -> Result<(), QuotaExceeded> {
        let count = self.file_count.load(Ordering::Relaxed);
        if count >= self.quota.max_file_count {
            return Err(QuotaExceeded::FileCountExceeded {
                count,
                max: self.quota.max_file_count,
            });
        }
        Ok(())
    }

    /// Record a read operation
    pub fn record_read(&self, size: u64) {
        self.used_read.fetch_add(size, Ordering::Relaxed);
    }

    /// Record a write operation
    pub fn record_write(&self, size: u64) {
        self.used_write.fetch_add(size, Ordering::Relaxed);
    }

    /// Record a file access
    pub fn record_file_access(&self) {
        self.file_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get current usage statistics
    pub fn usage(&self) -> QuotaUsage {
        QuotaUsage {
            read_bytes: self.used_read.load(Ordering::Relaxed),
            write_bytes: self.used_write.load(Ordering::Relaxed),
            file_count: self.file_count.load(Ordering::Relaxed),
        }
    }

    /// Reset usage counters
    pub fn reset(&self) {
        self.used_read.store(0, Ordering::Relaxed);
        self.used_write.store(0, Ordering::Relaxed);
        self.file_count.store(0, Ordering::Relaxed);
    }
}

/// Current quota usage
#[derive(Debug, Clone)]
pub struct QuotaUsage {
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub file_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_check_read() {
        let quota = ResourceQuota::default();
        let tracker = QuotaTracker::new(quota);

        assert!(tracker.check_read(1024).is_ok());
        assert!(tracker.check_read(200 * 1024 * 1024).is_err()); // 200MB > 100MB limit
    }

    #[test]
    fn test_quota_tracking() {
        let quota = ResourceQuota {
            max_total_read: 1000,
            ..Default::default()
        };
        let tracker = QuotaTracker::new(quota);

        tracker.record_read(500);
        assert!(tracker.check_read(400).is_ok());
        assert!(tracker.check_read(600).is_err()); // 500 + 600 > 1000
    }

    #[test]
    fn test_quota_file_count() {
        let quota = ResourceQuota {
            max_file_count: 5,
            ..Default::default()
        };
        let tracker = QuotaTracker::new(quota);

        for _ in 0..5 {
            tracker.record_file_access();
        }

        assert!(tracker.check_file_count().is_err());
    }
}
