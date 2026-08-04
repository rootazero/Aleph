/// Ceiling for a single backoff delay, so a high attempt count can't produce an
/// astronomically large (multi-day) wait from `base * 2^attempt`.
const MAX_DELAY_MS: u64 = 30_000;

pub struct ReconnectStrategy {
    pub max_attempts: u32,
    pub current_attempt: u32,
    pub base_delay_ms: u64,
}

impl ReconnectStrategy {
    #[must_use]
    pub const fn new(max_attempts: u32, base_delay_ms: u64) -> Self {
        Self {
            max_attempts,
            current_attempt: 0,
            base_delay_ms,
        }
    }

    pub fn next_delay(&mut self) -> Option<u64> {
        if self.current_attempt >= self.max_attempts {
            return None;
        }

        let delay = self
            .base_delay_ms
            .saturating_mul(2u64.saturating_pow(self.current_attempt))
            .min(MAX_DELAY_MS);
        self.current_attempt += 1;
        Some(delay)
    }

    /// Like [`next_delay`], but shaves a deterministic *downward* fraction off
    /// the delay to avoid every client re-connecting in lockstep after a server
    /// restart. `jitter_permille` is 0..=1000 (0 = no jitter, 100 = minus 10%).
    /// Only ever reduces the delay, so it can never exceed the backoff ceiling.
    pub fn next_delay_jittered(&mut self, jitter_permille: u64) -> Option<u64> {
        let base = self.next_delay()?;
        let permille = jitter_permille.min(1000);
        let cut = base.saturating_mul(permille) / 1000;
        Some(base.saturating_sub(cut))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_zero_equals_base_delay() {
        let mut s = ReconnectStrategy::new(5, 1000);
        assert_eq!(s.next_delay_jittered(0), Some(1000));
    }

    #[test]
    fn jitter_subtracts_proportional_fraction_only_downward() {
        let mut s = ReconnectStrategy::new(5, 1000);
        // 100 permille = 10% downward → 1000 - 100 = 900; never exceeds base.
        assert_eq!(s.next_delay_jittered(100), Some(900));
    }

    #[test]
    fn jitter_respects_attempt_exhaustion() {
        let mut s = ReconnectStrategy::new(1, 1000);
        assert!(s.next_delay_jittered(50).is_some());
        assert_eq!(s.next_delay_jittered(50), None);
    }
}
