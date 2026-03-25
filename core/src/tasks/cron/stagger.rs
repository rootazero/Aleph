//! Hash-based stagger spreading for cron jobs.
//!
//! Prevents "thundering herd" when many jobs share the same cron expression
//! (e.g., `0 * * * *`). Each job gets a deterministic offset based on
//! its ID's SHA-256 hash, spreading execution across a configurable window.

use sha2::{Digest, Sha256};

/// Compute a deterministic stagger offset for a job ID.
///
/// SHA-256 hashes the `job_id`, takes the first 4 bytes as a big-endian u32,
/// then returns `u32 % stagger_ms`. Returns 0 if `stagger_ms <= 0`.
pub fn compute_stagger_offset(job_id: &str, stagger_ms: i64) -> i64 {
    if stagger_ms <= 0 {
        return 0;
    }

    let hash = Sha256::digest(job_id.as_bytes());
    let head = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]);
    (head as i64) % stagger_ms
}

/// Compute the next staggered execution time for a job.
///
/// - If `stagger_ms <= 0`, returns `cron_next_ms` unchanged (passthrough).
/// - Otherwise, adds a deterministic offset to `cron_next_ms`.
/// - If the result is still in the future (`> now_ms`), returns it.
/// - Otherwise, advances by one full `stagger_ms` window.
pub fn compute_staggered_next(
    job_id: &str,
    cron_next_ms: i64,
    stagger_ms: i64,
    now_ms: i64,
) -> i64 {
    if stagger_ms <= 0 {
        return cron_next_ms;
    }

    let offset = compute_stagger_offset(job_id, stagger_ms);
    let staggered = cron_next_ms + offset;

    if staggered > now_ms {
        staggered
    } else {
        cron_next_ms + stagger_ms + offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stagger_deterministic() {
        let a = compute_stagger_offset("job-alpha", 10_000);
        let b = compute_stagger_offset("job-alpha", 10_000);
        assert_eq!(a, b, "same ID must produce same offset");
    }

    #[test]
    fn stagger_within_range() {
        let window = 60_000; // 60 seconds
        for id in &["a", "b", "c", "daily-report", "weekly-cleanup", "sync-42"] {
            let offset = compute_stagger_offset(id, window);
            assert!(
                offset >= 0 && offset < window,
                "offset {offset} out of range [0, {window}) for id '{id}'"
            );
        }
    }

    #[test]
    fn stagger_different_ids_likely_differ() {
        let a = compute_stagger_offset("job-one", 1_000_000);
        let b = compute_stagger_offset("job-two", 1_000_000);
        assert_ne!(a, b, "different IDs should (very likely) produce different offsets");
    }

    #[test]
    fn stagger_zero_window() {
        assert_eq!(compute_stagger_offset("anything", 0), 0);
    }

    #[test]
    fn stagger_negative_window() {
        assert_eq!(compute_stagger_offset("anything", -100), 0);
    }

    #[test]
    fn staggered_next_future() {
        let now = 1_000_000;
        let cron_next = 1_100_000;
        let stagger = 60_000;
        let result = compute_staggered_next("my-job", cron_next, stagger, now);
        assert!(result > now, "result {result} should be > now {now}");
        assert!(
            result >= cron_next && result < cron_next + stagger,
            "result {result} should be in [{cron_next}, {})",
            cron_next + stagger
        );
    }

    #[test]
    fn staggered_next_past_advances_window() {
        // cron_next is in the past relative to now; advancing one window must land in the future
        let now = 2_000_000;
        let cron_next = 1_950_000;
        let stagger = 60_000;
        let result = compute_staggered_next("my-job", cron_next, stagger, now);
        assert!(
            result > now,
            "result {result} should be > now {now} after advancing window"
        );
    }

    #[test]
    fn staggered_next_zero_stagger_passthrough() {
        let result = compute_staggered_next("any-job", 5_000_000, 0, 1_000_000);
        assert_eq!(result, 5_000_000, "zero stagger should return cron_next unchanged");
    }
}
