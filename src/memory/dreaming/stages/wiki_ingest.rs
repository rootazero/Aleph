//! WikiIngestStage: previously compiled facts into wiki pages via the knowledge graph.
//!
//! This stage is now a no-op. Knowledge Notes (via NoteIndexer and CompressionService::compress_to_notes)
//! are the single source of truth for structured knowledge. The graph-based wiki compilation
//! pipeline has been replaced.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;

/// Configuration for wiki ingestion during dreams (retained for config compat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiIngestConfig {
    pub enabled: bool,
    pub max_pages_per_run: usize,
    pub cooldown_days: u32,
    pub min_node_score: f32,
    pub min_facts_for_compile: usize,
    pub stale_threshold: f32,
}

impl Default for WikiIngestConfig {
    fn default() -> Self {
        Self {
            enabled: false, // disabled — Notes are the replacement
            max_pages_per_run: 10,
            cooldown_days: 1,
            min_node_score: 0.5,
            min_facts_for_compile: 3,
            stale_threshold: 0.5,
        }
    }
}

/// No-op: wiki ingestion is deprecated. Knowledge Notes handle this natively.
pub struct WikiIngestStage;

#[async_trait]
impl DreamStage for WikiIngestStage {
    fn name(&self) -> &'static str {
        "wiki_ingest"
    }

    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        false // Disabled — Knowledge Notes are the single source of truth
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        debug!("WikiIngestStage: skipped (deprecated, replaced by Knowledge Notes)");
        Ok(ctx)
    }
}

/// Convert an entity name to a URL-safe slug (retained for backward compat).
#[cfg(test)]
fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
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
        assert!(!config.enabled);
        assert_eq!(config.max_pages_per_run, 10);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Rust Ownership"), "rust-ownership");
        assert_eq!(slugify("LLM/GPT-4"), "llm-gpt-4");
        assert_eq!(slugify("hello"), "hello");
    }
}
