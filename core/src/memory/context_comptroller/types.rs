use crate::memory::context::{MemoryEntry, MemoryFact};

/// Token budget for context window
#[derive(Debug, Clone)]
pub struct TokenBudget {
    pub total: usize,
    pub used: usize,
}

impl TokenBudget {
    pub fn new(total: usize) -> Self {
        Self { total, used: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.total.saturating_sub(self.used)
    }

    pub fn usage_percent(&self) -> f32 {
        (self.used as f32 / self.total as f32) * 100.0
    }
}

/// Arbitrated context after redundancy removal
#[derive(Debug, Clone)]
pub struct ArbitratedContext {
    pub facts: Vec<MemoryFact>,
    pub raw_memories: Vec<MemoryEntry>,
    pub tokens_saved: usize,
}

/// Retention mode for arbitration
#[derive(Debug, Clone, Copy)]
#[derive(Default)]
pub enum RetentionMode {
    PreferTranscript,  // Default: keep original text
    PreferFact,        // Space-constrained: keep compressed
    #[default]
    Hybrid,            // Mix based on importance
}

