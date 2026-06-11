use crate::memory::context::MemoryFact;

/// Token budget for context window
#[derive(Debug, Clone)]
pub struct TokenBudget {
    pub total: usize,
    pub used: usize,
}

impl TokenBudget {
    #[must_use]
    pub const fn new(total: usize) -> Self {
        Self { total, used: 0 }
    }

    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.total.saturating_sub(self.used)
    }

    #[must_use]
    pub fn usage_percent(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.used as f32 / self.total as f32) * 100.0
    }
}

/// Arbitrated context after redundancy removal
#[derive(Debug, Clone)]
pub struct ArbitratedContext {
    pub facts: Vec<MemoryFact>,
    pub tokens_saved: usize,
}

/// Retention mode for arbitration
#[derive(Debug, Clone, Copy, Default)]
pub enum RetentionMode {
    PreferTranscript, // Default: keep original text
    PreferFact,       // Space-constrained: keep compressed
    #[default]
    Hybrid, // Mix based on importance
}
