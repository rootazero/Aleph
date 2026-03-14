//! P4: Retrieval Trace — 5 scenarios
//!
//! Validates that `RetrievalTrace` correctly records pipeline stage
//! metadata (names, order, durations, score snapshots) and that
//! tracing is side-effect-free when disabled.

use alephcore::memory::context::{FactType, MemoryFact};
use alephcore::memory::retrieval_trace::RetrievalTrace;
use alephcore::memory::scoring_pipeline::ScoringPipeline;
use alephcore::memory::store::types::ScoredFact;

use super::harness::{default_ctx, T0};

/// Create 3 high-scoring, recent candidates that survive all 7 stages.
fn viable_candidates() -> Vec<ScoredFact> {
    ["alpha", "beta", "gamma"]
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut f = MemoryFact::new(c.to_string(), FactType::Other, vec![]);
            f.created_at = T0;
            f.confidence = 1.0;
            ScoredFact {
                fact: f,
                score: 0.95 - i as f32 * 0.05,
            }
        })
        .collect()
}

/// The expected 7 stage names in pipeline order.
const EXPECTED_STAGE_NAMES: [&str; 7] = [
    "cosine_rerank",
    "recency_boost",
    "importance_weight",
    "length_normalization",
    "time_decay",
    "hard_min_score",
    "mmr_diversity",
];

// -----------------------------------------------------------------------
// p4_01: trace records all pipeline stages
// -----------------------------------------------------------------------

#[test]
fn p4_01_trace_records_all_pipeline_stages() {
    let pipeline = ScoringPipeline::default();
    let ctx = default_ctx();
    let mut trace = RetrievalTrace::new("test query", T0);

    let _results = pipeline.run_traced(viable_candidates(), &ctx, Some(&mut trace));

    assert_eq!(
        trace.stages.len(),
        7,
        "default pipeline should record exactly 7 stages"
    );

    for (i, stage) in trace.stages.iter().enumerate() {
        assert!(
            !stage.name.is_empty(),
            "stage {} should have a non-empty name",
            i
        );
    }
}

// -----------------------------------------------------------------------
// p4_02: trace stage names match pipeline order
// -----------------------------------------------------------------------

#[test]
fn p4_02_trace_stage_names_match_pipeline_order() {
    let pipeline = ScoringPipeline::default();
    let ctx = default_ctx();
    let mut trace = RetrievalTrace::new("test query", T0);

    let _results = pipeline.run_traced(viable_candidates(), &ctx, Some(&mut trace));

    let actual_names: Vec<&str> = trace.stages.iter().map(|s| s.name.as_str()).collect();

    assert_eq!(
        actual_names,
        EXPECTED_STAGE_NAMES.to_vec(),
        "stage names must match the expected 7-stage pipeline order"
    );
}

// -----------------------------------------------------------------------
// p4_03: trace scores descend per stage
// -----------------------------------------------------------------------

#[test]
fn p4_03_trace_scores_descend_per_stage() {
    let pipeline = ScoringPipeline::default();
    let ctx = default_ctx();
    let mut trace = RetrievalTrace::new("test query", T0);

    let _results = pipeline.run_traced(viable_candidates(), &ctx, Some(&mut trace));

    for stage in &trace.stages {
        if stage.scores.len() < 2 {
            continue;
        }
        for window in stage.scores.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "stage '{}': rank {} (score {}) should >= rank {} (score {})",
                stage.name,
                window[0].rank,
                window[0].score,
                window[1].rank,
                window[1].score,
            );
        }
    }
}

// -----------------------------------------------------------------------
// p4_04: trace total_duration sums all stages
// -----------------------------------------------------------------------

#[test]
fn p4_04_trace_total_duration_sums_all_stages() {
    let mut trace = RetrievalTrace::new("manual test", T0);

    trace.record_stage("stage_a", 10, 5, &[("f1".to_string(), 0.9)]);
    trace.record_stage("stage_b", 25, 3, &[("f1".to_string(), 0.8)]);
    trace.record_stage("stage_c", 15, 2, &[("f1".to_string(), 0.7)]);

    assert_eq!(
        trace.total_duration_ms(),
        50,
        "total_duration_ms should be 10 + 25 + 15 = 50"
    );
}

// -----------------------------------------------------------------------
// p4_05: trace=None produces no side-effects
// -----------------------------------------------------------------------

#[test]
fn p4_05_trace_none_produces_no_side_effects() {
    let pipeline = ScoringPipeline::default();
    let ctx = default_ctx();

    // Run with trace
    let mut trace = RetrievalTrace::new("test query", T0);
    let traced_results = pipeline.run_traced(viable_candidates(), &ctx, Some(&mut trace));

    // Run without trace (via `run`, which calls `run_traced(None)`)
    let plain_results = pipeline.run(viable_candidates(), &ctx);

    // Same number of results
    assert_eq!(
        traced_results.len(),
        plain_results.len(),
        "traced and untraced runs should return the same number of results"
    );

    // Same fact IDs in the same order
    let traced_ids: Vec<&str> = traced_results.iter().map(|sf| sf.fact.content.as_str()).collect();
    let plain_ids: Vec<&str> = plain_results.iter().map(|sf| sf.fact.content.as_str()).collect();
    assert_eq!(
        traced_ids, plain_ids,
        "traced and untraced runs should return facts in the same order"
    );

    // Same scores (within float tolerance)
    for (t, p) in traced_results.iter().zip(plain_results.iter()) {
        assert!(
            (t.score - p.score).abs() < 1e-6,
            "scores should match: traced={} vs plain={}",
            t.score,
            p.score,
        );
    }
}
