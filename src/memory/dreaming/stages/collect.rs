//! CollectStage: gathers recent memories for the dream pipeline.

use async_trait::async_trait;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::dreaming::now_timestamp;
use crate::memory::namespace::NamespaceScope;
/// Collects recent memories from the database into the pipeline context.
///
/// With SessionStore removed, this stage is a no-op — raw memories
/// are no longer stored in SQLite.
pub struct CollectStage;

#[async_trait]
impl DreamStage for CollectStage {
    fn name(&self) -> &'static str {
        "collect"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let _ = now_timestamp();
        let _ = NamespaceScope::Owner;
        // Raw memory collection removed — SessionStore no longer exists.
        info!(count = 0, "CollectStage: no raw memories (SessionStore removed)");
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_stage_name() {
        let stage = CollectStage;
        assert_eq!(stage.name(), "collect");
    }
}
