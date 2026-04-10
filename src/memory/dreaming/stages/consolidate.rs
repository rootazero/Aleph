//! ConsolidateStage: promotes STM facts to LTM and prunes weak facts.
//!
//! Iterates all valid ShortTerm facts aged >= 24h and applies:
//! - **Pruning**: non-Core facts with strength < pruning threshold are invalidated.
//! - **Promotion**: STM facts that meet the strength threshold are promoted to LongTerm.
//!
//! NOTE: The previous 8-dimensional algorithmic scorer (PromotionScorer) has been
//! removed as part of the LLM sovereignty refactor. Task 8 will replace this with
//! LLM-based promotion judgment.

use async_trait::async_trait;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::MemoryTier;
use crate::memory::dreaming::{now_timestamp, should_prune, ConsolidationPipelineConfig};
use crate::memory::store::MemoryStore;

/// Consolidates short-term memory facts into long-term memory.
/// Promotion logic will be replaced by LLM-based scoring in Task 8.
pub struct ConsolidateStage;

#[async_trait]
impl DreamStage for ConsolidateStage {
    fn name(&self) -> &'static str {
        "consolidate"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let config = ConsolidationPipelineConfig::default();
        let now = now_timestamp();
        let min_age_secs: i64 = 24 * 3600; // 24 hours

        // 1. Get all valid facts
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let facts: Vec<_> = all_facts
            .into_iter()
            .take(config.max_facts_per_run)
            .collect();

        let mut consolidated_count: usize = 0;
        let mut pruned_count: usize = 0;

        for mut fact in facts {
            // Pruning check: remove weak non-Core facts
            if should_prune(&fact, config.pruning_threshold) {
                ctx.database
                    .invalidate_fact(&fact.id, "strength below pruning threshold")
                    .await?;
                pruned_count += 1;
                continue;
            }

            // Only consider ShortTerm facts aged >= min_age
            if fact.tier != MemoryTier::ShortTerm || (now - fact.created_at) < min_age_secs {
                continue;
            }

            // Stub promotion: promote facts that exceed the strength threshold.
            // Task 8 will replace this with LLM-based judgment.
            if fact.strength >= config.strength_threshold {
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
