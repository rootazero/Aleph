//! Cooldown backoff calculation for auth profile failures.

/// Calculate cooldown duration for rate limit errors.
///
/// Uses base-5 exponential backoff:
/// - 1st error: 1 minute
/// - 2nd error: 5 minutes
/// - 3rd error: 25 minutes
/// - 4th+ error: 1 hour (max)
#[must_use]
pub fn calculate_cooldown_ms(error_count: u32) -> u64 {
    let normalized = error_count.max(1);
    let exponent = (normalized - 1).min(3);
    let base_ms = 60 * 1000u64; // 1 minute
    let max_ms = 60 * 60 * 1000u64; // 1 hour

    (base_ms * 5u64.pow(exponent)).min(max_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cooldown_calculation() {
        // Rate limit: 5^n minutes
        assert_eq!(calculate_cooldown_ms(1), 60_000); // 1 min
        assert_eq!(calculate_cooldown_ms(2), 300_000); // 5 min
        assert_eq!(calculate_cooldown_ms(3), 1_500_000); // 25 min
        assert_eq!(calculate_cooldown_ms(4), 3_600_000); // 1 hour (max)
        assert_eq!(calculate_cooldown_ms(10), 3_600_000); // Still max
    }
}
