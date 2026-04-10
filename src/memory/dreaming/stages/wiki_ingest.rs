//! WikiIngestStage: passive ingestion of unprocessed Document facts into wiki pages.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::{FactSource, FactType};
use crate::memory::store::MemoryStore;

/// Configuration for wiki ingestion during dreams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiIngestConfig {
    pub enabled: bool,
    pub max_pages_per_run: usize,
    pub cooldown_days: u32,
}

impl Default for WikiIngestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pages_per_run: 10,
            cooldown_days: 1,
        }
    }
}

/// Passively ingests unprocessed Document facts into wiki pages.
pub struct WikiIngestStage;

#[async_trait]
impl DreamStage for WikiIngestStage {
    fn name(&self) -> &'static str {
        "wiki_ingest"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.provider.is_some()
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let config = WikiIngestConfig::default();
        if !config.enabled {
            return Ok(ctx);
        }

        let all_facts = ctx.database.get_all_facts(false, None).await?;

        let document_facts: Vec<_> = all_facts
            .iter()
            .filter(|f| f.fact_source == FactSource::Document)
            .collect();

        let wiki_facts: Vec<_> = all_facts
            .iter()
            .filter(|f| f.fact_type == FactType::Wiki)
            .collect();

        let wiki_source_ids: std::collections::HashSet<&str> = wiki_facts
            .iter()
            .flat_map(|f| f.source_memory_ids.iter().map(|s| s.as_str()))
            .collect();

        let unprocessed: Vec<_> = document_facts
            .iter()
            .filter(|f| !wiki_source_ids.contains(f.id.as_str()))
            .take(config.max_pages_per_run)
            .collect();

        let unprocessed_count = unprocessed.len();

        if unprocessed_count == 0 {
            info!("WikiIngestStage: no unprocessed documents found");
            return Ok(ctx);
        }

        info!(
            unprocessed = unprocessed_count,
            "WikiIngestStage: found unprocessed documents (LLM ingestion pending)"
        );

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_ingest_stage_name() {
        assert_eq!(WikiIngestStage.name(), "wiki_ingest");
    }

    #[test]
    fn wiki_ingest_config_defaults() {
        let config = WikiIngestConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_pages_per_run, 10);
        assert_eq!(config.cooldown_days, 1);
    }
}
