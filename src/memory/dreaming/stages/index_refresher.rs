//! `IndexRefresherStage` — idempotent full rebuild of `index.md` + log rotation.

use crate::error::AlephError;
use crate::memory::dreaming::stages::DreamStage;
use crate::memory::dreaming::DreamContext;
use async_trait::async_trait;

pub struct IndexRefresherStage;

#[async_trait]
impl DreamStage for IndexRefresherStage {
    fn name(&self) -> &'static str {
        "index_refresher"
    }

    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        true
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        if let Some(w) = ctx.orientation.as_ref() {
            let stats = w.rebuild_index(&ctx.agent_id).await?;
            tracing::info!(
                agent = %ctx.agent_id,
                notes_indexed = stats.notes_indexed,
                wiki_bytes = stats.bytes_written,
                "IndexRefresher rebuilt index.md"
            );
            w.rotate_log_if_needed(&ctx.agent_id).await?;
        }
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn name_is_stable() {
        let s = IndexRefresherStage;
        assert_eq!(s.name(), "index_refresher");
    }
    // Behaviour-level integration test lives in tests/memory_note_orientation.rs (Task 13).
}
