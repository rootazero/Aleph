//! ConsolidateStage: promotes STM facts to LTM.

use async_trait::async_trait;

use crate::error::AlephError;
use super::{DreamStage, DreamContext};

/// Consolidates short-term memory facts into long-term memory.
pub struct ConsolidateStage;

#[async_trait]
impl DreamStage for ConsolidateStage {
    fn name(&self) -> &'static str {
        "consolidate"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // Placeholder: pass-through. Implementation in Task 8.
        Ok(ctx)
    }
}
