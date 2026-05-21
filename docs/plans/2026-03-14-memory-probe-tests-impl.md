# Memory Probe Tests Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement 41 production-grade probe test scenarios validating the memory optimization features (RRF fusion, cross-encoder rerank, query expansion, retrieval trace, tiered decay, access reinforcement, tier promotion, and reflection system).

**Architecture:** 7 tasks matching the file structure — infrastructure first, then P1–P6 phase modules. P1–P5 are sync unit tests against production logic; P6 is async integration with the real ScoringPipeline. Tests live under `tests/memory_probe/` following existing probe conventions.

**Tech Stack:** Rust, `#[test]` / `#[tokio::test]`, `alephcore` crate (no mocks for production types), `tempfile` for P6.

**Reference:**
- Design doc: `docs/plans/2026-03-14-memory-probe-tests-design.md`
- Existing probe pattern: `tests/session_probe.rs` + `tests/session_probe/`

---

### Task 1: Infrastructure — entry point + harness + mock embedding

**Files:**
- Create: `tests/memory_probe.rs`
- Create: `tests/memory_probe/harness.rs`
- Create: `tests/memory_probe/mock_embedding.rs`

**Step 1: Create entry point**

Create `tests/memory_probe.rs`:

```rust
//! Memory system probe integration tests.
//!
//! Validates RRF fusion, cross-encoder rerank, query expansion,
//! retrieval trace, tiered decay, access reinforcement, tier
//! promotion, and the reflection system.

mod memory_probe {
    pub mod mock_embedding;
    pub mod harness;
    pub mod extraction_classification;
    pub mod fusion_scoring;
    pub mod rerank;
    pub mod retrieval_trace;
    pub mod decay_promotion;
    pub mod end_to_end;
}
```

**Step 2: Create MockEmbeddingProvider**

Create `tests/memory_probe/mock_embedding.rs`:

```rust
//! Deterministic pseudo-embedding provider for probe tests.
//!
//! Uses a simple hash of the content to generate a fixed-dimension
//! vector. Texts sharing keywords will have higher cosine similarity
//! because shared words contribute identical components.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub const DEFAULT_DIM: usize = 128;

/// Generate a deterministic pseudo-embedding from text content.
///
/// Strategy: split on whitespace, hash each token, and scatter
/// the hash into a fixed-dimension vector. This means texts with
/// overlapping tokens will have partial vector overlap → higher
/// cosine similarity.
pub fn embed(text: &str, dim: usize) -> Vec<f32> {
    let mut vec = vec![0.0_f32; dim];

    for token in text.split_whitespace() {
        let mut hasher = DefaultHasher::new();
        token.to_lowercase().hash(&mut hasher);
        let h = hasher.finish();

        // Scatter into multiple slots for richer signal
        for i in 0..4 {
            let idx = ((h >> (i * 16)) as usize) % dim;
            let sign = if (h >> (i * 8)) & 1 == 0 { 1.0 } else { -1.0 };
            vec[idx] += sign * 0.25;
        }
    }

    // L2-normalise
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    } else {
        // Fallback: unit vector along first axis
        vec[0] = 1.0;
    }

    vec
}

/// Cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_produces_identical_vectors() {
        let a = embed("user prefers Rust", DEFAULT_DIM);
        let b = embed("user prefers Rust", DEFAULT_DIM);
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn similar_text_has_higher_similarity_than_unrelated() {
        let a = embed("user prefers Rust programming", DEFAULT_DIM);
        let b = embed("user likes Rust coding", DEFAULT_DIM);
        let c = embed("the weather is sunny today", DEFAULT_DIM);
        let sim_ab = cosine_similarity(&a, &b);
        let sim_ac = cosine_similarity(&a, &c);
        assert!(sim_ab > sim_ac, "similar texts should score higher: {sim_ab} vs {sim_ac}");
    }

    #[test]
    fn vectors_are_normalised() {
        let v = embed("hello world", DEFAULT_DIM);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
```

**Step 3: Create Harness**

Create `tests/memory_probe/harness.rs`:

```rust
//! Test harness for memory probe tests.
//!
//! Provides helpers to construct MemoryFact instances with
//! controlled timestamps, tiers, access counts, and embeddings.

use alephcore::memory::context::{
    FactSource, FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryTier,
};
use alephcore::memory::decay::{
    AccessReinforcementConfig, MemoryStrength, TierDecayParams, TieredDecayConfig,
};
use alephcore::memory::promotion::PromotionCriteria;
use alephcore::memory::scoring_pipeline::{ScoringContext, ScoringPipeline, ScoringPipelineConfig};
use alephcore::memory::store::types::ScoredFact;

use super::mock_embedding;

/// Seconds per day.
pub const DAY: i64 = 86400;

/// Reference "now" timestamp for deterministic tests.
pub const T0: i64 = 1_700_000_000;

/// Create a `MemoryFact` with controlled test values.
///
/// Uses `T0` as the baseline timestamp and sets sensible defaults
/// for fields not specified by the caller.
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
    fact.created_at = T0; // ensure recency-neutral
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
```

**Step 4: Verify compilation**

Run: `cargo test -p alephcore --test memory_probe -- --list 2>&1 | head -20`
Expected: lists the mock_embedding tests. May show compile errors for missing phase modules — that's expected; create empty stub files if needed.

Actually, since the entry point declares all 6 phase modules but they don't exist yet, create stubs:

Create these 6 empty files:
- `tests/memory_probe/extraction_classification.rs` → `//! P1: Extraction & Classification`
- `tests/memory_probe/fusion_scoring.rs` → `//! P2: Fusion & Scoring`
- `tests/memory_probe/rerank.rs` → `//! P3: Cross-Encoder Rerank`
- `tests/memory_probe/retrieval_trace.rs` → `//! P4: Retrieval Trace`
- `tests/memory_probe/decay_promotion.rs` → `//! P5: Decay & Promotion`
- `tests/memory_probe/end_to_end.rs` → `//! P6: End-to-End`

**Step 5: Run compilation check**

Run: `cargo test -p alephcore --test memory_probe -- --list`
Expected: Shows `mock_embedding::tests::identical_text_produces_identical_vectors`, etc.

**Step 6: Run the tests**

Run: `cargo test -p alephcore --test memory_probe`
Expected: 3 tests pass (the mock_embedding unit tests).

**Step 7: Commit**

```bash
git add tests/memory_probe.rs tests/memory_probe/
git commit -m "memory probe: add infrastructure — harness, mock embedding, stubs"
```

---

### Task 2: P1 — Extraction & Classification (8 scenarios)

**Files:**
- Modify: `tests/memory_probe/extraction_classification.rs`

**Dependencies:** Task 1

**Context:**
- `parse_reflection()` in `src/memory/reflection/parser.rs` — parses `## Invariants / Derived / Lessons / Open Loops` markdown
- `map_to_facts()` in `src/memory/reflection/mapper.rs` — maps parsed output to `MemoryFact` with tier/type/confidence
- `classify_invariant()` in `mapper.rs` — keyword-based Preference vs Personal
- `should_reflect()` in `src/memory/reflection/service.rs` — gate logic with 4 conditions
- `parse_lesson()` (private, tested via `parse_reflection`) — "symptom: cause → fix" format
- `LessonItem` has fields: symptom, cause, resolution
- `ReflectionConfig` has: enabled, min_turns (5), min_user_chars (200), cooldown_minutes (30)

**Step 1: Write all 8 test scenarios**

Replace `tests/memory_probe/extraction_classification.rs`:

```rust
//! P1: Extraction & Classification — 8 scenarios
//!
//! Tests the reflection parser, mapper, and gate logic.
//! Pure logic tests, no LanceDB or async.

use alephcore::config::types::memory::ReflectionConfig;
use alephcore::memory::context::{FactType, MemoryCategory, MemoryLayer, MemoryTier};
use alephcore::memory::reflection::mapper::{classify_invariant, map_to_facts};
use alephcore::memory::reflection::parser::parse_reflection;
use alephcore::memory::reflection::service::should_reflect;

// ============================================================
// p1_01: reflection_parses_all_four_sections
// ============================================================

#[test]
fn p1_01_reflection_parses_all_four_sections() {
    let md = "\
## Invariants
- User prefers dark mode
- User works on Aleph project

## Derived
- Session focused on memory optimization
- Token budget was tight

## Lessons
- UTF-8 slicing: byte index panics on CJK → use char_indices
- Lock poisoning: unwrap cascades → use unwrap_or_else

## Open Loops
- Finish compression daemon tuning
- Benchmark retrieval latency
- Review PR #42
";
    let out = parse_reflection(md);
    assert_eq!(out.invariants.len(), 2, "Expected 2 invariants");
    assert_eq!(out.derived.len(), 2, "Expected 2 derived");
    assert_eq!(out.lessons.len(), 2, "Expected 2 lessons");
    assert_eq!(out.open_loops.len(), 3, "Expected 3 open loops");
}

// ============================================================
// p1_02: reflection_skips_placeholders_and_empty_lines
// ============================================================

#[test]
fn p1_02_reflection_skips_placeholders_and_empty_lines() {
    let md = "\
## Invariants
- (none)
- Real invariant item
- (none captured)
-

## Derived

## Lessons
- (none)

## Open Loops
- (None)
";
    let out = parse_reflection(md);
    assert_eq!(out.invariants.len(), 1, "Only real item should survive");
    assert_eq!(out.invariants[0], "Real invariant item");
    assert!(out.derived.is_empty(), "Empty section should yield 0 items");
    assert!(out.lessons.is_empty(), "Placeholder-only section should be empty");
    assert!(out.open_loops.is_empty(), "Placeholder-only open loops should be empty");
}

// ============================================================
// p1_03: lesson_parses_symptom_cause_fix_format
// ============================================================

#[test]
fn p1_03_lesson_parses_symptom_cause_fix_format() {
    let md = "\
## Lessons
- UTF-8 slicing: byte index panics on CJK → use char_indices
";
    let out = parse_reflection(md);
    assert_eq!(out.lessons.len(), 1);
    let lesson = &out.lessons[0];
    assert_eq!(lesson.symptom, "UTF-8 slicing");
    assert_eq!(lesson.cause, "byte index panics on CJK");
    assert_eq!(lesson.resolution, "use char_indices");
}

// ============================================================
// p1_04: lesson_fallback_unstructured_text
// ============================================================

#[test]
fn p1_04_lesson_fallback_unstructured_text() {
    let md = "\
## Lessons
- always test with real database
";
    let out = parse_reflection(md);
    assert_eq!(out.lessons.len(), 1);
    let lesson = &out.lessons[0];
    assert_eq!(lesson.symptom, "always test with real database");
    assert!(lesson.cause.is_empty(), "No colon → empty cause");
    assert!(lesson.resolution.is_empty(), "No arrow → empty resolution");
}

// ============================================================
// p1_05: mapper_invariant_to_core_tier
// ============================================================

#[test]
fn p1_05_mapper_invariant_to_core_tier() {
    let md = "\
## Invariants
- User prefers dark mode
";
    let out = parse_reflection(md);
    let facts = map_to_facts(&out);

    assert_eq!(facts.len(), 1);
    let f = &facts[0];
    assert_eq!(f.tier, MemoryTier::Core, "Invariants → Core tier");
    assert_eq!(
        f.fact_type,
        FactType::Preference,
        "'prefers' keyword → Preference type"
    );
    assert!((f.confidence - 0.85).abs() < f32::EPSILON, "Invariant confidence=0.85");
    assert_eq!(f.layer, MemoryLayer::L1Overview);
}

// ============================================================
// p1_06: mapper_derived_to_short_term_tier
// ============================================================

#[test]
fn p1_06_mapper_derived_to_short_term_tier() {
    let md = "\
## Derived
- Session focused on memory optimization
";
    let out = parse_reflection(md);
    let facts = map_to_facts(&out);

    assert_eq!(facts.len(), 1);
    let f = &facts[0];
    assert_eq!(f.tier, MemoryTier::ShortTerm, "Derived → ShortTerm");
    assert_eq!(f.fact_type, FactType::Other, "Derived → Other type");
    assert!((f.confidence - 0.70).abs() < f32::EPSILON, "Derived confidence=0.70");
    assert_eq!(f.layer, MemoryLayer::L2Detail);
}

// ============================================================
// p1_07: mapper_lesson_to_long_term_tier
// ============================================================

#[test]
fn p1_07_mapper_lesson_to_long_term_tier() {
    let md = "\
## Lessons
- UTF-8 slicing: byte index panics → use char_indices
";
    let out = parse_reflection(md);
    let facts = map_to_facts(&out);

    assert_eq!(facts.len(), 1);
    let f = &facts[0];
    assert_eq!(f.tier, MemoryTier::LongTerm, "Lesson → LongTerm");
    assert_eq!(f.fact_type, FactType::Lesson, "Lesson → Lesson type");
    assert!((f.confidence - 0.80).abs() < f32::EPSILON, "Lesson confidence=0.80");
    assert_eq!(f.layer, MemoryLayer::L1Overview);
    assert_eq!(f.category, MemoryCategory::Cases, "Lesson → Cases category");
}

// ============================================================
// p1_08: reflection_gate_all_conditions
// ============================================================

#[test]
fn p1_08_reflection_gate_all_conditions() {
    let enabled = ReflectionConfig {
        enabled: true,
        min_turns: 5,
        min_user_chars: 200,
        cooldown_minutes: 30,
        open_loop_tracking: false,
        open_loop_inject_prompt: false,
    };
    let disabled = ReflectionConfig {
        enabled: false,
        ..enabled.clone()
    };

    // 1. Disabled config → false
    assert!(!should_reflect(10, 500, Some(60), &disabled), "disabled → false");

    // 2. Too few turns → false
    assert!(!should_reflect(3, 500, Some(60), &enabled), "too few turns → false");

    // 3. Too few chars → false
    assert!(!should_reflect(10, 50, Some(60), &enabled), "too few chars → false");

    // 4. Cooldown not elapsed → false
    assert!(!should_reflect(10, 500, Some(10), &enabled), "cooldown → false");

    // 5. All criteria met → true
    assert!(should_reflect(10, 500, Some(60), &enabled), "all met → true");

    // 6. No prior reflection (None) → true (cooldown does not apply)
    assert!(should_reflect(10, 500, None, &enabled), "no prior → true");
}
```

**Step 2: Run the tests**

Run: `cargo test -p alephcore --test memory_probe -- extraction_classification`
Expected: 8 tests pass.

**Step 3: Commit**

```bash
git add tests/memory_probe/extraction_classification.rs
git commit -m "memory probe: P1 extraction & classification — 8 scenarios"
```

---

### Task 3: P2 — Fusion & Scoring (8 scenarios)

**Files:**
- Modify: `tests/memory_probe/fusion_scoring.rs`

**Dependencies:** Task 1

**Context:**
- `rrf_fuse(vector_results, text_results, k, bm25_bonus)` → `Vec<FusedScore>` — RRF algorithm, normalises to [0,1], sorts descending
- `weighted_fuse(vector_results, text_results, vec_weight, text_weight)` → `Vec<FusedScore>` — linear combination
- `expand(query)` → `ExpandedQuery { original, bm25_query }` — Chinese synonym injection
- RRF: `Σ 1/(k + rank + 1)` per source, `*(1+bm25_bonus)` for text-matched, normalise by max

**Step 1: Write all 8 test scenarios**

Replace `tests/memory_probe/fusion_scoring.rs`:

```rust
//! P2: Fusion & Scoring — 8 scenarios
//!
//! Tests RRF/Weighted fusion math and query expansion behavior.
//! Pure logic tests, no LanceDB or async.

use alephcore::memory::hybrid_retrieval::fusion::{rrf_fuse, weighted_fuse};
use alephcore::memory::query_expander::expand;

// ============================================================
// p2_01: rrf_both_sources_overlap_ranks_highest
// ============================================================

#[test]
fn p2_01_rrf_both_sources_overlap_ranks_highest() {
    let vec_results = vec![
        ("a".into(), 0.9_f32),
        ("b".into(), 0.8),
        ("c".into(), 0.7),
    ];
    let text_results = vec![
        ("b".into(), 0.95_f32),
        ("a".into(), 0.7),
        ("d".into(), 0.5),
    ];

    let fused = rrf_fuse(&vec_results, &text_results, 60, 0.0);

    // "a" and "b" appear in both sources → should rank top-2
    let top2: Vec<&str> = fused.iter().take(2).map(|f| f.id.as_str()).collect();
    assert!(top2.contains(&"a"), "a should be in top-2: {top2:?}");
    assert!(top2.contains(&"b"), "b should be in top-2: {top2:?}");
}

// ============================================================
// p2_02: rrf_single_source_degrades_gracefully
// ============================================================

#[test]
fn p2_02_rrf_single_source_degrades_gracefully() {
    let vec_results = vec![
        ("x".into(), 0.9_f32),
        ("y".into(), 0.5),
    ];
    let text_results: Vec<(String, f32)> = vec![];

    let fused = rrf_fuse(&vec_results, &text_results, 60, 0.0);

    assert_eq!(fused.len(), 2, "Both vector-only docs preserved");
    let ids: Vec<&str> = fused.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains(&"x"));
    assert!(ids.contains(&"y"));
    // RRF scores should still be valid [0, 1]
    for f in &fused {
        assert!(f.score >= 0.0 && f.score <= 1.0, "score {} out of [0,1]", f.score);
    }
}

// ============================================================
// p2_03: rrf_bm25_bonus_breaks_tie
// ============================================================

#[test]
fn p2_03_rrf_bm25_bonus_breaks_tie() {
    // "a" rank-0 in vector only; "b" rank-1 in vector AND rank-0 in text.
    // Without bonus, "a" would lead because 1/(61) > 1/(62) from vector.
    // But "b" gets both contributions + bonus.
    let vec_results = vec![
        ("a".into(), 0.9_f32),
        ("b".into(), 0.8),
    ];
    let text_results = vec![
        ("b".into(), 0.9_f32),
    ];

    let fused = rrf_fuse(&vec_results, &text_results, 60, 0.15);

    assert_eq!(fused[0].id, "b", "BM25 bonus should push 'b' to rank 1");
}

// ============================================================
// p2_04: weighted_fusion_exact_formula
// ============================================================

#[test]
fn p2_04_weighted_fusion_exact_formula() {
    let vec_results = vec![("doc1".into(), 1.0_f32)];
    let text_results = vec![("doc1".into(), 0.5_f32)];

    let fused = weighted_fuse(&vec_results, &text_results, 0.7, 0.3);

    assert_eq!(fused.len(), 1);
    let expected = 0.7 * 1.0 + 0.3 * 0.5; // 0.85
    assert!(
        (fused[0].score - expected).abs() < 1e-5,
        "Expected {expected}, got {}",
        fused[0].score
    );
}

// ============================================================
// p2_05: rrf_normalization_max_is_one
// ============================================================

#[test]
fn p2_05_rrf_normalization_max_is_one() {
    let vec_results: Vec<(String, f32)> = (0..20)
        .map(|i| (format!("doc_{i}"), 1.0 - i as f32 * 0.05))
        .collect();
    let text_results: Vec<(String, f32)> = (0..10)
        .map(|i| (format!("doc_{i}"), 0.9 - i as f32 * 0.08))
        .collect();

    let fused = rrf_fuse(&vec_results, &text_results, 60, 0.2);

    assert!(!fused.is_empty());
    assert!(
        (fused[0].score - 1.0).abs() < 1e-5,
        "Top fused score should be normalized to 1.0, got {}",
        fused[0].score
    );
    // All scores in [0, 1]
    for f in &fused {
        assert!(f.score >= 0.0 && f.score <= 1.0 + 1e-5, "score {} not in [0,1]", f.score);
    }
}

// ============================================================
// p2_06: query_expansion_chinese_injects_synonyms
// ============================================================

#[test]
fn p2_06_query_expansion_chinese_injects_synonyms() {
    let result = expand("用户喜欢什么编程语言");

    assert_eq!(result.original, "用户喜欢什么编程语言", "original unchanged");
    assert_ne!(result.original, result.bm25_query, "bm25 should differ");
    assert!(
        result.bm25_query.contains("偏好"),
        "Should inject synonym '偏好' for '喜欢', got: {}",
        result.bm25_query
    );
}

// ============================================================
// p2_07: query_expansion_english_unchanged
// ============================================================

#[test]
fn p2_07_query_expansion_english_unchanged() {
    let result = expand("What programming language do you prefer?");
    assert_eq!(
        result.original, result.bm25_query,
        "English query should not be expanded"
    );
}

// ============================================================
// p2_08: query_expansion_no_known_keywords_unchanged
// ============================================================

#[test]
fn p2_08_query_expansion_no_known_keywords_unchanged() {
    let result = expand("天气很好");
    assert_eq!(
        result.original, result.bm25_query,
        "Chinese without known keywords should not expand"
    );
}
```

**Step 2: Run the tests**

Run: `cargo test -p alephcore --test memory_probe -- fusion_scoring`
Expected: 8 tests pass.

**Step 3: Commit**

```bash
git add tests/memory_probe/fusion_scoring.rs
git commit -m "memory probe: P2 fusion & scoring — 8 scenarios"
```

---

### Task 4: P3 — Cross-Encoder Rerank (6 scenarios)

**Files:**
- Modify: `tests/memory_probe/rerank.rs`

**Dependencies:** Task 1

**Context:**
- `blend_scores(originals, reranked, rerank_weight)` → `Vec<(String, f32)>` — formula: `weight * rerank + (1-weight) * original`
- `RerankConfig::default()` → enabled=false, provider=Jina, model="BAAI/bge-reranker-v2-m3", timeout=5000, weight=0.6
- `build_provider(config)` → `Box<dyn RerankProvider>` — factory returns correct provider
- `RerankResult { index, relevance_score }` — matched by index position in originals
- Missing rerank entry → rerank_score = 0.0
- `provider_id()` returns: "jina", "siliconflow", "voyage", "pinecone", "vllm"

**Step 1: Write all 6 test scenarios**

Replace `tests/memory_probe/rerank.rs`:

```rust
//! P3: Cross-Encoder Rerank — 6 scenarios
//!
//! Tests blend_scores math, config defaults, and provider factory.
//! Pure logic tests, no async HTTP calls.

use alephcore::memory::rerank::{
    blend_scores, build_provider, RerankConfig, RerankProviderType, RerankResult,
};

// ============================================================
// p3_01: blend_scores_applies_weight_formula
// ============================================================

#[test]
fn p3_01_blend_scores_applies_weight_formula() {
    let originals = vec![
        ("doc_a".to_string(), 0.9_f32),
        ("doc_b".to_string(), 0.4),
    ];
    let reranked = vec![
        RerankResult { index: 0, relevance_score: 0.3 },
        RerankResult { index: 1, relevance_score: 0.8 },
    ];

    let result = blend_scores(&originals, &reranked, 0.6);

    // doc_a: 0.6*0.3 + 0.4*0.9 = 0.18 + 0.36 = 0.54
    // doc_b: 0.6*0.8 + 0.4*0.4 = 0.48 + 0.16 = 0.64
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].0, "doc_b", "doc_b should rank first (higher blended)");
    assert!((result[0].1 - 0.64).abs() < 1e-5, "doc_b score = 0.64, got {}", result[0].1);
    assert!((result[1].1 - 0.54).abs() < 1e-5, "doc_a score = 0.54, got {}", result[1].1);
}

// ============================================================
// p3_02: blend_scores_missing_rerank_uses_zero
// ============================================================

#[test]
fn p3_02_blend_scores_missing_rerank_uses_zero() {
    let originals = vec![
        ("doc_a".to_string(), 0.8_f32),
        ("doc_b".to_string(), 0.5),
    ];
    // Only doc_a has a rerank result
    let reranked = vec![
        RerankResult { index: 0, relevance_score: 0.9 },
    ];

    let result = blend_scores(&originals, &reranked, 0.6);

    // doc_a: 0.6*0.9 + 0.4*0.8 = 0.54 + 0.32 = 0.86
    // doc_b: 0.6*0.0 + 0.4*0.5 = 0.00 + 0.20 = 0.20
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].0, "doc_a");
    assert!((result[0].1 - 0.86).abs() < 1e-5, "got {}", result[0].1);
    assert!((result[1].1 - 0.20).abs() < 1e-5, "got {}", result[1].1);
}

// ============================================================
// p3_03: blend_scores_preserves_order_when_weight_zero
// ============================================================

#[test]
fn p3_03_blend_scores_preserves_order_when_weight_zero() {
    let originals = vec![
        ("a".to_string(), 0.5_f32),
        ("b".to_string(), 0.9),
    ];
    let reranked = vec![
        RerankResult { index: 0, relevance_score: 1.0 },
    ];

    let result = blend_scores(&originals, &reranked, 0.0);

    // weight=0 → pure original scores: b=0.9, a=0.5
    assert_eq!(result[0].0, "b", "Original order preserved with weight=0");
    assert_eq!(result[1].0, "a");
}

// ============================================================
// p3_04: blend_scores_full_rerank_weight_ignores_original
// ============================================================

#[test]
fn p3_04_blend_scores_full_rerank_weight_ignores_original() {
    let originals = vec![
        ("a".to_string(), 0.9_f32),
        ("b".to_string(), 0.1),
    ];
    let reranked = vec![
        RerankResult { index: 1, relevance_score: 1.0 },
        // "a" not reranked → rerank_score = 0.0
    ];

    let result = blend_scores(&originals, &reranked, 1.0);

    // weight=1 → pure rerank: b=1.0, a=0.0
    assert_eq!(result[0].0, "b", "Full rerank weight flips the order");
}

// ============================================================
// p3_05: rerank_config_defaults_are_sane
// ============================================================

#[test]
fn p3_05_rerank_config_defaults_are_sane() {
    let config = RerankConfig::default();

    assert!(!config.enabled, "Should be disabled by default");
    assert_eq!(config.provider, RerankProviderType::Jina);
    assert_eq!(config.model, "BAAI/bge-reranker-v2-m3");
    assert_eq!(config.timeout_ms, 5000);
    assert!((config.rerank_weight - 0.6).abs() < f32::EPSILON);
}

// ============================================================
// p3_06: build_provider_returns_correct_type
// ============================================================

#[test]
fn p3_06_build_provider_returns_correct_type() {
    let providers = [
        (RerankProviderType::Jina, "jina"),
        (RerankProviderType::SiliconFlow, "siliconflow"),
        (RerankProviderType::Voyage, "voyage"),
        (RerankProviderType::Pinecone, "pinecone"),
        (RerankProviderType::Vllm, "vllm"),
    ];

    for (provider_type, expected_id) in &providers {
        let config = RerankConfig {
            provider: provider_type.clone(),
            ..RerankConfig::default()
        };
        let provider = build_provider(&config);
        assert_eq!(
            provider.provider_id(),
            *expected_id,
            "Provider {:?} should have id '{}'",
            provider_type,
            expected_id
        );
    }
}
```

**Step 2: Run the tests**

Run: `cargo test -p alephcore --test memory_probe -- rerank`
Expected: 6 tests pass.

**Step 3: Commit**

```bash
git add tests/memory_probe/rerank.rs
git commit -m "memory probe: P3 cross-encoder rerank — 6 scenarios"
```

---

### Task 5: P4 — Retrieval Trace (5 scenarios)

**Files:**
- Modify: `tests/memory_probe/retrieval_trace.rs`

**Dependencies:** Task 1

**Context:**
- `RetrievalTrace::new(query, timestamp)` — creates empty trace
- `trace.record_stage(name, duration_ms, input_count, scored_facts)` — appends `TraceStage`
- `TraceStage { name, duration_ms, input_count, output_count, scores: Vec<ScoreSnapshot> }`
- `ScoreSnapshot { fact_id, score, rank }` — rank = index+1
- `trace.total_duration_ms()` — sum of all stage durations
- `ScoringPipeline.run_traced(candidates, ctx, Some(&mut trace))` — records real stages
- `ScoringPipeline.run(candidates, ctx)` delegates to `run_traced(..., None)`
- Default pipeline has 7 stages: CosineRerank, RecencyBoost, ImportanceWeight, LengthNormalization, TimeDecay, HardMinScore, MmrDiversity

**Step 1: Write all 5 test scenarios**

Replace `tests/memory_probe/retrieval_trace.rs`:

```rust
//! P4: Retrieval Trace — 5 scenarios
//!
//! Tests trace recording completeness and non-interference.

use alephcore::memory::context::{FactType, MemoryFact};
use alephcore::memory::retrieval_trace::RetrievalTrace;
use alephcore::memory::scoring_pipeline::{ScoringContext, ScoringPipeline, ScoringPipelineConfig};
use alephcore::memory::store::types::ScoredFact;

use super::harness::{T0, default_ctx};

/// Create high-scoring candidates that survive the full pipeline.
fn viable_candidates() -> Vec<ScoredFact> {
    let mut facts = Vec::new();
    for (i, content) in ["alpha", "beta", "gamma"].iter().enumerate() {
        let mut f = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        f.created_at = T0;
        f.confidence = 1.0;
        facts.push(ScoredFact {
            fact: f,
            score: 0.95 - i as f32 * 0.05,
        });
    }
    facts
}

// ============================================================
// p4_01: trace_records_all_pipeline_stages
// ============================================================

#[test]
fn p4_01_trace_records_all_pipeline_stages() {
    let pipeline = ScoringPipeline::default();
    let ctx = default_ctx();
    let candidates = viable_candidates();

    let mut trace = RetrievalTrace::new("test query", T0);
    let _ = pipeline.run_traced(candidates, &ctx, Some(&mut trace));

    assert_eq!(
        trace.stages.len(),
        7,
        "Default pipeline should record 7 stages, got {}",
        trace.stages.len()
    );

    // Each stage should have a name, duration, and counts
    for stage in &trace.stages {
        assert!(!stage.name.is_empty(), "Stage name should not be empty");
        // duration_ms can be 0 for fast stages, that's fine
        // input_count/output_count should be populated
    }
}

// ============================================================
// p4_02: trace_stage_names_match_pipeline_order
// ============================================================

#[test]
fn p4_02_trace_stage_names_match_pipeline_order() {
    let pipeline = ScoringPipeline::default();
    let ctx = default_ctx();
    let candidates = viable_candidates();

    let mut trace = RetrievalTrace::new("test query", T0);
    let _ = pipeline.run_traced(candidates, &ctx, Some(&mut trace));

    let expected_order = [
        "cosine_rerank",
        "recency_boost",
        "importance_weight",
        "length_normalization",
        "time_decay",
        "hard_min_score",
        "mmr_diversity",
    ];

    let actual_names: Vec<&str> = trace.stages.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        actual_names, expected_order,
        "Stage names should match pipeline order"
    );
}

// ============================================================
// p4_03: trace_scores_descend_per_stage
// ============================================================

#[test]
fn p4_03_trace_scores_descend_per_stage() {
    let pipeline = ScoringPipeline::default();
    let ctx = default_ctx();
    let candidates = viable_candidates();

    let mut trace = RetrievalTrace::new("test query", T0);
    let _ = pipeline.run_traced(candidates, &ctx, Some(&mut trace));

    for stage in &trace.stages {
        if stage.scores.len() < 2 {
            continue; // Can't check ordering with 0 or 1 items
        }
        for window in stage.scores.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "Stage '{}': rank {} score {} should be >= rank {} score {}",
                stage.name,
                window[0].rank,
                window[0].score,
                window[1].rank,
                window[1].score,
            );
        }
    }
}

// ============================================================
// p4_04: trace_total_duration_sums_all_stages
// ============================================================

#[test]
fn p4_04_trace_total_duration_sums_all_stages() {
    let mut trace = RetrievalTrace::new("query", T0);
    trace.record_stage("s1", 10, 5, &[("f1".into(), 0.9)]);
    trace.record_stage("s2", 25, 5, &[("f1".into(), 0.8)]);
    trace.record_stage("s3", 15, 5, &[]);

    assert_eq!(trace.total_duration_ms(), 50, "10 + 25 + 15 = 50");
}

// ============================================================
// p4_05: trace_none_produces_no_side_effects
// ============================================================

#[test]
fn p4_05_trace_none_produces_no_side_effects() {
    let pipeline = ScoringPipeline::default();
    let ctx = default_ctx();

    // Run with trace
    let candidates_a = viable_candidates();
    let mut trace = RetrievalTrace::new("query", T0);
    let result_traced = pipeline.run_traced(candidates_a, &ctx, Some(&mut trace));

    // Run without trace
    let candidates_b = viable_candidates();
    let result_plain = pipeline.run(candidates_b, &ctx);

    // Same number of results
    assert_eq!(
        result_traced.len(),
        result_plain.len(),
        "Trace should not affect result count"
    );

    // Same scores (within floating point tolerance)
    for (a, b) in result_traced.iter().zip(result_plain.iter()) {
        assert!(
            (a.score - b.score).abs() < 1e-6,
            "Traced score {} should equal plain score {}",
            a.score,
            b.score
        );
    }
}
```

**Step 2: Run the tests**

Run: `cargo test -p alephcore --test memory_probe -- retrieval_trace`
Expected: 5 tests pass.

**Step 3: Commit**

```bash
git add tests/memory_probe/retrieval_trace.rs
git commit -m "memory probe: P4 retrieval trace — 5 scenarios"
```

---

### Task 6: P5 — Decay & Promotion (8 scenarios)

**Files:**
- Modify: `tests/memory_probe/decay_promotion.rs`

**Dependencies:** Task 1

**Context:**
- `effective_half_life(base, access_count, days_since_last_access, config)` → `f32`
  - 0 access → returns base
  - Formula: `freshness = exp(-days*ln2/decay_days)`, `eff_count = count*freshness`, `ext = base*factor*ln(1+eff_count)`, result = min(base+ext, base*max_multiplier)
- `MemoryStrength.calculate_strength_tiered(config, tier, fact_type, now)` → `f32`
  - Protected types → 1.0
  - Otherwise: `0.5^(days/eff_hl)`
- `AccessReinforcementConfig` defaults: factor=0.5, max_multiplier=3.0, access_decay_days=30.0
- `TieredDecayConfig` defaults: Core=90d, LongTerm=45d, ShortTerm=7d, protected=[Personal]
- `check_promotion(fact, strength, now, criteria)` → `Option<MemoryTier>`
  - ShortTerm→LongTerm: access≥3, age≥3d, strength≥0.5
  - LongTerm→Core: access≥10, age≥30d, strength≥0.7
  - Core → None always

**Step 1: Write all 8 test scenarios**

Replace `tests/memory_probe/decay_promotion.rs`:

```rust
//! P5: Decay & Promotion — 8 scenarios
//!
//! Tests tiered decay math, access reinforcement, and promotion criteria.
//! Pure logic tests, no LanceDB or async.

use alephcore::memory::context::{FactType, MemoryFact, MemoryTier};
use alephcore::memory::decay::{
    effective_half_life, AccessReinforcementConfig, MemoryStrength, TieredDecayConfig,
};
use alephcore::memory::promotion::{check_promotion, PromotionCriteria};

use super::harness::DAY;

// ============================================================
// p5_01: tiered_decay_short_term_fastest
// ============================================================

#[test]
fn p5_01_tiered_decay_short_term_fastest() {
    let config = TieredDecayConfig::default();
    let ms = MemoryStrength {
        access_count: 0,
        last_accessed: 0,
        creation_time: 0,
    };
    let now = 7 * DAY;

    let short = ms.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Other, now);
    let long = ms.calculate_strength_tiered(&config, &MemoryTier::LongTerm, &FactType::Other, now);
    let core = ms.calculate_strength_tiered(&config, &MemoryTier::Core, &FactType::Other, now);

    assert!(
        short < long,
        "ShortTerm ({short}) should decay faster than LongTerm ({long}) at 7d"
    );
    assert!(
        long < core,
        "LongTerm ({long}) should decay faster than Core ({core}) at 7d"
    );
}

// ============================================================
// p5_02: tiered_decay_half_life_inflection
// ============================================================

#[test]
fn p5_02_tiered_decay_half_life_inflection() {
    let config = TieredDecayConfig::default();
    let ms = MemoryStrength {
        access_count: 0,
        last_accessed: 0,
        creation_time: 0,
    };
    // ShortTerm half-life = 7d → at exactly 7d, strength ≈ 0.5
    let now = 7 * DAY;
    let strength =
        ms.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Other, now);

    assert!(
        (strength - 0.5).abs() < 0.05,
        "ShortTerm at 7d should be ≈0.5 (±0.05), got {strength}"
    );
}

// ============================================================
// p5_03: protected_type_never_decays
// ============================================================

#[test]
fn p5_03_protected_type_never_decays() {
    let config = TieredDecayConfig::default();
    let ms = MemoryStrength {
        access_count: 0,
        last_accessed: 0,
        creation_time: 0,
    };
    // 365 days later, Personal type should still be 1.0
    let now = 365 * DAY;
    let strength =
        ms.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Personal, now);

    assert!(
        (strength - 1.0).abs() < 0.001,
        "Protected type should always be 1.0, got {strength}"
    );
}

// ============================================================
// p5_04: access_reinforcement_extends_half_life
// ============================================================

#[test]
fn p5_04_access_reinforcement_extends_half_life() {
    let config = AccessReinforcementConfig::default();

    // 3 recent accesses (last access 1 day ago)
    let base = 7.0;
    let eff = effective_half_life(base, 3, 1.0, &config);

    assert!(
        eff > base,
        "3 recent accesses should extend half-life: base={base}, effective={eff}"
    );
    assert!(
        eff < base * config.max_multiplier,
        "Should be below cap ({}) but got {eff}",
        base * config.max_multiplier
    );
}

// ============================================================
// p5_05: access_reinforcement_stale_access_fades
// ============================================================

#[test]
fn p5_05_access_reinforcement_stale_access_fades() {
    let config = AccessReinforcementConfig::default();
    let base = 7.0;

    // 50 accesses but 90 days ago → freshness very low
    let eff_stale = effective_half_life(base, 50, 90.0, &config);
    // 3 accesses just yesterday
    let eff_recent = effective_half_life(base, 3, 1.0, &config);

    // Stale access should barely extend
    assert!(
        eff_stale < eff_recent,
        "50 stale accesses ({eff_stale}) should extend less than 3 recent ({eff_recent})"
    );
    // Stale should be close to base
    assert!(
        eff_stale < base * 1.5,
        "50 stale accesses should barely extend base: got {eff_stale}"
    );
}

// ============================================================
// p5_06: access_reinforcement_capped_at_max_multiplier
// ============================================================

#[test]
fn p5_06_access_reinforcement_capped_at_max_multiplier() {
    let config = AccessReinforcementConfig::default();
    let base = 7.0;

    // 1000 accesses just now → should hit cap
    let eff = effective_half_life(base, 1000, 0.0, &config);

    let cap = base * config.max_multiplier; // 7 * 3 = 21
    assert!(
        (eff - cap).abs() < 0.01,
        "Should be capped at {cap}, got {eff}"
    );
}

// ============================================================
// p5_07: promotion_short_to_long_all_criteria
// ============================================================

#[test]
fn p5_07_promotion_short_to_long_all_criteria() {
    let criteria = PromotionCriteria::default();
    let now = 100 * DAY;

    // Helper to make a ShortTerm fact
    let make = |access: u32, age_days: i64| -> MemoryFact {
        MemoryFact::new("test".into(), FactType::Other, vec![])
            .with_tier(MemoryTier::ShortTerm)
            .with_access_count(access)
            .with_created_at(now - age_days * DAY)
    };

    // 1. All criteria met → promotes
    let fact = make(5, 10);
    assert_eq!(
        check_promotion(&fact, 0.6, now, &criteria),
        Some(MemoryTier::LongTerm),
        "All criteria met → LongTerm"
    );

    // 2. Access too low (1 < 3) → stays
    let fact = make(1, 10);
    assert_eq!(
        check_promotion(&fact, 0.6, now, &criteria),
        None,
        "Too few accesses → None"
    );

    // 3. Too young (1d < 3d) → stays
    let fact = make(5, 1);
    assert_eq!(
        check_promotion(&fact, 0.6, now, &criteria),
        None,
        "Too young → None"
    );

    // 4. Strength too low (0.3 < 0.5) → stays
    let fact = make(5, 10);
    assert_eq!(
        check_promotion(&fact, 0.3, now, &criteria),
        None,
        "Strength too low → None"
    );

    // 5. Core never promotes
    let fact = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::Core)
        .with_access_count(100)
        .with_created_at(now - 365 * DAY);
    assert_eq!(
        check_promotion(&fact, 0.9, now, &criteria),
        None,
        "Core → never promotes"
    );
}

// ============================================================
// p5_08: promotion_long_to_core_threshold
// ============================================================

#[test]
fn p5_08_promotion_long_to_core_threshold() {
    let criteria = PromotionCriteria::default();
    let now = 100 * DAY;

    // LongTerm→Core: access≥10, age≥30d, strength≥0.7
    let fact_meets = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::LongTerm)
        .with_access_count(15)
        .with_created_at(now - 60 * DAY);
    assert_eq!(
        check_promotion(&fact_meets, 0.8, now, &criteria),
        Some(MemoryTier::Core),
        "All criteria met → Core"
    );

    // Insufficient access (5 < 10) → stays
    let fact_low = MemoryFact::new("test".into(), FactType::Other, vec![])
        .with_tier(MemoryTier::LongTerm)
        .with_access_count(5)
        .with_created_at(now - 60 * DAY);
    assert_eq!(
        check_promotion(&fact_low, 0.8, now, &criteria),
        None,
        "Too few accesses → None"
    );
}
```

**Step 2: Run the tests**

Run: `cargo test -p alephcore --test memory_probe -- decay_promotion`
Expected: 8 tests pass.

**Step 3: Commit**

```bash
git add tests/memory_probe/decay_promotion.rs
git commit -m "memory probe: P5 decay & promotion — 8 scenarios"
```

---

### Task 7: P6 — End-to-End (6 scenarios)

**Files:**
- Modify: `tests/memory_probe/end_to_end.rs`

**Dependencies:** Tasks 1–6

**Context:**
- This phase wires multiple systems together: ScoringPipeline + RetrievalTrace + TieredDecay + Promotion + Reflection
- Uses real `ScoringPipeline::default()` (7 stages)
- Uses real `TieredDecayConfig::default()` and `PromotionCriteria::default()`
- Uses the reflection parser → mapper → check_promotion chain
- All tests are sync (pipeline is sync) — no LanceDB needed since we're testing logic integration

**Step 1: Write all 6 test scenarios**

Replace `tests/memory_probe/end_to_end.rs`:

```rust
//! P6: End-to-End — 6 scenarios
//!
//! Full lifecycle integration through production components.
//! Tests combine multiple subsystems: pipeline + trace + decay + promotion + reflection.

use alephcore::memory::context::{FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryTier};
use alephcore::memory::decay::{MemoryStrength, TieredDecayConfig};
use alephcore::memory::hybrid_retrieval::fusion::rrf_fuse;
use alephcore::memory::promotion::{check_promotion, PromotionCriteria};
use alephcore::memory::reflection::mapper::map_to_facts;
use alephcore::memory::reflection::parser::parse_reflection;
use alephcore::memory::retrieval_trace::RetrievalTrace;
use alephcore::memory::scoring_pipeline::{ScoringContext, ScoringPipeline, ScoringPipelineConfig};
use alephcore::memory::store::types::ScoredFact;

use super::harness::{DAY, T0};

// ============================================================
// p6_01: fact_survives_full_pipeline_cycle
// ============================================================

#[test]
fn p6_01_fact_survives_full_pipeline_cycle() {
    let pipeline = ScoringPipeline::default();
    let ctx = ScoringContext {
        query: "user preferences".to_string(),
        query_embedding: None,
        timestamp: T0,
        config: ScoringPipelineConfig::default(),
    };

    let mut facts = Vec::new();
    for (i, content) in [
        "User prefers Rust for systems programming",
        "User likes dark mode in IDE",
        "User works on Aleph project",
    ]
    .iter()
    .enumerate()
    {
        let mut f = MemoryFact::new(content.to_string(), FactType::Preference, vec![]);
        f.created_at = T0;
        f.confidence = 1.0;
        facts.push(ScoredFact {
            fact: f,
            score: 0.95 - i as f32 * 0.03,
        });
    }

    let results = pipeline.run(facts, &ctx);

    assert!(
        !results.is_empty(),
        "High-scoring recent facts should survive the full pipeline"
    );
    for r in &results {
        assert!(r.score > 0.0, "Surviving facts should have positive scores");
    }
}

// ============================================================
// p6_02: rrf_fusion_outperforms_single_source
// ============================================================

#[test]
fn p6_02_rrf_fusion_outperforms_single_source() {
    // "doc_star" appears in both vector and text results
    let vec_results = vec![
        ("doc_star".into(), 0.9_f32),
        ("doc_vec_only".into(), 0.85),
        ("doc_low".into(), 0.5),
    ];
    let text_results = vec![
        ("doc_star".into(), 0.95_f32),
        ("doc_text_only".into(), 0.8),
    ];

    let fused = rrf_fuse(&vec_results, &text_results, 60, 0.15);

    // "doc_star" should be rank 1 because it appears in both sources
    assert_eq!(fused[0].id, "doc_star", "Dual-source doc should rank first");

    // "doc_star" should appear in top-3 of the fused results
    let top3_ids: Vec<&str> = fused.iter().take(3).map(|f| f.id.as_str()).collect();
    assert!(
        top3_ids.contains(&"doc_star"),
        "Dual-source doc should be in top-3"
    );
}

// ============================================================
// p6_03: tiered_decay_reshuffles_ranking_over_time
// ============================================================

#[test]
fn p6_03_tiered_decay_reshuffles_ranking_over_time() {
    let config = TieredDecayConfig::default();

    // Short-term fact created at T0
    let ms_short = MemoryStrength {
        access_count: 0,
        last_accessed: T0,
        creation_time: T0,
    };

    // Core fact also created at T0
    let ms_core = MemoryStrength {
        access_count: 0,
        last_accessed: T0,
        creation_time: T0,
    };

    // At T0 + 14 days: ShortTerm (hl=7d) should be heavily decayed, Core (hl=90d) barely
    let now = T0 + 14 * DAY;
    let s_short =
        ms_short.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Other, now);
    let s_core =
        ms_core.calculate_strength_tiered(&config, &MemoryTier::Core, &FactType::Other, now);

    assert!(
        s_core > s_short,
        "After 14d, Core ({s_core}) should outrank ShortTerm ({s_short})"
    );
    assert!(s_short < 0.3, "ShortTerm at 14d (2× half-life) should be < 0.3: {s_short}");
    assert!(s_core > 0.85, "Core at 14d should be > 0.85: {s_core}");
}

// ============================================================
// p6_04: access_reinforcement_keeps_popular_fact_alive
// ============================================================

#[test]
fn p6_04_access_reinforcement_keeps_popular_fact_alive() {
    let config = TieredDecayConfig::default();
    let now = T0 + 14 * DAY;

    // Unaccessed ShortTerm fact
    let ms_cold = MemoryStrength {
        access_count: 0,
        last_accessed: T0,
        creation_time: T0,
    };

    // Same tier but accessed 3 times recently
    let ms_hot = MemoryStrength {
        access_count: 3,
        last_accessed: T0 + 13 * DAY, // accessed yesterday
        creation_time: T0,
    };

    let s_cold =
        ms_cold.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Other, now);
    let s_hot =
        ms_hot.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Other, now);

    assert!(
        s_hot > s_cold,
        "Accessed fact ({s_hot}) should have higher strength than unaccessed ({s_cold})"
    );
}

// ============================================================
// p6_05: promotion_upgrades_tier_after_criteria_met
// ============================================================

#[test]
fn p6_05_promotion_upgrades_tier_after_criteria_met() {
    let criteria = PromotionCriteria::default();
    let now = T0 + 10 * DAY;

    // ShortTerm fact: access=5, age=10d, strength=0.6 → should promote to LongTerm
    let fact = MemoryFact::new("popular pattern".to_string(), FactType::Other, vec![])
        .with_tier(MemoryTier::ShortTerm)
        .with_access_count(5)
        .with_created_at(T0);

    let result = check_promotion(&fact, 0.6, now, &criteria);
    assert_eq!(
        result,
        Some(MemoryTier::LongTerm),
        "Should promote ShortTerm → LongTerm"
    );

    // After promotion (simulate by changing tier), re-check should return None
    // (LongTerm→Core needs access≥10, age≥30d, strength≥0.7)
    let promoted = MemoryFact::new("popular pattern".to_string(), FactType::Other, vec![])
        .with_tier(MemoryTier::LongTerm)
        .with_access_count(5)
        .with_created_at(T0);

    let result2 = check_promotion(&promoted, 0.6, now, &criteria);
    assert_eq!(
        result2, None,
        "Freshly promoted LongTerm should not immediately promote to Core"
    );
}

// ============================================================
// p6_06: reflection_to_storage_round_trip
// ============================================================

#[test]
fn p6_06_reflection_to_storage_round_trip() {
    // Simulate LLM reflection output
    let md = "\
## Invariants
- User prefers Rust for all systems work

## Derived
- Currently focused on memory optimization

## Lessons
- Lock poisoning: unwrap cascades panics → use unwrap_or_else

## Open Loops
- Benchmark retrieval latency
";

    // Step 1: Parse
    let parsed = parse_reflection(md);
    assert_eq!(parsed.invariants.len(), 1);
    assert_eq!(parsed.derived.len(), 1);
    assert_eq!(parsed.lessons.len(), 1);
    assert_eq!(parsed.open_loops.len(), 1);

    // Step 2: Map to facts
    let facts = map_to_facts(&parsed);
    assert_eq!(facts.len(), 3, "3 facts (open loops excluded)");

    // Step 3: Verify invariant → Core tier, Preference type (keyword "prefers")
    let invariant = &facts[0];
    assert_eq!(invariant.tier, MemoryTier::Core);
    assert_eq!(invariant.fact_type, FactType::Preference);
    assert!((invariant.confidence - 0.85).abs() < f32::EPSILON);

    // Step 4: Verify derived → ShortTerm tier
    let derived = &facts[1];
    assert_eq!(derived.tier, MemoryTier::ShortTerm);
    assert_eq!(derived.fact_type, FactType::Other);
    assert!((derived.confidence - 0.70).abs() < f32::EPSILON);

    // Step 5: Verify lesson → LongTerm tier, Lesson type, Cases category
    let lesson = &facts[2];
    assert_eq!(lesson.tier, MemoryTier::LongTerm);
    assert_eq!(lesson.fact_type, FactType::Lesson);
    assert!((lesson.confidence - 0.80).abs() < f32::EPSILON);
    assert_eq!(lesson.category, MemoryCategory::Cases);
    assert!(lesson.content.contains("Lock poisoning"));
    assert!(lesson.content.contains("unwrap_or_else"));

    // Step 6: Verify open loops are separate (not in facts)
    assert!(
        facts.iter().all(|f| !f.content.contains("Benchmark")),
        "Open loops should not appear in facts"
    );
}
```

**Step 2: Run the tests**

Run: `cargo test -p alephcore --test memory_probe -- end_to_end`
Expected: 6 tests pass.

**Step 3: Run the full probe suite**

Run: `cargo test -p alephcore --test memory_probe`
Expected: All 41 tests pass (3 mock_embedding + 8 P1 + 8 P2 + 6 P3 + 5 P4 + 8 P5 + 6 P6 = 44, including the 3 mock tests).

**Step 4: Commit**

```bash
git add tests/memory_probe/end_to_end.rs
git commit -m "memory probe: P6 end-to-end — 6 scenarios"
```

---

## Summary

| Task | Phase | Scenarios | Files |
|------|-------|-----------|-------|
| 1 | Infrastructure | 3 (mock tests) | `memory_probe.rs`, `harness.rs`, `mock_embedding.rs`, 6 stubs |
| 2 | P1: Extraction & Classification | 8 | `extraction_classification.rs` |
| 3 | P2: Fusion & Scoring | 8 | `fusion_scoring.rs` |
| 4 | P3: Cross-Encoder Rerank | 6 | `rerank.rs` |
| 5 | P4: Retrieval Trace | 5 | `retrieval_trace.rs` |
| 6 | P5: Decay & Promotion | 8 | `decay_promotion.rs` |
| 7 | P6: End-to-End | 6 | `end_to_end.rs` |
| **Total** | | **44** (41 design + 3 mock) | 9 files |

**Run all:** `cargo test -p alephcore --test memory_probe`

**Parallelism:** Tasks 2–6 are independent (all depend only on Task 1). Task 7 depends on all prior tasks being committed but can be coded in parallel.
