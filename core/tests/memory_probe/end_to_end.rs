//! P6: End-to-End
//!
//! 6 scenarios that cross subsystem boundaries — scoring pipeline,
//! RRF fusion, tiered decay, access reinforcement, tier promotion,
//! and reflection-to-storage round trip.

use alephcore::memory::context::{
    FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryTier,
};
use alephcore::memory::decay::{MemoryStrength, TieredDecayConfig};
use alephcore::memory::hybrid_retrieval::fusion::rrf_fuse;
use alephcore::memory::promotion::{check_promotion, PromotionCriteria};
use alephcore::memory::reflection::mapper::map_to_facts;
use alephcore::memory::reflection::parser::parse_reflection;
use alephcore::memory::scoring_pipeline::{ScoringContext, ScoringPipeline, ScoringPipelineConfig};
use alephcore::memory::store::types::ScoredFact;

use super::harness::{DAY, T0};

// ---------------------------------------------------------------------------
// p6_01: Full pipeline cycle
// ---------------------------------------------------------------------------

/// Insert 3 high-scoring recent facts into the default 7-stage pipeline.
/// All should survive with positive scores.
#[test]
fn p6_01_fact_survives_full_pipeline_cycle() {
    let pipeline = ScoringPipeline::default();

    let scores = [0.95_f32, 0.92, 0.89];
    let labels = ["alpha", "beta", "gamma"];

    let candidates: Vec<ScoredFact> = labels
        .iter()
        .zip(scores.iter())
        .map(|(label, &score)| {
            let mut fact = MemoryFact::new(label.to_string(), FactType::Other, vec![]);
            fact.created_at = T0;
            fact.confidence = 1.0;
            ScoredFact { fact, score }
        })
        .collect();

    let ctx = ScoringContext {
        query: "test query".to_string(),
        query_embedding: None,
        timestamp: T0,
        config: ScoringPipelineConfig::default(),
    };

    let result = pipeline.run(candidates, &ctx);

    assert!(
        !result.is_empty(),
        "High-scoring recent facts should survive the full pipeline"
    );
    for sf in &result {
        assert!(
            sf.score > 0.0,
            "Surviving fact '{}' should have positive score, got {}",
            sf.fact.content,
            sf.score
        );
    }
}

// ---------------------------------------------------------------------------
// p6_02: RRF fusion outperforms single source
// ---------------------------------------------------------------------------

/// "doc_star" appears in both vector and text sources. After rrf_fuse
/// with a BM25 bonus it should rank first.
#[test]
fn p6_02_rrf_fusion_outperforms_single_source() {
    let vector_results: Vec<(String, f32)> = vec![
        ("doc_only_vec".into(), 0.95),
        ("doc_star".into(), 0.90),
        ("doc_c".into(), 0.70),
    ];
    let text_results: Vec<(String, f32)> = vec![
        ("doc_star".into(), 0.92),
        ("doc_only_text".into(), 0.85),
    ];

    let fused = rrf_fuse(&vector_results, &text_results, 60, 0.3);

    assert!(!fused.is_empty());
    assert_eq!(
        fused[0].id, "doc_star",
        "doc_star (in both sources + BM25 bonus) should rank first, got '{}'",
        fused[0].id
    );

    // doc_star should have the highest (normalised) score = 1.0
    assert!(
        (fused[0].score - 1.0).abs() < f32::EPSILON,
        "Top fused score should be normalised to 1.0, got {}",
        fused[0].score
    );
}

// ---------------------------------------------------------------------------
// p6_03: Tiered decay reshuffles ranking over time
// ---------------------------------------------------------------------------

/// After 14 days: Core (half-life=90d) should still be strong (>0.85),
/// while ShortTerm (half-life=7d) should have decayed below 0.3.
#[test]
fn p6_03_tiered_decay_reshuffles_ranking_over_time() {
    let config = TieredDecayConfig::default();
    let now = 14 * DAY; // 14 days after creation at t=0

    let ms = MemoryStrength {
        access_count: 0,
        last_accessed: 0,
        creation_time: 0,
    };

    let s_core =
        ms.calculate_strength_tiered(&config, &MemoryTier::Core, &FactType::Other, now);
    let s_short =
        ms.calculate_strength_tiered(&config, &MemoryTier::ShortTerm, &FactType::Other, now);

    assert!(
        s_core > 0.85,
        "Core strength at 14d should be > 0.85, got {s_core}"
    );
    assert!(
        s_short < 0.3,
        "ShortTerm strength at 14d should be < 0.3, got {s_short}"
    );
    assert!(
        s_core > s_short,
        "Core ({s_core}) should outrank ShortTerm ({s_short}) after 14d"
    );
}

// ---------------------------------------------------------------------------
// p6_04: Access reinforcement keeps popular fact alive
// ---------------------------------------------------------------------------

/// A fact with 3 recent accesses should have higher strength than
/// an unaccessed peer at the same tier and age.
#[test]
fn p6_04_access_reinforcement_keeps_popular_fact_alive() {
    let config = TieredDecayConfig::default();
    let now = 7 * DAY;

    // Unaccessed peer
    let ms_cold = MemoryStrength {
        access_count: 0,
        last_accessed: 0,
        creation_time: 0,
    };

    // 3 recent accesses (last accessed 1 day ago)
    let ms_hot = MemoryStrength {
        access_count: 3,
        last_accessed: 6 * DAY,
        creation_time: 0,
    };

    let s_cold = ms_cold.calculate_strength_tiered(
        &config,
        &MemoryTier::ShortTerm,
        &FactType::Other,
        now,
    );
    let s_hot = ms_hot.calculate_strength_tiered(
        &config,
        &MemoryTier::ShortTerm,
        &FactType::Other,
        now,
    );

    assert!(
        s_hot > s_cold,
        "Accessed fact ({s_hot}) should be stronger than unaccessed ({s_cold})"
    );
}

// ---------------------------------------------------------------------------
// p6_05: Promotion upgrades tier after criteria met
// ---------------------------------------------------------------------------

/// A ShortTerm fact that meets all criteria should promote to LongTerm.
/// A freshly promoted LongTerm fact (access=5, age=5d) should NOT
/// immediately promote to Core.
#[test]
fn p6_05_promotion_upgrades_tier_after_criteria_met() {
    let criteria = PromotionCriteria::default();

    // ShortTerm fact: access=5, age=5 days, strength=0.6 → exceeds S→L thresholds
    let short_fact = MemoryFact::new("test fact".to_string(), FactType::Other, vec![])
        .with_tier(MemoryTier::ShortTerm)
        .with_access_count(5)
        .with_created_at(T0 - 5 * DAY);

    let result = check_promotion(&short_fact, 0.6, T0, &criteria);
    assert_eq!(
        result,
        Some(MemoryTier::LongTerm),
        "ShortTerm fact meeting criteria should promote to LongTerm"
    );

    // Freshly promoted LongTerm fact: access=5, age=5 days, strength=0.6
    // Does NOT meet L→C thresholds (need access≥10, age≥30d, strength≥0.7)
    let long_fact = MemoryFact::new("test fact".to_string(), FactType::Other, vec![])
        .with_tier(MemoryTier::LongTerm)
        .with_access_count(5)
        .with_created_at(T0 - 5 * DAY);

    let result = check_promotion(&long_fact, 0.6, T0, &criteria);
    assert_eq!(
        result, None,
        "Freshly promoted LongTerm (access=5, age=5d) should NOT promote to Core"
    );
}

// ---------------------------------------------------------------------------
// p6_06: Reflection → storage round trip
// ---------------------------------------------------------------------------

/// Parse a reflection markdown, map to facts, and verify each category:
/// - Invariant → Core / Preference / confidence=0.85
/// - Derived → ShortTerm / Other / confidence=0.70
/// - Lesson → LongTerm / Lesson / confidence=0.80 / Cases category
/// - Open loops → excluded
#[test]
fn p6_06_reflection_to_storage_round_trip() {
    let md = "\
## Invariants
- User prefers Chinese dialogue

## Derived
- Session focused on memory optimization

## Lessons
- UTF-8 slicing: byte index panics on CJK → use char_indices

## Open Loops
- Finish compression daemon tuning
";

    let output = parse_reflection(md);
    let facts = map_to_facts(&output);

    // Open loops excluded → 3 facts total
    assert_eq!(facts.len(), 3, "Should produce 3 facts (open loops excluded)");

    // Invariant: "prefers" keyword → Preference type, Core tier, confidence 0.85
    let inv = &facts[0];
    assert_eq!(inv.tier, MemoryTier::Core);
    assert_eq!(inv.fact_type, FactType::Preference);
    assert!(
        (inv.confidence - 0.85).abs() < f32::EPSILON,
        "Invariant confidence should be 0.85, got {}",
        inv.confidence
    );

    // Derived: Other type, ShortTerm tier, confidence 0.70
    let der = &facts[1];
    assert_eq!(der.tier, MemoryTier::ShortTerm);
    assert_eq!(der.fact_type, FactType::Other);
    assert!(
        (der.confidence - 0.70).abs() < f32::EPSILON,
        "Derived confidence should be 0.70, got {}",
        der.confidence
    );

    // Lesson: Lesson type, LongTerm tier, confidence 0.80, Cases category
    let les = &facts[2];
    assert_eq!(les.tier, MemoryTier::LongTerm);
    assert_eq!(les.fact_type, FactType::Lesson);
    assert!(
        (les.confidence - 0.80).abs() < f32::EPSILON,
        "Lesson confidence should be 0.80, got {}",
        les.confidence
    );
    assert_eq!(les.category, MemoryCategory::Cases);
    assert_eq!(les.layer, MemoryLayer::L1Overview);
    assert!(
        les.content.contains("UTF-8 slicing"),
        "Lesson content should preserve the original text"
    );
}
