/// Ceiling for a single backoff delay, so a high attempt count can't produce an
/// astronomically large (multi-day) wait from `base * 2^attempt`.
const MAX_DELAY_MS: u64 = 30_000;

pub struct ReconnectStrategy {
    pub max_attempts: u32,
    pub current_attempt: u32,
    pub base_delay_ms: u64,
}

impl ReconnectStrategy {
    pub fn new(max_attempts: u32, base_delay_ms: u64) -> Self {
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

    pub fn reset(&mut self) {
        self.current_attempt = 0;
    }
}
