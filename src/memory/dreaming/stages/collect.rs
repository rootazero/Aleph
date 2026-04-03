//! CollectStage: gathers recent memories for the dream pipeline.

use async_trait::async_trait;

use crate::error::AlephError;
use super::{DreamStage, DreamContext};

/// Collects recent memories from the database into the pipeline context.
pub struct CollectStage;

#[async_trait]
impl DreamStage for CollectStage {
    fn name(&self) -> &'static str {
        "collect"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // Placeholder: pass-through. Implementation in Task 3.
        Ok(ctx)
    }
}
