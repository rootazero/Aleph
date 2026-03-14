//! Test harness for memory probe tests.
//!
//! Provides helpers to construct MemoryFact instances with
//! controlled timestamps, tiers, access counts, and embeddings.

use alephcore::memory::context::{
    FactSource, FactType, MemoryFact, MemoryTier,
};
use alephcore::memory::decay::MemoryStrength;
use alephcore::memory::scoring_pipeline::{ScoringContext, ScoringPipeline, ScoringPipelineConfig};
use alephcore::memory::store::types::ScoredFact;

use super::mock_embedding;

/// Seconds per day.
pub const DAY: i64 = 86400;

/// Reference "now" timestamp for deterministic tests.
pub const T0: i64 = 1_700_000_000;

/// Create a `MemoryFact` with controlled test values.
pub fn make_fact(content: &str, fact_type: FactType, tier: MemoryTier) -> MemoryFact {
    MemoryFact::new(content.to_string(), fact_type, vec![])
        .with_tier(tier)
        .with_confidence(1.0)
        .with_created_at(T0)
        .with_fact_source(FactSource::Extracted)
}

/// Create a `MemoryFact` with a deterministic embedding attached.
pub fn make_fact_with_embedding(
    content: &str,
    fact_type: FactType,
    tier: MemoryTier,
) -> MemoryFact {
    let embedding = mock_embedding::embed(content, mock_embedding::DEFAULT_DIM);
    make_fact(content, fact_type, tier).with_embedding(embedding)
}

/// Create a `ScoredFact` wrapper.
pub fn scored(content: &str, score: f32, fact_type: FactType, tier: MemoryTier) -> ScoredFact {
    let mut fact = make_fact(content, fact_type, tier);
    fact.created_at = T0;
    fact.confidence = 1.0;
    ScoredFact { fact, score }
}

/// Build a default `ScoringContext` for pipeline tests.
pub fn default_ctx() -> ScoringContext {
    ScoringContext {
        query: "test query".to_string(),
        query_embedding: None,
        timestamp: T0,
        config: ScoringPipelineConfig::default(),
    }
}

/// Build the default 7-stage scoring pipeline.
pub fn default_pipeline() -> ScoringPipeline {
    ScoringPipeline::default()
}

/// Build a `MemoryStrength` with controlled values.
pub fn make_strength(access_count: u32, last_accessed: i64, creation_time: i64) -> MemoryStrength {
    MemoryStrength {
        access_count,
        last_accessed,
        creation_time,
    }
}
