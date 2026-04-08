//! ConsolidateStage: promotes STM facts to LTM and prunes weak facts.
//!
//! Iterates all valid facts and applies two checks:
//! - **Consolidation**: ShortTerm facts with access_count >= 2 AND strength >= threshold
//!   are upgraded to LongTerm. This prevents freshly-created facts (default strength 1.0)
//!   from being promoted before they've been actually used in conversations.
//! - **Pruning**: non-Core facts with strength < pruning threshold are invalidated.

use async_trait::async_trait;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::MemoryTier;
use crate::memory::dreaming::{should_consolidate, should_prune, ConsolidationPipelineConfig};
use crate::memory::store::MemoryStore;

/// Consolidates short-term memory facts into long-term memory.
pub struct ConsolidateStage;

#[async_trait]
impl DreamStage for ConsolidateStage {
    fn name(&self) -> &'static str {
        "consolidate"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let config = ConsolidationPipelineConfig::default();

        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let facts: Vec<_> = all_facts
            .into_iter()
            .take(config.max_facts_per_run)
            .collect();

        let mut consolidated_count: usize = 0;
        let mut pruned_count: usize = 0;

        for mut fact in facts {
            if should_prune(&fact, config.pruning_threshold) {
                ctx.database
                    .invalidate_fact(&fact.id, "strength below pruning threshold")
                    .await?;
                pruned_count += 1;
            } else if should_consolidate(&fact, config.strength_threshold) {
                fact.tier = MemoryTier::LongTerm;
                ctx.database.update_fact(&fact).await?;
                consolidated_count += 1;
            }
        }

        info!(
            consolidated = consolidated_count,
            pruned = pruned_count,
            "ConsolidateStage: STM→LTM consolidation complete"
        );

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consolidate_stage_name() {
        assert_eq!(ConsolidateStage.name(), "consolidate");
    }
}
