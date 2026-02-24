# Memory Module Optimization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a multi-stage scoring pipeline, embedding cache, adaptive retrieval gate, noise filter, dual decay, storage deduplication, and JSONL backup for Aleph's memory module.

**Architecture:** Configurable scoring pipeline attached after hybrid retrieval's RRF fusion. Each stage implements a `ScoringStage` trait. Embedding cache wraps `SmartEmbedder` with LRU+TTL. Adaptive gate and noise filter are standalone modules. All features are config-driven and can be toggled off.

**Tech Stack:** Rust, LanceDB, fastembed (multilingual-e5-small), tokio, lru crate, sha2 crate.

**Design Doc:** `docs/plans/2026-02-24-memory-optimization-design.md`

---

## Task 1: Scoring Pipeline Config

**Files:**
- Create: `core/src/memory/scoring_pipeline/config.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing test**

Create the config file with tests that reference the struct:

```rust
// core/src/memory/scoring_pipeline/config.rs
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

/// Configuration for the multi-stage scoring pipeline.
///
/// Each field controls a specific scoring stage. Set weights to 0.0
/// or thresholds to extremes to effectively disable a stage.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScoringPipelineConfig {
    /// Master switch for the scoring pipeline.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Blend ratio for cosine reranking: `blended = (1-blend)*original + blend*cosine`.
    #[serde(default = "default_rerank_blend")]
    pub rerank_blend: f32,

    /// Half-life (days) for the recency boost exponential.
    #[serde(default = "default_recency_half_life_days")]
    pub recency_half_life_days: f32,

    /// Maximum additive recency bonus.
    #[serde(default = "default_recency_weight")]
    pub recency_weight: f32,

    /// Anchor character count for length normalization.
    #[serde(default = "default_length_norm_anchor")]
    pub length_norm_anchor: usize,

    /// Half-life (days) for the time-decay multiplicative penalty.
    #[serde(default = "default_time_decay_half_life_days")]
    pub time_decay_half_life_days: f32,

    /// Candidates scoring below this are discarded.
    #[serde(default = "default_hard_min_score")]
    pub hard_min_score: f32,

    /// Cosine-similarity threshold for MMR diversity deduplication.
    #[serde(default = "default_mmr_similarity_threshold")]
    pub mmr_similarity_threshold: f32,
}

fn default_enabled() -> bool { true }
fn default_rerank_blend() -> f32 { 0.3 }
fn default_recency_half_life_days() -> f32 { 14.0 }
fn default_recency_weight() -> f32 { 0.1 }
fn default_length_norm_anchor() -> usize { 500 }
fn default_time_decay_half_life_days() -> f32 { 60.0 }
fn default_hard_min_score() -> f32 { 0.35 }
fn default_mmr_similarity_threshold() -> f32 { 0.85 }

impl Default for ScoringPipelineConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            rerank_blend: default_rerank_blend(),
            recency_half_life_days: default_recency_half_life_days(),
            recency_weight: default_recency_weight(),
            length_norm_anchor: default_length_norm_anchor(),
            time_decay_half_life_days: default_time_decay_half_life_days(),
            hard_min_score: default_hard_min_score(),
            mmr_similarity_threshold: default_mmr_similarity_threshold(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = ScoringPipelineConfig::default();
        assert!(cfg.enabled);
        assert!((cfg.rerank_blend - 0.3).abs() < f32::EPSILON);
        assert!((cfg.recency_half_life_days - 14.0).abs() < f32::EPSILON);
        assert!((cfg.recency_weight - 0.1).abs() < f32::EPSILON);
        assert_eq!(cfg.length_norm_anchor, 500);
        assert!((cfg.time_decay_half_life_days - 60.0).abs() < f32::EPSILON);
        assert!((cfg.hard_min_score - 0.35).abs() < f32::EPSILON);
        assert!((cfg.mmr_similarity_threshold - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn test_deserialize_partial_toml() {
        let toml_str = r#"
            enabled = true
            hard_min_score = 0.5
        "#;
        let cfg: ScoringPipelineConfig = toml::from_str(toml_str).unwrap();
        assert!((cfg.hard_min_score - 0.5).abs() < f32::EPSILON);
        // Other fields should be defaults
        assert!((cfg.rerank_blend - 0.3).abs() < f32::EPSILON);
    }
}
```

**Step 2: Create directory and verify compilation**

Run: `mkdir -p core/src/memory/scoring_pipeline && cargo test -p alephcore --lib memory::scoring_pipeline::config::tests -- --nocapture`
Expected: PASS (2 tests)

**Step 3: Commit**

```bash
git add core/src/memory/scoring_pipeline/config.rs
git commit -m "memory: add ScoringPipelineConfig with defaults and serde"
```

---

## Task 2: Scoring Pipeline Context & Trait

**Files:**
- Create: `core/src/memory/scoring_pipeline/context.rs`
- Create: `core/src/memory/scoring_pipeline/mod.rs`
- Create: `core/src/memory/scoring_pipeline/stages/mod.rs`

**Step 1: Write context.rs**

```rust
// core/src/memory/scoring_pipeline/context.rs

use super::config::ScoringPipelineConfig;

/// Immutable context passed to every scoring stage.
pub struct ScoringContext {
    /// Original user query text.
    pub query: String,
    /// Query embedding vector (if available).
    pub query_embedding: Option<Vec<f32>>,
    /// Current Unix timestamp (seconds).
    pub timestamp: i64,
    /// Pipeline configuration snapshot.
    pub config: ScoringPipelineConfig,
}
```

**Step 2: Write stages/mod.rs with ScoringStage trait**

```rust
// core/src/memory/scoring_pipeline/stages/mod.rs

pub mod cosine_rerank;
pub mod recency_boost;
pub mod importance_weight;
pub mod length_normalization;
pub mod time_decay;
pub mod hard_min_score;
pub mod mmr_diversity;

use crate::memory::store::types::ScoredFact;
use super::context::ScoringContext;

/// A single stage in the scoring pipeline.
///
/// Each stage receives a list of scored candidates and the pipeline context,
/// and returns a (possibly reordered/filtered) list. Stages are pure functions
/// with no side effects.
pub trait ScoringStage: Send + Sync {
    /// Human-readable name for logging/metrics.
    fn name(&self) -> &str;

    /// Apply this stage to the candidate list.
    ///
    /// Implementations MUST return candidates sorted by descending score.
    fn apply(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact>;
}
```

**Step 3: Write mod.rs pipeline assembler**

```rust
// core/src/memory/scoring_pipeline/mod.rs

pub mod config;
pub mod context;
pub mod stages;

use crate::memory::store::types::ScoredFact;
use config::ScoringPipelineConfig;
use context::ScoringContext;
use stages::ScoringStage;
use tracing::debug;

/// Assembles and runs a sequence of scoring stages.
pub struct ScoringPipeline {
    stages: Vec<Box<dyn ScoringStage>>,
}

impl ScoringPipeline {
    /// Create an empty pipeline (useful for testing).
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Build the default pipeline from config.
    ///
    /// Constructs all 7 stages in order. Stages whose config
    /// effectively disables them (e.g. weight=0) still run but
    /// produce no score change, keeping the code path consistent.
    pub fn from_config(config: &ScoringPipelineConfig) -> Self {
        use stages::{
            cosine_rerank::CosineRerank,
            recency_boost::RecencyBoost,
            importance_weight::ImportanceWeight,
            length_normalization::LengthNormalization,
            time_decay::TimeDecay,
            hard_min_score::HardMinScore,
            mmr_diversity::MmrDiversity,
        };

        let stages: Vec<Box<dyn ScoringStage>> = vec![
            Box::new(CosineRerank),
            Box::new(RecencyBoost),
            Box::new(ImportanceWeight),
            Box::new(LengthNormalization),
            Box::new(TimeDecay),
            Box::new(HardMinScore),
            Box::new(MmrDiversity),
        ];

        Self { stages }
    }

    /// Run all stages sequentially.
    pub fn run(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        for stage in &self.stages {
            let before = candidates.len();
            candidates = stage.apply(candidates, ctx);
            debug!(
                stage = stage.name(),
                before,
                after = candidates.len(),
                "scoring stage applied"
            );
        }
        candidates
    }
}

impl Default for ScoringPipeline {
    fn default() -> Self {
        Self::from_config(&ScoringPipelineConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::store::types::ScoredFact;

    fn make_scored_fact(content: &str, score: f32) -> ScoredFact {
        let fact = MemoryFact::new(content.to_string(), FactType::Learning, vec![]);
        ScoredFact { fact, score }
    }

    #[test]
    fn test_empty_pipeline_passthrough() {
        let pipeline = ScoringPipeline::new();
        let candidates = vec![
            make_scored_fact("a", 0.9),
            make_scored_fact("b", 0.5),
        ];
        let ctx = ScoringContext {
            query: "test".into(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig::default(),
        };
        let result = pipeline.run(candidates, &ctx);
        assert_eq!(result.len(), 2);
        assert!((result[0].score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_default_pipeline_creates_all_stages() {
        let pipeline = ScoringPipeline::default();
        assert_eq!(pipeline.stages.len(), 7);
    }
}
```

**Step 4: Create placeholder stage files**

Create 7 empty stage files (each with a stub struct that compiles). Each file follows this pattern — the actual formulas are filled in subsequent tasks:

```rust
// core/src/memory/scoring_pipeline/stages/cosine_rerank.rs
use super::{ScoringStage, ScoringContext};
use crate::memory::store::types::ScoredFact;

pub struct CosineRerank;

impl ScoringStage for CosineRerank {
    fn name(&self) -> &str { "cosine_rerank" }
    fn apply(&self, candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        candidates // placeholder
    }
}
```

Repeat for: `recency_boost.rs`, `importance_weight.rs`, `length_normalization.rs`, `time_decay.rs`, `hard_min_score.rs`, `mmr_diversity.rs`. Each has its own struct name (e.g., `RecencyBoost`, `ImportanceWeight`, etc.).

**Step 5: Register module in memory/mod.rs**

Add to `core/src/memory/mod.rs` after `pub mod reranker;` (line 34):

```rust
pub mod scoring_pipeline;
```

Add re-exports after existing re-exports:

```rust
pub use scoring_pipeline::{ScoringPipeline, ScoringPipelineConfig, ScoringContext};
```

**Step 6: Verify compilation**

Run: `cargo test -p alephcore --lib memory::scoring_pipeline -- --nocapture`
Expected: PASS (4 tests: 2 config + 2 pipeline)

**Step 7: Commit**

```bash
git add core/src/memory/scoring_pipeline/ core/src/memory/mod.rs
git commit -m "memory: add ScoringPipeline skeleton with ScoringStage trait"
```

---

## Task 3: Implement Scoring Stages (Cosine Rerank + Recency Boost)

**Files:**
- Modify: `core/src/memory/scoring_pipeline/stages/cosine_rerank.rs`
- Modify: `core/src/memory/scoring_pipeline/stages/recency_boost.rs`

**Step 1: Write cosine_rerank.rs tests and implementation**

```rust
// core/src/memory/scoring_pipeline/stages/cosine_rerank.rs

use super::{ScoringContext, ScoringStage};
use crate::memory::store::types::ScoredFact;

pub struct CosineRerank;

impl CosineRerank {
    /// Compute cosine similarity between two vectors.
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        (dot / (norm_a * norm_b)).clamp(0.0, 1.0)
    }
}

impl ScoringStage for CosineRerank {
    fn name(&self) -> &str {
        "cosine_rerank"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let query_emb = match &ctx.query_embedding {
            Some(e) => e,
            None => return candidates, // no embedding, skip reranking
        };
        let blend = ctx.config.rerank_blend;

        for candidate in &mut candidates {
            if let Some(ref fact_emb) = candidate.fact.embedding {
                let cosine = Self::cosine_similarity(query_emb, fact_emb);
                candidate.score = (1.0 - blend) * candidate.score + blend * cosine;
            }
            // If fact has no embedding, keep original score
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;

    fn make_fact_with_embedding(content: &str, score: f32, emb: Vec<f32>) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Learning, vec![]);
        fact.embedding = Some(emb);
        ScoredFact { fact, score }
    }

    fn make_ctx(query_embedding: Option<Vec<f32>>) -> ScoringContext {
        ScoringContext {
            query: "test".into(),
            query_embedding,
            timestamp: 1700000000,
            config: ScoringPipelineConfig::default(), // blend = 0.3
        }
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let sim = CosineRerank::cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let sim = CosineRerank::cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_no_query_embedding_passthrough() {
        let candidates = vec![make_fact_with_embedding("a", 0.8, vec![1.0, 0.0])];
        let ctx = make_ctx(None);
        let result = CosineRerank.apply(candidates, &ctx);
        assert!((result[0].score - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_rerank_blends_scores() {
        // query=[1,0], fact=[1,0] → cosine=1.0
        // blended = 0.7*0.5 + 0.3*1.0 = 0.65
        let candidates = vec![make_fact_with_embedding("a", 0.5, vec![1.0, 0.0])];
        let ctx = make_ctx(Some(vec![1.0, 0.0]));
        let result = CosineRerank.apply(candidates, &ctx);
        assert!((result[0].score - 0.65).abs() < 1e-6);
    }

    #[test]
    fn test_rerank_reorders() {
        // A: original=0.9, cosine with [1,0] = 0 → blended = 0.63
        // B: original=0.5, cosine with [1,0] = 1 → blended = 0.65
        let candidates = vec![
            make_fact_with_embedding("a", 0.9, vec![0.0, 1.0]),
            make_fact_with_embedding("b", 0.5, vec![1.0, 0.0]),
        ];
        let ctx = make_ctx(Some(vec![1.0, 0.0]));
        let result = CosineRerank.apply(candidates, &ctx);
        assert_eq!(result[0].fact.content, "b"); // B now ranks higher
    }
}
```

**Step 2: Write recency_boost.rs tests and implementation**

```rust
// core/src/memory/scoring_pipeline/stages/recency_boost.rs

use super::{ScoringContext, ScoringStage};
use crate::memory::store::types::ScoredFact;

pub struct RecencyBoost;

impl ScoringStage for RecencyBoost {
    fn name(&self) -> &str {
        "recency_boost"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let half_life = ctx.config.recency_half_life_days;
        let weight = ctx.config.recency_weight;

        if weight <= 0.0 || half_life <= 0.0 {
            return candidates;
        }

        let now = ctx.timestamp;

        for candidate in &mut candidates {
            let age_secs = (now - candidate.fact.created_at).max(0) as f64;
            let age_days = age_secs / 86400.0;
            let boost = (-(age_days) / half_life as f64).exp() as f32 * weight;
            candidate.score += boost;
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;

    fn make_fact_at(content: &str, score: f32, created_at: i64) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Learning, vec![]);
        fact.created_at = created_at;
        ScoredFact { fact, score }
    }

    fn make_ctx(now: i64) -> ScoringContext {
        ScoringContext {
            query: "test".into(),
            query_embedding: None,
            timestamp: now,
            config: ScoringPipelineConfig::default(), // half_life=14, weight=0.1
        }
    }

    #[test]
    fn test_brand_new_memory_gets_full_boost() {
        let now = 1700000000;
        let candidates = vec![make_fact_at("new", 0.5, now)];
        let ctx = make_ctx(now);
        let result = RecencyBoost.apply(candidates, &ctx);
        // age=0 → boost = exp(0)*0.1 = 0.1
        assert!((result[0].score - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_old_memory_gets_small_boost() {
        let now = 1700000000;
        let one_month_ago = now - 30 * 86400;
        let candidates = vec![make_fact_at("old", 0.5, one_month_ago)];
        let ctx = make_ctx(now);
        let result = RecencyBoost.apply(candidates, &ctx);
        // age=30 days, half_life=14 → boost = exp(-30/14)*0.1 ≈ 0.012
        assert!(result[0].score > 0.5);
        assert!(result[0].score < 0.52);
    }

    #[test]
    fn test_zero_weight_disables() {
        let now = 1700000000;
        let mut cfg = ScoringPipelineConfig::default();
        cfg.recency_weight = 0.0;
        let candidates = vec![make_fact_at("a", 0.5, now)];
        let ctx = ScoringContext {
            query: "test".into(),
            query_embedding: None,
            timestamp: now,
            config: cfg,
        };
        let result = RecencyBoost.apply(candidates, &ctx);
        assert!((result[0].score - 0.5).abs() < f32::EPSILON);
    }
}
```

**Step 3: Verify compilation**

Run: `cargo test -p alephcore --lib memory::scoring_pipeline -- --nocapture`
Expected: PASS (all tests including new stage tests)

**Step 4: Commit**

```bash
git add core/src/memory/scoring_pipeline/stages/cosine_rerank.rs core/src/memory/scoring_pipeline/stages/recency_boost.rs
git commit -m "memory: implement CosineRerank and RecencyBoost scoring stages"
```

---

## Task 4: Implement Scoring Stages (Importance + Length + TimeDecay)

**Files:**
- Modify: `core/src/memory/scoring_pipeline/stages/importance_weight.rs`
- Modify: `core/src/memory/scoring_pipeline/stages/length_normalization.rs`
- Modify: `core/src/memory/scoring_pipeline/stages/time_decay.rs`

**Step 1: Write importance_weight.rs**

```rust
// core/src/memory/scoring_pipeline/stages/importance_weight.rs

use super::{ScoringContext, ScoringStage};
use crate::memory::store::types::ScoredFact;

pub struct ImportanceWeight;

impl ScoringStage for ImportanceWeight {
    fn name(&self) -> &str {
        "importance_weight"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, _ctx: &ScoringContext) -> Vec<ScoredFact> {
        for candidate in &mut candidates {
            // confidence is [0,1], maps to multiplier [0.7, 1.0]
            let importance = candidate.fact.confidence.clamp(0.0, 1.0);
            candidate.score *= 0.7 + 0.3 * importance;
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
    use crate::memory::scoring_pipeline::context::ScoringContext;

    fn make_fact_with_confidence(score: f32, confidence: f32) -> ScoredFact {
        let mut fact = MemoryFact::new("test".to_string(), FactType::Learning, vec![]);
        fact.confidence = confidence;
        ScoredFact { fact, score }
    }

    fn make_ctx() -> ScoringContext {
        ScoringContext {
            query: "test".into(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig::default(),
        }
    }

    #[test]
    fn test_max_confidence_preserves_score() {
        let candidates = vec![make_fact_with_confidence(1.0, 1.0)];
        let result = ImportanceWeight.apply(candidates, &make_ctx());
        assert!((result[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_zero_confidence_scales_to_70_percent() {
        let candidates = vec![make_fact_with_confidence(1.0, 0.0)];
        let result = ImportanceWeight.apply(candidates, &make_ctx());
        assert!((result[0].score - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_half_confidence() {
        let candidates = vec![make_fact_with_confidence(1.0, 0.5)];
        let result = ImportanceWeight.apply(candidates, &make_ctx());
        // 0.7 + 0.3*0.5 = 0.85
        assert!((result[0].score - 0.85).abs() < 1e-6);
    }
}
```

**Step 2: Write length_normalization.rs**

```rust
// core/src/memory/scoring_pipeline/stages/length_normalization.rs

use super::{ScoringContext, ScoringStage};
use crate::memory::store::types::ScoredFact;

pub struct LengthNormalization;

impl ScoringStage for LengthNormalization {
    fn name(&self) -> &str {
        "length_normalization"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let anchor = ctx.config.length_norm_anchor.max(1) as f32;

        for candidate in &mut candidates {
            let len = candidate.fact.content.len() as f32;
            let ratio = len / anchor;
            // Only penalize if longer than anchor
            let factor = 1.0 / (1.0 + 0.5 * (ratio.max(1.0)).log2());
            candidate.score *= factor;
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
    use crate::memory::scoring_pipeline::context::ScoringContext;

    fn make_fact_with_len(len: usize, score: f32) -> ScoredFact {
        let content = "x".repeat(len);
        let fact = MemoryFact::new(content, FactType::Learning, vec![]);
        ScoredFact { fact, score }
    }

    fn make_ctx() -> ScoringContext {
        ScoringContext {
            query: "test".into(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig::default(), // anchor=500
        }
    }

    #[test]
    fn test_anchor_length_no_penalty() {
        // len=500, anchor=500 → ratio=1.0 → log2(1)=0 → factor=1.0
        let candidates = vec![make_fact_with_len(500, 1.0)];
        let result = LengthNormalization.apply(candidates, &make_ctx());
        assert!((result[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_short_content_no_penalty() {
        // len=100, anchor=500 → ratio<1 → clamped to 1.0 → factor=1.0
        let candidates = vec![make_fact_with_len(100, 1.0)];
        let result = LengthNormalization.apply(candidates, &make_ctx());
        assert!((result[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_long_content_penalized() {
        // len=2000, anchor=500 → ratio=4 → log2(4)=2 → factor=1/(1+1)=0.5
        let candidates = vec![make_fact_with_len(2000, 1.0)];
        let result = LengthNormalization.apply(candidates, &make_ctx());
        assert!((result[0].score - 0.5).abs() < 1e-6);
    }
}
```

**Step 3: Write time_decay.rs**

```rust
// core/src/memory/scoring_pipeline/stages/time_decay.rs

use super::{ScoringContext, ScoringStage};
use crate::memory::store::types::ScoredFact;

pub struct TimeDecay;

impl ScoringStage for TimeDecay {
    fn name(&self) -> &str {
        "time_decay"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let half_life = ctx.config.time_decay_half_life_days;
        if half_life <= 0.0 {
            return candidates;
        }

        let now = ctx.timestamp;

        for candidate in &mut candidates {
            let age_secs = (now - candidate.fact.created_at).max(0) as f64;
            let age_days = age_secs / 86400.0;
            // Multiplicative: floor at 0.5, so old memories keep at least 50%
            let decay = 0.5 + 0.5 * (-(age_days) / half_life as f64).exp();
            candidate.score *= decay as f32;
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
    use crate::memory::scoring_pipeline::context::ScoringContext;

    fn make_fact_at(score: f32, created_at: i64) -> ScoredFact {
        let mut fact = MemoryFact::new("test".to_string(), FactType::Learning, vec![]);
        fact.created_at = created_at;
        ScoredFact { fact, score }
    }

    fn make_ctx(now: i64) -> ScoringContext {
        ScoringContext {
            query: "test".into(),
            query_embedding: None,
            timestamp: now,
            config: ScoringPipelineConfig::default(), // half_life=60 days
        }
    }

    #[test]
    fn test_brand_new_no_decay() {
        let now = 1700000000;
        let candidates = vec![make_fact_at(1.0, now)];
        let result = TimeDecay.apply(candidates, &make_ctx(now));
        // age=0 → decay = 0.5 + 0.5*1.0 = 1.0
        assert!((result[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_very_old_memory_floor_at_half() {
        let now = 1700000000;
        let very_old = now - 365 * 86400; // 1 year ago
        let candidates = vec![make_fact_at(1.0, very_old)];
        let result = TimeDecay.apply(candidates, &make_ctx(now));
        // age=365 days, half_life=60 → exp(-365/60) ≈ 0.002 → decay ≈ 0.501
        assert!(result[0].score > 0.49);
        assert!(result[0].score < 0.52);
    }

    #[test]
    fn test_at_half_life() {
        let now = 1700000000;
        let at_half_life = now - 60 * 86400; // 60 days ago
        let candidates = vec![make_fact_at(1.0, at_half_life)];
        let result = TimeDecay.apply(candidates, &make_ctx(now));
        // age=60, half_life=60 → exp(-1)≈0.368 → decay ≈ 0.5+0.5*0.368 ≈ 0.684
        assert!(result[0].score > 0.67);
        assert!(result[0].score < 0.70);
    }
}
```

**Step 4: Verify compilation**

Run: `cargo test -p alephcore --lib memory::scoring_pipeline -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/scoring_pipeline/stages/importance_weight.rs core/src/memory/scoring_pipeline/stages/length_normalization.rs core/src/memory/scoring_pipeline/stages/time_decay.rs
git commit -m "memory: implement ImportanceWeight, LengthNormalization, TimeDecay stages"
```

---

## Task 5: Implement Scoring Stages (HardMinScore + MMR Diversity)

**Files:**
- Modify: `core/src/memory/scoring_pipeline/stages/hard_min_score.rs`
- Modify: `core/src/memory/scoring_pipeline/stages/mmr_diversity.rs`

**Step 1: Write hard_min_score.rs**

```rust
// core/src/memory/scoring_pipeline/stages/hard_min_score.rs

use super::{ScoringContext, ScoringStage};
use crate::memory::store::types::ScoredFact;

pub struct HardMinScore;

impl ScoringStage for HardMinScore {
    fn name(&self) -> &str {
        "hard_min_score"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let threshold = ctx.config.hard_min_score;
        candidates
            .into_iter()
            .filter(|c| c.score >= threshold)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
    use crate::memory::scoring_pipeline::context::ScoringContext;

    fn make_scored(score: f32) -> ScoredFact {
        ScoredFact {
            fact: MemoryFact::new("t".into(), FactType::Learning, vec![]),
            score,
        }
    }

    fn make_ctx() -> ScoringContext {
        ScoringContext {
            query: "test".into(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig::default(), // hard_min_score=0.35
        }
    }

    #[test]
    fn test_filters_below_threshold() {
        let candidates = vec![make_scored(0.5), make_scored(0.3), make_scored(0.1)];
        let result = HardMinScore.apply(candidates, &make_ctx());
        assert_eq!(result.len(), 1); // only 0.5 survives
    }

    #[test]
    fn test_keeps_at_threshold() {
        let candidates = vec![make_scored(0.35)];
        let result = HardMinScore.apply(candidates, &make_ctx());
        assert_eq!(result.len(), 1); // exactly at threshold
    }
}
```

**Step 2: Write mmr_diversity.rs**

```rust
// core/src/memory/scoring_pipeline/stages/mmr_diversity.rs

use super::{ScoringContext, ScoringStage};
use crate::memory::store::types::ScoredFact;

pub struct MmrDiversity;

impl MmrDiversity {
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }
}

impl ScoringStage for MmrDiversity {
    fn name(&self) -> &str {
        "mmr_diversity"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let threshold = ctx.config.mmr_similarity_threshold;
        if candidates.len() <= 1 {
            return candidates;
        }

        // Greedy MMR: pick highest-scoring, demote near-duplicates
        let mut selected: Vec<ScoredFact> = Vec::with_capacity(candidates.len());
        let mut deferred: Vec<ScoredFact> = Vec::new();

        for candidate in candidates {
            let is_duplicate = selected.iter().any(|s| {
                match (&s.fact.embedding, &candidate.fact.embedding) {
                    (Some(a), Some(b)) => Self::cosine_similarity(a, b) > threshold,
                    _ => false, // no embedding = not a duplicate
                }
            });

            if is_duplicate {
                deferred.push(candidate);
            } else {
                selected.push(candidate);
            }
        }

        // Append deferred at the end (demoted, not removed)
        selected.extend(deferred);
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
    use crate::memory::scoring_pipeline::context::ScoringContext;

    fn make_fact_with_emb(content: &str, score: f32, emb: Vec<f32>) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Learning, vec![]);
        fact.embedding = Some(emb);
        ScoredFact { fact, score }
    }

    fn make_ctx() -> ScoringContext {
        ScoringContext {
            query: "test".into(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig::default(), // mmr_threshold=0.85
        }
    }

    #[test]
    fn test_identical_embeddings_demoted() {
        let candidates = vec![
            make_fact_with_emb("a", 0.9, vec![1.0, 0.0]),
            make_fact_with_emb("b", 0.8, vec![1.0, 0.0]), // identical → demoted
            make_fact_with_emb("c", 0.7, vec![0.0, 1.0]), // different → kept
        ];
        let result = MmrDiversity.apply(candidates, &make_ctx());
        assert_eq!(result[0].fact.content, "a");
        assert_eq!(result[1].fact.content, "c"); // c before b now
        assert_eq!(result[2].fact.content, "b"); // b demoted
    }

    #[test]
    fn test_diverse_embeddings_unchanged() {
        let candidates = vec![
            make_fact_with_emb("a", 0.9, vec![1.0, 0.0]),
            make_fact_with_emb("b", 0.8, vec![0.0, 1.0]),
        ];
        let result = MmrDiversity.apply(candidates, &make_ctx());
        assert_eq!(result[0].fact.content, "a");
        assert_eq!(result[1].fact.content, "b");
    }

    #[test]
    fn test_no_embeddings_unchanged() {
        let candidates = vec![
            ScoredFact {
                fact: MemoryFact::new("a".into(), FactType::Learning, vec![]),
                score: 0.9,
            },
            ScoredFact {
                fact: MemoryFact::new("b".into(), FactType::Learning, vec![]),
                score: 0.8,
            },
        ];
        let result = MmrDiversity.apply(candidates, &make_ctx());
        assert_eq!(result.len(), 2);
    }
}
```

**Step 3: Verify all pipeline tests pass**

Run: `cargo test -p alephcore --lib memory::scoring_pipeline -- --nocapture`
Expected: PASS (all tests across all 7 stages + pipeline tests)

**Step 4: Commit**

```bash
git add core/src/memory/scoring_pipeline/stages/hard_min_score.rs core/src/memory/scoring_pipeline/stages/mmr_diversity.rs
git commit -m "memory: implement HardMinScore and MmrDiversity scoring stages"
```

---

## Task 6: Embedding Cache

**Files:**
- Create: `core/src/memory/embedding_cache.rs`
- Modify: `core/src/memory/smart_embedder.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Write embedding_cache.rs**

```rust
// core/src/memory/embedding_cache.rs

use lru::LruCache;
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// TTL entry stored in the LRU cache.
struct CacheEntry {
    vector: Vec<f32>,
    created_at: Instant,
}

/// LRU embedding cache with per-entry TTL.
///
/// Cache keys incorporate the embedding task type so that query and passage
/// embeddings for the same text are stored separately.
pub struct EmbeddingCache {
    entries: Mutex<LruCache<String, CacheEntry>>,
    ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl EmbeddingCache {
    /// Create a new cache.
    ///
    /// * `max_size` — maximum number of entries (default: 256).
    /// * `ttl` — time-to-live per entry (default: 30 minutes).
    pub fn new(max_size: usize, ttl: Duration) -> Self {
        let cap = NonZeroUsize::new(max_size.max(1)).unwrap();
        Self {
            entries: Mutex::new(LruCache::new(cap)),
            ttl,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Build a deterministic cache key from task prefix and text.
    fn cache_key(task_prefix: &str, text: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(task_prefix.as_bytes());
        hasher.update(b":");
        hasher.update(text.as_bytes());
        let hash = hasher.finalize();
        // First 24 hex chars (12 bytes) — sufficient for uniqueness in a 256-entry cache
        hex::encode(&hash[..12])
    }

    /// Try to get a cached embedding.
    pub async fn get(&self, task_prefix: &str, text: &str) -> Option<Vec<f32>> {
        let key = Self::cache_key(task_prefix, text);
        let mut cache = self.entries.lock().await;
        if let Some(entry) = cache.get(&key) {
            if entry.created_at.elapsed() < self.ttl {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.vector.clone());
            }
            // Expired — remove it
            cache.pop(&key);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert an embedding into the cache.
    pub async fn put(&self, task_prefix: &str, text: &str, vector: Vec<f32>) {
        let key = Self::cache_key(task_prefix, text);
        let entry = CacheEntry {
            vector,
            created_at: Instant::now(),
        };
        let mut cache = self.entries.lock().await;
        cache.put(key, entry);
    }

    /// Return (hits, misses).
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }
}

impl Default for EmbeddingCache {
    fn default() -> Self {
        Self::new(256, Duration::from_secs(1800))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_hit_and_miss() {
        let cache = EmbeddingCache::new(10, Duration::from_secs(60));
        assert!(cache.get("query", "hello").await.is_none());

        cache.put("query", "hello", vec![1.0, 2.0]).await;

        let result = cache.get("query", "hello").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap(), vec![1.0, 2.0]);

        let (hits, misses) = cache.stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
    }

    #[tokio::test]
    async fn test_task_prefix_isolation() {
        let cache = EmbeddingCache::new(10, Duration::from_secs(60));
        cache.put("query", "hello", vec![1.0]).await;
        cache.put("passage", "hello", vec![2.0]).await;

        let q = cache.get("query", "hello").await.unwrap();
        let p = cache.get("passage", "hello").await.unwrap();
        assert_eq!(q, vec![1.0]);
        assert_eq!(p, vec![2.0]);
    }

    #[tokio::test]
    async fn test_ttl_expiry() {
        let cache = EmbeddingCache::new(10, Duration::from_millis(50));
        cache.put("query", "hello", vec![1.0]).await;

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(cache.get("query", "hello").await.is_none());
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let cache = EmbeddingCache::new(2, Duration::from_secs(60));
        cache.put("q", "a", vec![1.0]).await;
        cache.put("q", "b", vec![2.0]).await;
        cache.put("q", "c", vec![3.0]).await; // evicts "a"

        assert!(cache.get("q", "a").await.is_none());
        assert!(cache.get("q", "b").await.is_some());
        assert!(cache.get("q", "c").await.is_some());
    }
}
```

**Step 2: Add task-aware methods to SmartEmbedder**

Modify `core/src/memory/smart_embedder.rs`. Add after existing imports:

```rust
use crate::memory::embedding_cache::EmbeddingCache;
```

Add `EmbeddingTask` enum and new methods. The key changes:

1. Add `cache: EmbeddingCache` field to `SmartEmbedder`
2. Add `embed_query(&self, text: &str)` — prepends `"query: "` and uses cache key `"query"`
3. Add `embed_passage(&self, text: &str)` — prepends `"passage: "` and uses cache key `"passage"`
4. Existing `embed()` method remains unchanged for backward compat (delegates to `embed_passage`)
5. Both new methods check/populate the cache

**Important**: The `multilingual-e5-small` model expects `"query: "` and `"passage: "` prefixes for task-aware embedding. See [fastembed docs](https://huggingface.co/intfloat/multilingual-e5-small#faq).

**Step 3: Register module in memory/mod.rs**

Add: `pub mod embedding_cache;`
Add re-export: `pub use embedding_cache::EmbeddingCache;`

**Step 4: Verify all tests pass**

Run: `cargo test -p alephcore --lib memory::embedding_cache -- --nocapture`
Expected: PASS (4 tests)

Run: `cargo test -p alephcore --lib memory::smart_embedder -- --nocapture`
Expected: PASS (existing + new tests)

**Step 5: Commit**

```bash
git add core/src/memory/embedding_cache.rs core/src/memory/smart_embedder.rs core/src/memory/mod.rs
git commit -m "memory: add EmbeddingCache with LRU+TTL and task-aware embedding"
```

---

## Task 7: Adaptive Retrieval Gate

**Files:**
- Create: `core/src/memory/adaptive_retrieval.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Write adaptive_retrieval.rs**

```rust
// core/src/memory/adaptive_retrieval.rs

use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

/// Configuration for the adaptive retrieval gate.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AdaptiveRetrievalConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_min_length_cjk")]
    pub min_length_cjk: usize,
    #[serde(default = "default_min_length_other")]
    pub min_length_other: usize,
    #[serde(default = "default_skip_patterns")]
    pub skip_patterns: Vec<String>,
    #[serde(default = "default_force_patterns")]
    pub force_patterns: Vec<String>,
}

fn default_enabled() -> bool { true }
fn default_min_length_cjk() -> usize { 6 }
fn default_min_length_other() -> usize { 15 }
fn default_skip_patterns() -> Vec<String> {
    ["hello", "hi", "hey", "yes", "no", "ok", "thanks", "thank you",
     "bye", "goodbye", "你好", "好的", "谢谢", "再见"]
        .iter().map(|s| s.to_string()).collect()
}
fn default_force_patterns() -> Vec<String> {
    ["remember", "recall", "last time", "previously", "earlier",
     "you said", "you told", "my preference", "我记得", "上次",
     "之前", "你说过", "我的偏好"]
        .iter().map(|s| s.to_string()).collect()
}

impl Default for AdaptiveRetrievalConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            min_length_cjk: default_min_length_cjk(),
            min_length_other: default_min_length_other(),
            skip_patterns: default_skip_patterns(),
            force_patterns: default_force_patterns(),
        }
    }
}

/// Decision from the adaptive retrieval gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrievalDecision {
    /// Normal retrieval should proceed.
    Retrieve,
    /// Skip retrieval — query is too trivial.
    Skip,
    /// Force retrieval — query explicitly requests memory recall.
    ForceRetrieve,
}

/// Lightweight rule engine that decides whether to perform memory retrieval.
pub struct AdaptiveRetrievalGate {
    config: AdaptiveRetrievalConfig,
}

impl AdaptiveRetrievalGate {
    pub fn new(config: AdaptiveRetrievalConfig) -> Self {
        Self { config }
    }

    /// Evaluate whether the query warrants memory retrieval.
    ///
    /// Priority: ForcePattern > SkipPattern > LengthCheck.
    pub fn evaluate(&self, query: &str) -> RetrievalDecision {
        if !self.config.enabled {
            return RetrievalDecision::Retrieve;
        }

        let trimmed = query.trim();
        let lower = trimmed.to_lowercase();

        // 1. Force patterns take highest priority
        for pattern in &self.config.force_patterns {
            if lower.contains(&pattern.to_lowercase()) {
                return RetrievalDecision::ForceRetrieve;
            }
        }

        // 2. Skip patterns (exact match on trimmed lowercase)
        for pattern in &self.config.skip_patterns {
            if lower == pattern.to_lowercase() {
                return RetrievalDecision::Skip;
            }
        }

        // 3. Slash commands
        if trimmed.starts_with('/') {
            return RetrievalDecision::Skip;
        }

        // 4. Length check with CJK awareness
        let has_cjk = trimmed.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        let min_len = if has_cjk {
            self.config.min_length_cjk
        } else {
            self.config.min_length_other
        };

        if trimmed.chars().count() < min_len {
            return RetrievalDecision::Skip;
        }

        RetrievalDecision::Retrieve
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> AdaptiveRetrievalGate {
        AdaptiveRetrievalGate::new(AdaptiveRetrievalConfig::default())
    }

    #[test]
    fn test_force_pattern_highest_priority() {
        // "do you remember" contains "remember" → ForceRetrieve
        assert_eq!(gate().evaluate("do you remember my name?"), RetrievalDecision::ForceRetrieve);
    }

    #[test]
    fn test_force_pattern_chinese() {
        assert_eq!(gate().evaluate("上次我们讨论了什么"), RetrievalDecision::ForceRetrieve);
    }

    #[test]
    fn test_skip_greeting() {
        assert_eq!(gate().evaluate("hello"), RetrievalDecision::Skip);
        assert_eq!(gate().evaluate("Hello"), RetrievalDecision::Skip);
        assert_eq!(gate().evaluate("你好"), RetrievalDecision::Skip);
    }

    #[test]
    fn test_skip_command() {
        assert_eq!(gate().evaluate("/new"), RetrievalDecision::Skip);
        assert_eq!(gate().evaluate("/help"), RetrievalDecision::Skip);
    }

    #[test]
    fn test_skip_short_english() {
        assert_eq!(gate().evaluate("yes"), RetrievalDecision::Skip);
        assert_eq!(gate().evaluate("short"), RetrievalDecision::Skip); // 5 chars < 15
    }

    #[test]
    fn test_retrieve_long_english() {
        assert_eq!(
            gate().evaluate("What is the capital of France and why?"),
            RetrievalDecision::Retrieve,
        );
    }

    #[test]
    fn test_retrieve_cjk_above_threshold() {
        // 7 CJK chars > 6 threshold
        assert_eq!(gate().evaluate("法国的首都是哪个城市"), RetrievalDecision::Retrieve);
    }

    #[test]
    fn test_skip_short_cjk() {
        // 3 CJK chars < 6 threshold
        assert_eq!(gate().evaluate("什么？"), RetrievalDecision::Skip);
    }

    #[test]
    fn test_disabled_always_retrieves() {
        let config = AdaptiveRetrievalConfig {
            enabled: false,
            ..Default::default()
        };
        let gate = AdaptiveRetrievalGate::new(config);
        assert_eq!(gate.evaluate("hi"), RetrievalDecision::Retrieve);
    }

    #[test]
    fn test_force_overrides_skip() {
        // "hello" would skip, but "do you remember saying hello" → force
        assert_eq!(
            gate().evaluate("do you remember saying hello"),
            RetrievalDecision::ForceRetrieve,
        );
    }
}
```

**Step 2: Register module**

Add to `core/src/memory/mod.rs`:
```rust
pub mod adaptive_retrieval;
```
And re-export:
```rust
pub use adaptive_retrieval::{AdaptiveRetrievalGate, AdaptiveRetrievalConfig, RetrievalDecision};
```

**Step 3: Verify tests**

Run: `cargo test -p alephcore --lib memory::adaptive_retrieval -- --nocapture`
Expected: PASS (10 tests)

**Step 4: Commit**

```bash
git add core/src/memory/adaptive_retrieval.rs core/src/memory/mod.rs
git commit -m "memory: add AdaptiveRetrievalGate with CJK-aware rules"
```

---

## Task 8: Noise Filter

**Files:**
- Create: `core/src/memory/noise_filter.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Write noise_filter.rs**

```rust
// core/src/memory/noise_filter.rs

use crate::memory::store::types::ScoredFact;
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

/// Configuration for the noise filter.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoiseFilterConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_min_content_length")]
    pub min_content_length: usize,
    #[serde(default = "default_denial_patterns")]
    pub denial_patterns: Vec<String>,
    #[serde(default = "default_boilerplate_patterns")]
    pub boilerplate_patterns: Vec<String>,
}

fn default_enabled() -> bool { true }
fn default_min_content_length() -> usize { 10 }
fn default_denial_patterns() -> Vec<String> {
    ["i can't help with", "i'm sorry, but i", "i cannot assist",
     "as an ai", "i don't have the ability"]
        .iter().map(|s| s.to_string()).collect()
}
fn default_boilerplate_patterns() -> Vec<String> {
    ["<system>", "</system>", "<relevant-memories>", "</relevant-memories>"]
        .iter().map(|s| s.to_string()).collect()
}

impl Default for NoiseFilterConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            min_content_length: default_min_content_length(),
            denial_patterns: default_denial_patterns(),
            boilerplate_patterns: default_boilerplate_patterns(),
        }
    }
}

/// Dual-defense noise filter for memory content.
///
/// Used at two points:
/// - **Storage time**: `should_store()` prevents noisy content from being saved
/// - **Retrieval time**: `filter_results()` removes low-quality search results
pub struct NoiseFilter {
    config: NoiseFilterConfig,
}

impl NoiseFilter {
    pub fn new(config: NoiseFilterConfig) -> Self {
        Self { config }
    }

    /// Check if content should be stored as a memory.
    ///
    /// Returns `true` if the content passes all noise checks.
    pub fn should_store(&self, content: &str) -> bool {
        if !self.config.enabled {
            return true;
        }

        let trimmed = content.trim();

        // Too short
        if trimmed.len() < self.config.min_content_length {
            return false;
        }

        // Pure emoji / punctuation check
        let has_alphanumeric = trimmed.chars().any(|c| c.is_alphanumeric());
        if !has_alphanumeric {
            return false;
        }

        let lower = trimmed.to_lowercase();

        // Agent denial patterns
        for pattern in &self.config.denial_patterns {
            if lower.contains(&pattern.to_lowercase()) {
                return false;
            }
        }

        // Boilerplate / system tags
        for pattern in &self.config.boilerplate_patterns {
            if lower.contains(&pattern.to_lowercase()) {
                return false;
            }
        }

        true
    }

    /// Filter retrieval results, removing noisy entries.
    pub fn filter_results(&self, results: Vec<ScoredFact>) -> Vec<ScoredFact> {
        if !self.config.enabled {
            return results;
        }

        results
            .into_iter()
            .filter(|r| self.should_store(&r.fact.content))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    fn filter() -> NoiseFilter {
        NoiseFilter::new(NoiseFilterConfig::default())
    }

    #[test]
    fn test_normal_content_passes() {
        assert!(filter().should_store("The user prefers dark mode for all applications."));
    }

    #[test]
    fn test_short_content_rejected() {
        assert!(!filter().should_store("hi"));
    }

    #[test]
    fn test_pure_emoji_rejected() {
        assert!(!filter().should_store("😀😎🎉🔥"));
    }

    #[test]
    fn test_denial_rejected() {
        assert!(!filter().should_store("I'm sorry, but I can't help with that request."));
    }

    #[test]
    fn test_system_tags_rejected() {
        assert!(!filter().should_store("<system>You are a helpful assistant</system>"));
    }

    #[test]
    fn test_filter_results() {
        let results = vec![
            ScoredFact {
                fact: MemoryFact::new("Good memory content here".into(), FactType::Learning, vec![]),
                score: 0.9,
            },
            ScoredFact {
                fact: MemoryFact::new("hi".into(), FactType::Learning, vec![]),
                score: 0.8,
            },
        ];
        let filtered = filter().filter_results(results);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].fact.content, "Good memory content here");
    }

    #[test]
    fn test_disabled_passes_everything() {
        let config = NoiseFilterConfig {
            enabled: false,
            ..Default::default()
        };
        let f = NoiseFilter::new(config);
        assert!(f.should_store("hi"));
    }
}
```

**Step 2: Register module**

Add to `core/src/memory/mod.rs`:
```rust
pub mod noise_filter;
```
Re-export:
```rust
pub use noise_filter::{NoiseFilter, NoiseFilterConfig};
```

**Step 3: Verify**

Run: `cargo test -p alephcore --lib memory::noise_filter -- --nocapture`
Expected: PASS (7 tests)

**Step 4: Commit**

```bash
git add core/src/memory/noise_filter.rs core/src/memory/mod.rs
git commit -m "memory: add dual-defense NoiseFilter for storage and retrieval"
```

---

## Task 9: Storage Deduplication

**Files:**
- Modify: `core/src/memory/ingestion.rs`

**Step 1: Write test for deduplication**

Add to `ingestion.rs` tests module a test that verifies dedup logic:

```rust
#[test]
fn test_is_duplicate_above_threshold() {
    assert!(is_duplicate_score(0.96, 0.95));
    assert!(!is_duplicate_score(0.94, 0.95));
    assert!(is_duplicate_score(0.95, 0.95));
}
```

**Step 2: Add dedup helper function**

Add to `ingestion.rs` (before `impl MemoryIngestion`):

```rust
/// Check if a similarity score indicates a duplicate.
fn is_duplicate_score(score: f32, threshold: f32) -> bool {
    score >= threshold
}
```

**Step 3: Modify `store_memory` to check duplicates**

In `MemoryIngestion::store_memory()`, after generating the embedding and before inserting, add:

```rust
// Dedup check: skip if near-identical memory exists
let filter = SearchFilter::valid_only(None);
let existing = self.database.vector_search(&embedding, 1, Some(&filter)).await?;
if let Some(top) = existing.first() {
    if is_duplicate_score(top.score, 0.95) {
        tracing::debug!(
            existing_id = %top.fact.id,
            score = top.score,
            "skipping duplicate memory"
        );
        return Ok(top.fact.id.clone());
    }
}
```

**Step 4: Verify compilation**

Run: `cargo test -p alephcore --lib memory::ingestion -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/ingestion.rs
git commit -m "memory: add storage-time deduplication via vector similarity"
```

---

## Task 10: JSONL Backup Service

**Files:**
- Create: `core/src/memory/backup.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Write backup.rs**

```rust
// core/src/memory/backup.rs

use crate::error::AlephError;
use crate::memory::store::MemoryBackend;
use crate::memory::store::types::SearchFilter;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Service for backing up memories to JSONL files.
pub struct MemoryBackupService {
    database: MemoryBackend,
    backup_dir: PathBuf,
    max_backups: usize,
}

impl MemoryBackupService {
    pub fn new(database: MemoryBackend, backup_dir: PathBuf, max_backups: usize) -> Self {
        Self {
            database,
            backup_dir,
            max_backups,
        }
    }

    /// Export all valid facts to a dated JSONL file.
    ///
    /// Returns the path of the created backup file.
    pub async fn export_backup(&self) -> Result<PathBuf, AlephError> {
        // Ensure backup directory exists
        tokio::fs::create_dir_all(&self.backup_dir)
            .await
            .map_err(|e| AlephError::config(format!("Failed to create backup dir: {e}")))?;

        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let filename = format!("memory-backup-{date}.jsonl");
        let path = self.backup_dir.join(&filename);

        let filter = SearchFilter::new().with_valid_only();
        let facts = self.database.get_all_facts(Some(&filter)).await?;

        let mut lines = Vec::with_capacity(facts.len());
        for fact in &facts {
            match serde_json::to_string(fact) {
                Ok(line) => lines.push(line),
                Err(e) => warn!(fact_id = %fact.id, "Failed to serialize fact: {e}"),
            }
        }

        let content = lines.join("\n") + "\n";
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| AlephError::config(format!("Failed to write backup: {e}")))?;

        info!(path = %path.display(), facts = facts.len(), "Memory backup exported");

        // Clean up old backups
        self.cleanup_old_backups().await;

        Ok(path)
    }

    /// Remove backups exceeding the retention limit.
    async fn cleanup_old_backups(&self) {
        let mut backups: Vec<PathBuf> = Vec::new();

        let Ok(mut entries) = tokio::fs::read_dir(&self.backup_dir).await else {
            return;
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("memory-backup-") && name.ends_with(".jsonl") {
                backups.push(entry.path());
            }
        }

        // Sort by name (date-based names sort chronologically)
        backups.sort();

        // Remove oldest until within limit
        while backups.len() > self.max_backups {
            if let Some(oldest) = backups.first() {
                if let Err(e) = tokio::fs::remove_file(oldest).await {
                    warn!(path = %oldest.display(), "Failed to remove old backup: {e}");
                } else {
                    info!(path = %oldest.display(), "Removed old backup");
                }
                backups.remove(0);
            }
        }
    }

    /// Restore facts from a JSONL backup file.
    ///
    /// Returns the number of facts restored.
    pub async fn restore_from_backup(&self, path: &Path) -> Result<usize, AlephError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| AlephError::config(format!("Failed to read backup: {e}")))?;

        let mut count = 0;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str(line) {
                Ok(fact) => {
                    self.database.insert_fact(&fact).await?;
                    count += 1;
                }
                Err(e) => warn!(line_prefix = &line[..line.len().min(50)], "Skipping malformed line: {e}"),
            }
        }

        info!(path = %path.display(), restored = count, "Memory backup restored");
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backup_filename_format() {
        let date = "2026-02-24";
        let filename = format!("memory-backup-{date}.jsonl");
        assert!(filename.starts_with("memory-backup-"));
        assert!(filename.ends_with(".jsonl"));
    }
}
```

**Step 2: Register module**

Add to `core/src/memory/mod.rs`:
```rust
pub mod backup;
```
Re-export:
```rust
pub use backup::MemoryBackupService;
```

**Step 3: Verify**

Run: `cargo test -p alephcore --lib memory::backup -- --nocapture`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/memory/backup.rs core/src/memory/mod.rs
git commit -m "memory: add JSONL backup service with rolling retention"
```

---

## Task 11: Configuration Integration

**Files:**
- Modify: `core/src/config/types/memory.rs`

**Step 1: Add new config sections to MemoryConfig**

Add these fields to `MemoryConfig` struct (after `pub memory_decay: MemoryDecayPolicy`):

```rust
/// Scoring pipeline configuration.
#[serde(default)]
pub scoring_pipeline: crate::memory::scoring_pipeline::config::ScoringPipelineConfig,

/// Adaptive retrieval gate configuration.
#[serde(default)]
pub adaptive_retrieval: crate::memory::adaptive_retrieval::AdaptiveRetrievalConfig,

/// Noise filter configuration.
#[serde(default)]
pub noise_filter: crate::memory::noise_filter::NoiseFilterConfig,

/// Storage deduplication similarity threshold.
#[serde(default = "default_dedup_threshold")]
pub dedup_similarity_threshold: f32,

/// Embedding cache max entries.
#[serde(default = "default_embedding_cache_max_size")]
pub embedding_cache_max_size: usize,

/// Embedding cache TTL in seconds.
#[serde(default = "default_embedding_cache_ttl_seconds")]
pub embedding_cache_ttl_seconds: u64,

/// Backup configuration: enabled.
#[serde(default = "default_backup_enabled")]
pub backup_enabled: bool,

/// Backup max retained files.
#[serde(default = "default_backup_max_files")]
pub backup_max_files: usize,
```

Add default functions:

```rust
fn default_dedup_threshold() -> f32 { 0.95 }
fn default_embedding_cache_max_size() -> usize { 256 }
fn default_embedding_cache_ttl_seconds() -> u64 { 1800 }
fn default_backup_enabled() -> bool { true }
fn default_backup_max_files() -> usize { 7 }
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: success

**Step 3: Commit**

```bash
git add core/src/config/types/memory.rs
git commit -m "config: add scoring pipeline, adaptive retrieval, noise filter, backup settings"
```

---

## Task 12: Wire Pipeline into Hybrid Retrieval

**Files:**
- Modify: `core/src/memory/hybrid_retrieval/hybrid.rs`

**Step 1: Add ScoringPipeline to HybridRetrieval**

Add a field to `HybridRetrieval`:

```rust
use crate::memory::scoring_pipeline::{ScoringPipeline, ScoringPipelineConfig};
use crate::memory::scoring_pipeline::context::ScoringContext;

pub struct HybridRetrieval {
    config: HybridSearchConfig,
    database: MemoryBackend,
    scoring_pipeline: Option<ScoringPipeline>,
}
```

**Step 2: Update constructors**

- `new()`: accept optional `ScoringPipelineConfig`, build pipeline if provided
- `with_defaults()`: create pipeline with default config
- Backward compat: if `scoring_pipeline` is `None`, skip post-processing

**Step 3: Integrate in search methods**

In `search_facts()` and `search_facts_with_limit()`, after RRF fusion produces `Vec<ScoredFact>` and before `apply_min_score()`:

```rust
if let Some(ref pipeline) = self.scoring_pipeline {
    let ctx = ScoringContext {
        query: query_text.to_string(),
        query_embedding: Some(query_embedding.to_vec()),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
        config: pipeline_config.clone(),
    };
    scored_facts = pipeline.run(scored_facts, &ctx);
}
```

**Step 4: Update existing tests**

Update `HybridRetrieval::new()` and `with_defaults()` call sites in tests to pass `None` for the new parameter (backward compat).

**Step 5: Verify**

Run: `cargo test -p alephcore --lib memory::hybrid_retrieval -- --nocapture`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/memory/hybrid_retrieval/hybrid.rs
git commit -m "memory: wire ScoringPipeline into HybridRetrieval search flow"
```

---

## Task 13: Wire Noise Filter into Ingestion

**Files:**
- Modify: `core/src/memory/ingestion.rs`

**Step 1: Add NoiseFilter to MemoryIngestion**

Add field:

```rust
use crate::memory::noise_filter::{NoiseFilter, NoiseFilterConfig};

pub struct MemoryIngestion {
    database: MemoryBackend,
    embedder: Arc<SmartEmbedder>,
    config: Arc<MemoryConfig>,
    noise_filter: NoiseFilter,
}
```

**Step 2: Initialize in constructor**

In `new()`:

```rust
let noise_filter = NoiseFilter::new(config.noise_filter.clone());
```

**Step 3: Add noise check in store_memory()**

Before embedding generation:

```rust
// Noise filter: reject noisy content
if !self.noise_filter.should_store(user_input) {
    tracing::debug!("Noise filter rejected user input");
    return Err(AlephError::config("Content filtered as noise".to_string()));
}
```

**Step 4: Verify**

Run: `cargo test -p alephcore --lib memory::ingestion -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/ingestion.rs
git commit -m "memory: wire NoiseFilter into ingestion pipeline"
```

---

## Task 14: Full Integration Test

**Files:**
- Modify: `core/src/memory/scoring_pipeline/mod.rs` (add integration test)

**Step 1: Write end-to-end pipeline test**

Add to `scoring_pipeline/mod.rs` tests:

```rust
#[test]
fn test_full_pipeline_end_to_end() {
    use crate::memory::context::FactType;

    let config = ScoringPipelineConfig::default();
    let pipeline = ScoringPipeline::from_config(&config);

    let now = 1700000000i64;
    let one_day_ago = now - 86400;
    let one_year_ago = now - 365 * 86400;

    // Create diverse candidates
    let candidates = vec![
        // Recent, high confidence, moderate embedding match
        {
            let mut f = MemoryFact::new("Recent important fact".into(), FactType::Preference, vec![]);
            f.created_at = one_day_ago;
            f.confidence = 0.9;
            f.embedding = Some(vec![0.8, 0.6]);
            ScoredFact { fact: f, score: 0.7 }
        },
        // Old, low confidence
        {
            let mut f = MemoryFact::new("Old unimportant fact with lots of extra verbose text that goes on and on".into(), FactType::Other, vec![]);
            f.created_at = one_year_ago;
            f.confidence = 0.2;
            f.embedding = Some(vec![0.1, 0.9]);
            ScoredFact { fact: f, score: 0.6 }
        },
        // Below hard_min_score after processing
        {
            let mut f = MemoryFact::new("Barely relevant".into(), FactType::Learning, vec![]);
            f.created_at = now;
            f.confidence = 0.3;
            f.embedding = Some(vec![0.5, 0.5]);
            ScoredFact { fact: f, score: 0.2 }
        },
    ];

    let ctx = ScoringContext {
        query: "test query".into(),
        query_embedding: Some(vec![1.0, 0.0]),
        timestamp: now,
        config,
    };

    let results = pipeline.run(candidates, &ctx);

    // The low-scoring candidate should be filtered out by HardMinScore
    // The recent high-confidence fact should rank highest
    assert!(!results.is_empty());
    assert!(results.len() <= 3);

    // First result should be the recent important one (boosted by recency + confidence)
    if results.len() >= 1 {
        assert!(results[0].fact.content.contains("Recent"));
    }
}
```

**Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib memory::scoring_pipeline -- --nocapture`
Expected: PASS (all unit + integration tests)

Run: `cargo test -p alephcore --lib memory -- --nocapture 2>&1 | tail -20`
Expected: All memory module tests pass

**Step 3: Commit**

```bash
git add core/src/memory/scoring_pipeline/mod.rs
git commit -m "memory: add full pipeline integration test"
```

---

## Task 15: Final Verification & Docs Update

**Step 1: Run full crate build**

Run: `cargo build -p alephcore`
Expected: success, no warnings

**Step 2: Run full test suite**

Run: `cargo test -p alephcore`
Expected: all tests pass

**Step 3: Update MEMORY_SYSTEM.md**

Add a new section to `docs/reference/MEMORY_SYSTEM.md` documenting the scoring pipeline, adaptive retrieval, noise filter, embedding cache, dedup, and backup features.

**Step 4: Final commit**

```bash
git add docs/reference/MEMORY_SYSTEM.md
git commit -m "docs: update MEMORY_SYSTEM.md with scoring pipeline and optimization features"
```

---

## Summary

| Task | Component | New Files | Modified Files |
|------|-----------|-----------|----------------|
| 1 | ScoringPipelineConfig | `scoring_pipeline/config.rs` | — |
| 2 | Pipeline skeleton | `scoring_pipeline/mod.rs`, `context.rs`, `stages/mod.rs`, 7 stage stubs | `memory/mod.rs` |
| 3 | CosineRerank + RecencyBoost | — | 2 stage files |
| 4 | Importance + Length + TimeDecay | — | 3 stage files |
| 5 | HardMinScore + MMR | — | 2 stage files |
| 6 | EmbeddingCache | `embedding_cache.rs` | `smart_embedder.rs`, `mod.rs` |
| 7 | AdaptiveRetrievalGate | `adaptive_retrieval.rs` | `mod.rs` |
| 8 | NoiseFilter | `noise_filter.rs` | `mod.rs` |
| 9 | Storage Dedup | — | `ingestion.rs` |
| 10 | JSONL Backup | `backup.rs` | `mod.rs` |
| 11 | Config Integration | — | `config/types/memory.rs` |
| 12 | Wire Pipeline | — | `hybrid_retrieval/hybrid.rs` |
| 13 | Wire NoiseFilter | — | `ingestion.rs` |
| 14 | Integration Test | — | `scoring_pipeline/mod.rs` |
| 15 | Final Verification | — | `docs/reference/MEMORY_SYSTEM.md` |
