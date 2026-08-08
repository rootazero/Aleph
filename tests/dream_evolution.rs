//! Integration test for Dream Daemon evolution upgrade.
//!
//! Tests the full signal → select → gate → validate → solidify flow
//! using in-memory/temp-dir setup without an actual LLM provider.

use std::collections::HashMap;
use tempfile::tempdir;

use alephcore::memory::dreaming::event_log::{DreamEvent, EventLog};
use alephcore::memory::dreaming::mutation_gate::MutationGate;
use alephcore::memory::dreaming::selector::{GateDecision, StrategySelector};
use alephcore::memory::dreaming::signals::{RawMetrics, SignalSnapshot};
use alephcore::memory::dreaming::strategy::DreamStrategy;
use alephcore::memory::dreaming::validation::{
    check_duplicate_hashes, run_l1_validation, DreamValidationReport, ValidationTier,
};
use alephcore::memory::dreaming::{DreamPipeline, DreamReport};
use alephcore::{DreamingConfig, MemoryDecayPolicy};

/// Full evolution cycle: signals → select → gate → validate → log.
#[tokio::test]
async fn full_evolution_cycle_consolidate() {
    let dir = tempdir().unwrap();

    // 1. Collect signals (default → low growth, low issues)
    let metrics = RawMetrics::default();
    let snapshot = SignalSnapshot::from_metrics(&metrics);

    // 2. Gate evaluation (no history → Allow)
    let gate = MutationGate::new();
    let gate_decision = gate.evaluate();
    assert!(matches!(gate_decision, GateDecision::Allow));

    // 3. Strategy selection (default → Consolidate)
    let selector = StrategySelector::new();
    let selection = selector.select(&snapshot, &gate_decision);
    assert_eq!(selection.strategy, DreamStrategy::Consolidate);

    // 4. Build pipeline (verify stages)
    let pipeline = DreamPipeline::from_strategy(
        selection.strategy,
        &DreamingConfig::default(),
        &MemoryDecayPolicy::default(),
    );
    // Consolidate: lint, review, consolidate, feedback_distill,
    // tool_failure_distill, drift, index, co_recall_edges, graph_recompute,
    // weave, mention_weave, decay, skill_lifecycle, goal_lessons_promote —
    // mirrors the authoritative name-list test
    // `pipeline_from_strategy_consolidate` in `src/memory/dreaming/mod.rs`,
    // which is the source of truth for the count.
    assert_eq!(pipeline.stages.len(), 14);
    assert_eq!(
        pipeline.stages.last().map(|s| s.name()),
        Some("goal_lessons_promote")
    );

    // 5. Validation (empty notes → passes trivially)
    let l1 = run_l1_validation(&HashMap::new());
    let l2_issues = check_duplicate_hashes(&[]);
    assert!(l1.passed);
    assert!(l2_issues.is_empty());

    // 6. Solidify (write event)
    let event_log = EventLog::new(dir.path().join("test_agent"));
    let cycle = event_log.next_cycle().await.unwrap();
    assert_eq!(cycle, 1);

    let event = DreamEvent {
        id: format!("dream_test_{}", cycle),
        cycle,
        strategy: selection.strategy,
        selection,
        gate_decision,
        report: DreamReport::default(),
        validation: DreamValidationReport {
            l1_format: l1,
            l2_consistency: ValidationTier {
                passed: true,
                checks_run: 1,
                checks_passed: 1,
                issues: vec![],
            },
            l3_semantic: None,
            l4_retrospective: None,
        },
        duration_ms: 42,
        created_at: chrono::Utc::now().timestamp(),
    };

    event_log.append(&event).await.unwrap();

    // Verify event was persisted
    let events = event_log.read_last(10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].strategy, DreamStrategy::Consolidate);
    assert!(events[0].validation.overall_ok());
}

/// High-growth scenario selects Synthesize.
#[tokio::test]
async fn high_growth_selects_synthesize() {
    let metrics = RawMetrics {
        notes_added_24h: 80,
        total_notes: 100,
        skill_notes_total: 10,
        skill_notes_recalled: 0,
        ..Default::default()
    };
    let snapshot = SignalSnapshot::from_metrics(&metrics);

    let gate = MutationGate::new();
    let gate_decision = gate.evaluate();

    let selector = StrategySelector::new();
    let selection = selector.select(&snapshot, &gate_decision);
    assert_eq!(selection.strategy, DreamStrategy::Synthesize);

    let pipeline = DreamPipeline::from_strategy(
        selection.strategy,
        &DreamingConfig::default(),
        &MemoryDecayPolicy::default(),
    );
    // Synthesize: lint, review, consolidate, note_synthesis, skill_distill,
    // feedback_distill, tool_failure_distill, workflow_proposal,
    // corpus_narrative, daily_digest
    assert_eq!(pipeline.stages.len(), 10);
    assert_eq!(pipeline.stages[4].name(), "skill_distill");
    assert_eq!(pipeline.stages[5].name(), "feedback_distill");
    assert_eq!(pipeline.stages[6].name(), "tool_failure_distill");
    assert_eq!(pipeline.stages[7].name(), "workflow_proposal");
}

/// Mutation gate forces Conserve on merge cycle.
///
/// The gate is now rebuilt from persisted cycle reports rather than
/// accumulated in-process, so this drives it the way the daemon does: fold
/// three past cycles' reports, then evaluate.
#[tokio::test]
async fn merge_cycle_forces_conserve() {
    let repeated_merge = DreamReport {
        merged_pairs: vec![("note_a".to_string(), "note_b".to_string())],
        ..Default::default()
    };
    let history = [
        repeated_merge.clone(),
        repeated_merge.clone(),
        repeated_merge,
    ];
    let gate = MutationGate::from_reports(&history);

    // After 3 cycles, the pair triggers conserve
    let gate_decision = gate.evaluate();
    assert!(matches!(gate_decision, GateDecision::Conserve { .. }));

    // Selector should respect the gate
    let snapshot = SignalSnapshot::from_metrics(&RawMetrics {
        notes_added_24h: 80,
        total_notes: 100,
        ..Default::default()
    });
    let selector = StrategySelector::new();
    let selection = selector.select(&snapshot, &gate_decision);
    assert_eq!(selection.strategy, DreamStrategy::Conserve);

    // Conserve pipeline is minimal: lint, review, index, co_recall_edges,
    // graph_recompute — mirrors `pipeline_from_strategy_conserve` in
    // `src/memory/dreaming/mod.rs`, which is the source of truth for the count.
    let pipeline = DreamPipeline::from_strategy(
        selection.strategy,
        &DreamingConfig::default(),
        &MemoryDecayPolicy::default(),
    );
    assert_eq!(pipeline.stages.len(), 5);
}

/// Personality adaptation across multiple cycles.
#[tokio::test]
async fn personality_adapts_over_cycles() {
    let mut selector = StrategySelector::new();

    // 10 successful cycles → threshold drops
    for _ in 0..10 {
        selector.record_cycle_outcome(true);
    }
    let threshold_after_success = selector.synthesize_threshold();

    // Reset and do 10 failed cycles → threshold rises
    let mut selector2 = StrategySelector::new();
    for _ in 0..10 {
        selector2.record_cycle_outcome(false);
    }
    let threshold_after_failure = selector2.synthesize_threshold();

    assert!(
        threshold_after_success < threshold_after_failure,
        "success threshold ({}) should be lower than failure threshold ({})",
        threshold_after_success,
        threshold_after_failure
    );
}

/// L1 validation catches bad frontmatter.
#[test]
fn l1_catches_bad_frontmatter() {
    let mut contents = HashMap::new();
    contents.insert(
        "learning/good".to_string(),
        "---\ncategory: learning\ntags: []\ncreated: 2026-04-17\nupdated: 2026-04-17\n---\n\n- fact\n".to_string(),
    );
    contents.insert(
        "learning/bad".to_string(),
        "no frontmatter at all".to_string(),
    );

    let tier = run_l1_validation(&contents);
    assert!(!tier.passed);
    assert_eq!(tier.checks_run, 2);
    assert_eq!(tier.checks_passed, 1);
    assert!(!tier.issues.is_empty());
}
