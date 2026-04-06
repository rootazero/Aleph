//! CollectStage: gathers recent memories for the dream pipeline.

use async_trait::async_trait;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::dreaming::now_timestamp;
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::SessionStore;

const DEFAULT_LOOKBACK_HOURS: i64 = 24;
const DEFAULT_MAX_MEMORIES: usize = 500;

/// Collects recent memories from the database into the pipeline context.
pub struct CollectStage;

#[async_trait]
impl DreamStage for CollectStage {
    fn name(&self) -> &'static str {
        "collect"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let now = now_timestamp();
        let since = now - DEFAULT_LOOKBACK_HOURS * 3600;
        let memories = ctx
            .database
            .get_memories_since(since, &NamespaceScope::Owner, "default")
            .await?;
        ctx.memories = memories.into_iter().take(DEFAULT_MAX_MEMORIES).collect();
        info!(
            count = ctx.memories.len(),
            "CollectStage: gathered memories"
        );
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::MemoryEntry;

    /// CollectStage should report the correct name.
    #[test]
    fn collect_stage_name() {
        let stage = CollectStage;
        assert_eq!(stage.name(), "collect");
    }

    /// DEFAULT_LOOKBACK_HOURS and DEFAULT_MAX_MEMORIES have sensible values.
    #[test]
    fn constants_are_sensible() {
        assert_eq!(DEFAULT_LOOKBACK_HOURS, 24);
        assert_eq!(DEFAULT_MAX_MEMORIES, 500);
    }
}
