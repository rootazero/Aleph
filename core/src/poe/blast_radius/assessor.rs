//! Blast radius assessor: orchestrates System 1 (static) + System 2 (semantic).
//!
//! Provides both sync (System 1 only) and async (full hybrid) assessment paths.
//! System 1 results are never downgraded by System 2.

use crate::poe::blast_radius::semantic_analyzer::SemanticRiskAnalyzer;
use crate::poe::blast_radius::static_scanner::{ScanResult, StaticSafetyScanner};
use crate::poe::types::{BlastRadius, RiskLevel, SuccessManifest};
use crate::providers::AiProvider;

// ============================================================================
// Assessment Result
// ============================================================================

/// Result of blast radius assessment.
#[derive(Debug)]
pub enum AssessmentResult {
    /// Tier 0: Hard reject, abort immediately.
    Rejected { reason: String },

    /// Risk has been computed.
    Assessed(BlastRadius),

    /// System 1 returned Indeterminate; LLM analysis required.
    /// Only returned by `assess_sync`.
    NeedsLlm,
}

impl AssessmentResult {
    /// Returns true if the result is a rejection.
    pub fn is_rejected(&self) -> bool {
        matches!(self, AssessmentResult::Rejected { .. })
    }

    /// Returns the BlastRadius if assessed, None otherwise.
    pub fn blast_radius(&self) -> Option<&BlastRadius> {
        match self {
            AssessmentResult::Assessed(br) => Some(br),
            _ => None,
        }
    }
}

// ============================================================================
// Blast Radius Assessor
// ============================================================================

/// Orchestrates System 1 (StaticSafetyScanner) and System 2 (SemanticRiskAnalyzer)
/// for comprehensive blast radius assessment.
pub struct BlastRadiusAssessor {
    scanner: StaticSafetyScanner,
}

impl BlastRadiusAssessor {
    /// Create a new assessor.
    pub fn new() -> Self {
        Self {
            scanner: StaticSafetyScanner::new(),
        }
    }

    /// System 1 only assessment. Returns `NeedsLlm` for indeterminate cases.
    pub fn assess_sync(&self, manifest: &SuccessManifest) -> AssessmentResult {
        let scan = self.scanner.scan(manifest);
        Self::map_scan_result(scan)
    }

    /// Full hybrid assessment: System 1 first, then System 2 for gray zones.
    pub async fn assess(
        &self,
        manifest: &SuccessManifest,
        provider: &dyn AiProvider,
    ) -> AssessmentResult {
        let scan = self.scanner.scan(manifest);

        match scan {
            ScanResult::Indeterminate => {
                // System 2: LLM analysis
                let system2 = SemanticRiskAnalyzer::analyze(manifest, provider).await;
                AssessmentResult::Assessed(system2)
            }
            other => Self::map_scan_result(other),
        }
    }

    /// Apply reversibility compensation: High -> Medium if conditions met.
    ///
    /// If the blast radius is High AND the workspace is in a clean git state
    /// with all files tracked, the risk can be compensated down to Medium
    /// since changes are fully reversible via git.
    pub fn apply_reversibility_compensation(
        blast_radius: BlastRadius,
        is_clean_git: bool,
        all_tracked: bool,
    ) -> BlastRadius {
        if blast_radius.level == RiskLevel::High && is_clean_git && all_tracked {
            BlastRadius::new(
                blast_radius.scope,
                blast_radius.destructiveness,
                blast_radius.reversibility.max(0.8), // Boost reversibility
                RiskLevel::Medium,
                format!(
                    "Compensated High→Medium (clean git, all tracked): {}",
                    blast_radius.reasoning
                ),
            )
        } else {
            blast_radius
        }
    }

    /// Map a ScanResult to an AssessmentResult.
    fn map_scan_result(scan: ScanResult) -> AssessmentResult {
        match scan {
            ScanResult::HardReject { reason } => AssessmentResult::Rejected { reason },

            ScanResult::MandatorySignature { reason } => {
                AssessmentResult::Assessed(BlastRadius::new(
                    0.8,
                    0.9,
                    0.2,
                    RiskLevel::Critical,
                    reason,
                ))
            }

            ScanResult::Negligible => AssessmentResult::Assessed(BlastRadius::new(
                0.0,
                0.0,
                1.0,
                RiskLevel::Negligible,
                "Documentation-only changes".to_string(),
            )),

            ScanResult::Safe => AssessmentResult::Assessed(BlastRadius::new(
                0.1,
                0.1,
                0.9,
                RiskLevel::Low,
                "Single safe operation".to_string(),
            )),

            ScanResult::Indeterminate => AssessmentResult::NeedsLlm,
        }
    }
}

impl Default for BlastRadiusAssessor {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::{SoftMetric, SuccessManifest, ValidationRule};
    use std::path::PathBuf;

    /// Helper: manifest with a single command constraint.
    fn manifest_cmd(cmd: &str, args: &[&str]) -> SuccessManifest {
        SuccessManifest::new("test", "test objective").with_hard_constraint(
            ValidationRule::CommandPasses {
                cmd: cmd.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
                timeout_ms: 30_000,
            },
        )
    }

    #[test]
    fn test_tier0_skips_system2() {
        let assessor = BlastRadiusAssessor::new();
        let manifest = manifest_cmd("rm", &["-rf", "/"]);
        let result = assessor.assess_sync(&manifest);

        assert!(result.is_rejected());
        match result {
            AssessmentResult::Rejected { reason } => {
                assert!(reason.contains("root filesystem"));
            }
            _ => panic!("Expected Rejected"),
        }
    }

    #[test]
    fn test_negligible_returns_blast_radius() {
        let assessor = BlastRadiusAssessor::new();
        let manifest = SuccessManifest::new("test", "Update docs").with_hard_constraint(
            ValidationRule::FileContains {
                path: PathBuf::from("README.md"),
                pattern: "# Project".to_string(),
            },
        );

        let result = assessor.assess_sync(&manifest);
        let br = result.blast_radius().expect("Expected Assessed");
        assert_eq!(br.level, RiskLevel::Negligible);
        assert!((br.scope - 0.0).abs() < f32::EPSILON);
        assert!((br.reversibility - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_mandatory_signature_returns_critical() {
        let assessor = BlastRadiusAssessor::new();
        let manifest = manifest_cmd("git", &["push", "--force", "origin", "main"]);
        let result = assessor.assess_sync(&manifest);

        let br = result.blast_radius().expect("Expected Assessed");
        assert_eq!(br.level, RiskLevel::Critical);
        assert!((br.scope - 0.8).abs() < f32::EPSILON);
        assert!((br.destructiveness - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_safe_returns_low() {
        let assessor = BlastRadiusAssessor::new();
        let manifest = manifest_cmd("cargo", &["test"]);
        let result = assessor.assess_sync(&manifest);

        let br = result.blast_radius().expect("Expected Assessed");
        assert_eq!(br.level, RiskLevel::Low);
        assert!((br.scope - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn test_indeterminate_needs_llm() {
        let assessor = BlastRadiusAssessor::new();
        // Multiple non-doc constraints → Indeterminate → NeedsLlm
        let manifest = SuccessManifest::new("test", "Complex task")
            .with_hard_constraint(ValidationRule::CommandPasses {
                cmd: "cargo".to_string(),
                args: vec!["build".to_string()],
                timeout_ms: 60_000,
            })
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("src/main.rs"),
            });

        let result = assessor.assess_sync(&manifest);
        assert!(
            matches!(result, AssessmentResult::NeedsLlm),
            "Expected NeedsLlm, got: {:?}",
            result
        );
    }

    #[test]
    fn test_reversibility_compensation_high_to_medium() {
        let br = BlastRadius::new(0.5, 0.6, 0.4, RiskLevel::High, "risky operation");
        let compensated =
            BlastRadiusAssessor::apply_reversibility_compensation(br, true, true);

        assert_eq!(compensated.level, RiskLevel::Medium);
        assert!(compensated.reasoning.contains("Compensated High→Medium"));
        // Reversibility should be boosted to at least 0.8
        assert!(compensated.reversibility >= 0.8);
    }

    #[test]
    fn test_reversibility_compensation_not_applied_dirty_git() {
        let br = BlastRadius::new(0.5, 0.6, 0.4, RiskLevel::High, "risky operation");

        // Not clean git → no compensation
        let result =
            BlastRadiusAssessor::apply_reversibility_compensation(br.clone(), false, true);
        assert_eq!(result.level, RiskLevel::High);

        // Not all tracked → no compensation
        let result =
            BlastRadiusAssessor::apply_reversibility_compensation(br, true, false);
        assert_eq!(result.level, RiskLevel::High);
    }

    #[test]
    fn test_reversibility_compensation_only_for_high() {
        // Medium should NOT be compensated
        let br = BlastRadius::new(0.3, 0.3, 0.7, RiskLevel::Medium, "medium risk");
        let result =
            BlastRadiusAssessor::apply_reversibility_compensation(br, true, true);
        assert_eq!(result.level, RiskLevel::Medium);

        // Critical should NOT be compensated
        let br = BlastRadius::new(0.9, 0.9, 0.1, RiskLevel::Critical, "critical risk");
        let result =
            BlastRadiusAssessor::apply_reversibility_compensation(br, true, true);
        assert_eq!(result.level, RiskLevel::Critical);
    }

    #[test]
    fn test_soft_metric_scanned() {
        let assessor = BlastRadiusAssessor::new();
        // Soft metric with sudo → MandatorySignature → Critical
        let manifest = SuccessManifest::new("test", "test")
            .with_hard_constraint(ValidationRule::FileContains {
                path: PathBuf::from("README.md"),
                pattern: "hello".to_string(),
            })
            .with_soft_metric(SoftMetric::new(ValidationRule::CommandPasses {
                cmd: "sudo".to_string(),
                args: vec!["apt".to_string(), "install".to_string()],
                timeout_ms: 30_000,
            }));

        let result = assessor.assess_sync(&manifest);
        let br = result.blast_radius().expect("Expected Assessed");
        assert_eq!(br.level, RiskLevel::Critical);
    }
}
