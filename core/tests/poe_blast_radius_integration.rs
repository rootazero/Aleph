//! Integration tests for POE Phase 1: BlastRadius + TabooBuffer.
//!
//! Tests the full pipeline: static scanning, blast radius assessment,
//! reversibility compensation, taboo buffer micro-taboo detection,
//! anti-pattern avoidance prompts, and serialization round-trips.

use alephcore::poe::{
    BlastRadius, RiskLevel, SuccessManifest, ValidationRule, Verdict,
};
use alephcore::poe::blast_radius::assessor::{AssessmentResult, BlastRadiusAssessor};
use alephcore::poe::taboo::buffer::{TabooBuffer, TaggedVerdict};
use alephcore::poe::taboo::anti_pattern::AntiPattern;

// ============================================================================
// BlastRadius Pipeline Tests
// ============================================================================

#[test]
fn test_full_blast_radius_pipeline_safe_task() {
    let assessor = BlastRadiusAssessor::new();
    let manifest = SuccessManifest::new("integration-1", "run cargo test")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "cargo".into(),
            args: vec!["test".into()],
            timeout_ms: 60_000,
        });

    let result = assessor.assess_sync(&manifest);
    match result {
        AssessmentResult::Assessed(br) => {
            assert_eq!(br.level, RiskLevel::Low);
            assert!(br.reversibility > 0.5);
        }
        other => panic!("Expected Assessed(Low), got {:?}", other),
    }
}

#[test]
fn test_full_blast_radius_pipeline_dangerous_task() {
    let assessor = BlastRadiusAssessor::new();
    let manifest = SuccessManifest::new("integration-2", "destroy everything")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "rm".into(),
            args: vec!["-rf".into(), "/".into()],
            timeout_ms: 30_000,
        });

    let result = assessor.assess_sync(&manifest);
    assert!(matches!(result, AssessmentResult::Rejected { .. }));
}

#[test]
fn test_full_blast_radius_pipeline_tier1_forces_critical() {
    let assessor = BlastRadiusAssessor::new();
    let manifest = SuccessManifest::new("integration-3", "force push")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "git".into(),
            args: vec!["push".into(), "--force".into()],
            timeout_ms: 30_000,
        });

    let result = assessor.assess_sync(&manifest);
    match result {
        AssessmentResult::Assessed(br) => {
            assert_eq!(br.level, RiskLevel::Critical);
        }
        other => panic!("Expected Assessed(Critical), got {:?}", other),
    }
}

// ============================================================================
// TabooBuffer Tests
// ============================================================================

#[test]
fn test_taboo_buffer_micro_taboo_cycle() {
    let mut buffer = TabooBuffer::new(3);

    // Simulate 3 consecutive same-type failures
    for _ in 0..3 {
        buffer.record(TaggedVerdict::new(
            Verdict::failure("compilation error in auth.rs"),
            "CompilationError",
            "cannot find type `AuthToken`",
        ));
    }

    let taboo = buffer.check_micro_taboo();
    assert!(taboo.is_some());
    let prompt = taboo.unwrap();
    assert!(prompt.contains("TABOO WARNING"));
    assert!(prompt.contains("CompilationError"));
    assert!(prompt.contains("FORBIDDEN"));
}

#[test]
fn test_taboo_buffer_no_false_positive_on_variety() {
    let mut buffer = TabooBuffer::new(3);

    buffer.record(TaggedVerdict::new(
        Verdict::failure("permission denied"),
        "PermissionDenied",
        "cannot write",
    ));
    buffer.record(TaggedVerdict::new(
        Verdict::failure("file not found"),
        "FileNotFound",
        "missing file",
    ));
    buffer.record(TaggedVerdict::new(
        Verdict::failure("timeout"),
        "Timeout",
        "command timed out",
    ));

    assert!(buffer.check_micro_taboo().is_none());
}

// ============================================================================
// AntiPattern Tests
// ============================================================================

#[test]
fn test_anti_pattern_recall_prompt() {
    let ap = AntiPattern::new(
        "poe-refactor-auth",
        "Refactoring auth module fails when middleware depends on old types",
        vec!["CompilationError".into(), "DependencyMismatch".into()],
    )
    .with_attempts(5);

    let prompt = ap.to_avoidance_prompt();
    assert!(prompt.contains("AVOID"));
    assert!(prompt.contains("middleware depends on old types"));
    assert!(prompt.contains("CompilationError"));
}

// ============================================================================
// Serialization Tests
// ============================================================================

#[test]
fn test_manifest_with_blast_radius_serialization_roundtrip() {
    let manifest = SuccessManifest::new("ser-1", "test serialization")
        .with_blast_radius(BlastRadius::new(
            0.5,
            0.3,
            0.8,
            RiskLevel::Medium,
            "moderate risk",
        ))
        .with_hard_constraint(ValidationRule::FileExists {
            path: std::path::PathBuf::from("src/main.rs"),
        });

    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(json.contains("blast_radius"));
    assert!(json.contains("Medium"));

    let deserialized: SuccessManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.blast_radius.unwrap().level, RiskLevel::Medium);
}

#[test]
fn test_manifest_without_blast_radius_backward_compatible() {
    // Old-format JSON without blast_radius field
    let json = r#"{
        "task_id": "old-1",
        "objective": "test backward compat",
        "hard_constraints": [],
        "soft_metrics": [],
        "max_attempts": 5
    }"#;

    let manifest: SuccessManifest = serde_json::from_str(json).unwrap();
    assert!(manifest.blast_radius.is_none());
}

// ============================================================================
// Reversibility Compensation Tests
// ============================================================================

#[test]
fn test_reversibility_compensation() {
    let br = BlastRadius::new(0.5, 0.6, 0.3, RiskLevel::High, "risky but reversible");
    let compensated = BlastRadiusAssessor::apply_reversibility_compensation(br, true, true);
    assert_eq!(compensated.level, RiskLevel::Medium);
    assert!(compensated.reversibility >= 0.8);
}

#[test]
fn test_reversibility_no_compensation_without_clean_git() {
    let br = BlastRadius::new(0.5, 0.6, 0.3, RiskLevel::High, "not in clean git");
    let not_compensated = BlastRadiusAssessor::apply_reversibility_compensation(br, false, true);
    assert_eq!(not_compensated.level, RiskLevel::High);
}
