//! DecayStage: applies Ebbinghaus decay to memory facts and graph.

use async_trait::async_trait;

use crate::error::AlephError;
use super::{DreamStage, DreamContext};

/// Memory decay summary report.
#[derive(Debug, Clone, Default)]
pub struct MemoryDecayReport {
    pub updated_facts: u64,
    pub pruned_facts: u64,
}

/// Applies time-based decay to memory facts and the knowledge graph.
pub struct DecayStage;

#[async_trait]
impl DreamStage for DecayStage {
    fn name(&self) -> &'static str {
        "decay"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // Placeholder: pass-through. Implementation in Task 8.
        Ok(ctx)
    }
}
