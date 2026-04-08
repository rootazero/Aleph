//! DecayStage: applies Ebbinghaus decay to memory facts and graph.

use async_trait::async_trait;
use tracing::{debug, info};

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::events::commands::ApplyDecayCommand;
use crate::memory::store::MemoryStore;

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

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // 1. Apply graph decay
        let graph_report = ctx.graph_store.apply_decay(&ctx.graph_decay_config).await?;
        debug!(
            pruned_nodes = graph_report.pruned_nodes,
            pruned_edges = graph_report.pruned_edges,
            "DecayStage: graph decay applied"
        );

        // 2. Apply fact decay with Ebbinghaus exponential curve
        let half_life_days = ctx.memory_decay_config.half_life_days;
        let _min_strength = ctx.memory_decay_config.min_strength;
        let now_ts = crate::memory::dreaming::now_timestamp();

        // Optionally emit decay events via command_handler for audit trail
        if let Some(handler) = &ctx.command_handler {
            let valid_facts = ctx.database.get_all_facts(false, None).await?;
            let decay_tuples: Vec<(String, f32, f32)> = valid_facts
                .iter()
                .filter_map(|fact| {
                    // Ebbinghaus exponential decay: exp(-t * ln(2) / half_life)
                    let last_access = fact.last_accessed_at.unwrap_or(fact.updated_at);
                    let days_since_access = (now_ts - last_access) as f64 / 86400.0;
                    let decay = (-(days_since_access) * (2.0_f64.ln()) / half_life_days as f64)
                        .exp() as f32;
                    let new_strength = fact.strength * decay;
                    // Only record facts that actually change.
                    if (new_strength - fact.strength).abs() > f32::EPSILON {
                        Some((fact.id.clone(), fact.strength, new_strength))
                    } else {
                        None
                    }
                })
                .collect();

            if !decay_tuples.is_empty() {
                let _ = handler
                    .apply_decay(ApplyDecayCommand {
                        fact_ids_with_strength: decay_tuples,
                        decay_factor: (-(2.0_f64.ln()) / half_life_days as f64).exp() as f32,
                        correlation_id: None,
                    })
                    .await?;
            }
        }

        // 3. Apply tiered fact decay — each tier has its own half-life
        //    ShortTerm: 1 day (aggressive cleanup of session facts)
        //    LongTerm: 30 days (gradual decay of extracted facts)
        //    Core: 365 days (near-permanent identity facts)
        let tiers: [(&str, f64, f32); 3] = [
            ("short_term", 1.0, 0.1),
            ("long_term", 30.0, 0.05),
            ("core", 365.0, 0.01),
        ];

        let mut total_decayed: usize = 0;
        for (tier, tier_half_life, tier_min_str) in &tiers {
            let count = ctx
                .database
                .apply_fact_decay_by_tier(tier, *tier_half_life, *tier_min_str)
                .await?;
            if count > 0 {
                info!(tier, decayed = count, half_life = *tier_half_life, "Tiered decay applied");
            }
            total_decayed += count;
        }

        info!(
            decayed_count = total_decayed,
            graph_pruned_nodes = graph_report.pruned_nodes,
            graph_pruned_edges = graph_report.pruned_edges,
            "DecayStage: decay cycle complete"
        );

        ctx.graph_decay_report = Some(graph_report);
        ctx.memory_decay_report = Some(MemoryDecayReport {
            updated_facts: total_decayed as u64,
            pruned_facts: 0,
        });

        Ok(ctx)
    }
}
