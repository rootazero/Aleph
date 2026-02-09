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

        let delay = self.base_delay_ms * 2u64.pow(self.current_attempt);
        self.current_attempt += 1;
        Some(delay)
    }

    pub fn reset(&mut self) {
        self.current_attempt = 0;
    }
}
