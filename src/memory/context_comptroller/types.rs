use crate::memory::context::MemoryFact;

/// Token budget for context window
#[derive(Debug, Clone)]
pub struct TokenBudget {
    pub total: usize,
}

impl TokenBudget {
    #[must_use]
    pub const fn new(total: usize) -> Self {
        Self { total }
    }
}

/// Arbitrated context after redundancy removal
#[derive(Debug, Clone)]
pub struct ArbitratedContext {
    pub facts: Vec<MemoryFact>,
    pub tokens_saved: usize,
}
