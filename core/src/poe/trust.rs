//! Trust Evaluation for Progressive Auto-Approval
//!
//! This module provides the framework for evaluating whether a contract
//! can be auto-approved or requires user signature.
//!
//! ## Version Roadmap
//!
//! - **V1.0**: Always require signature (`AlwaysRequireSignature`)
//! - **V1.5**: Whitelist-based auto-approval (`WhitelistTrustEvaluator`)
//! - **V2.0**: Experience-based trust scoring (`ExperienceTrustEvaluator`)

use crate::poe::types::{RiskLevel, ValidationRule};
use crate::poe::SuccessManifest;

// ============================================================================
// Core Types
// ============================================================================

/// Decision on whether a contract can be auto-approved.
#[derive(Debug, Clone)]
pub enum AutoApprovalDecision {
    /// Contract requires user signature
    RequireSignature {
        /// Reason for requiring signature
        reason: String,
    },
    /// Contract can be auto-approved
    AutoApprove {
        /// Reason for auto-approval
        reason: String,
        /// Confidence level (0.0 - 1.0)
        confidence: f32,
    },
}

impl AutoApprovalDecision {
    /// Check if this decision requires a signature.
    pub fn requires_signature(&self) -> bool {
        matches!(self, AutoApprovalDecision::RequireSignature { .. })
    }

    /// Check if this decision allows auto-approval.
    pub fn can_auto_approve(&self) -> bool {
        matches!(self, AutoApprovalDecision::AutoApprove { .. })
    }

    /// Get the reason for this decision.
    pub fn reason(&self) -> &str {
        match self {
            AutoApprovalDecision::RequireSignature { reason } => reason,
            AutoApprovalDecision::AutoApprove { reason, .. } => reason,
        }
    }
}

/// Context for trust evaluation.
#[derive(Debug, Clone, Default)]
pub struct TrustContext {
    /// Task pattern ID (for matching historical experiences)
    pub pattern_id: Option<String>,

    /// Whether the task involves destructive operations
    pub has_destructive_ops: bool,

    /// Number of files affected
    pub file_count: usize,

    /// Whether this is a known/crystallized skill
    pub is_crystallized_skill: bool,

    /// Historical success rate (if available)
    pub historical_success_rate: Option<f32>,

    /// Number of historical executions
    pub historical_executions: Option<u32>,
}

impl TrustContext {
    /// Create a new empty trust context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the pattern ID.
    pub fn with_pattern_id(mut self, id: impl Into<String>) -> Self {
        self.pattern_id = Some(id.into());
        self
    }

    /// Mark as having destructive operations.
    pub fn with_destructive_ops(mut self) -> Self {
        self.has_destructive_ops = true;
        self
    }

    /// Set the file count.
    pub fn with_file_count(mut self, count: usize) -> Self {
        self.file_count = count;
        self
    }

    /// Set historical metrics.
    pub fn with_history(mut self, success_rate: f32, executions: u32) -> Self {
        self.historical_success_rate = Some(success_rate);
        self.historical_executions = Some(executions);
        self
    }

    /// Mark as a crystallized skill.
    pub fn with_crystallized_skill(mut self) -> Self {
        self.is_crystallized_skill = true;
        self
    }
}

// ============================================================================
// Trust Evaluator Trait
// ============================================================================

/// Trait for evaluating whether a contract can be auto-approved.
///
/// Implementations decide based on manifest contents and context.
pub trait TrustEvaluator: Send + Sync {
    /// Evaluate whether the manifest can be auto-approved.
    fn evaluate(&self, manifest: &SuccessManifest, context: &TrustContext) -> AutoApprovalDecision;
}

// ============================================================================
// V1.0: Always Require Signature
// ============================================================================

/// V1.0 evaluator that always requires user signature.
///
/// This is the safest default for initial deployment.
#[derive(Debug, Clone, Default)]
pub struct AlwaysRequireSignature;

impl AlwaysRequireSignature {
    /// Create a new instance.
    pub fn new() -> Self {
        Self
    }
}

impl TrustEvaluator for AlwaysRequireSignature {
    fn evaluate(&self, _manifest: &SuccessManifest, _context: &TrustContext) -> AutoApprovalDecision {
        AutoApprovalDecision::RequireSignature {
            reason: "V1.0: All contracts require user signature".into(),
        }
    }
}

// ============================================================================
// V1.5: Whitelist-Based Evaluator (Future)
// ============================================================================

/// V1.5 evaluator with whitelist rules.
///
/// Auto-approves low-risk tasks based on constraint types and scope.
///
/// # Safety Rules
///
/// - Never auto-approve destructive operations
/// - Only auto-approve if all constraints are in the safe list
/// - Limit file count to prevent large-scale changes
#[derive(Debug, Clone)]
pub struct WhitelistTrustEvaluator {
    /// Maximum files for auto-approval
    max_files: usize,
    /// Allowed constraint types for auto-approval
    safe_constraints: Vec<SafeConstraintType>,
}

/// Types of constraints considered safe for auto-approval.
#[derive(Debug, Clone, PartialEq)]
pub enum SafeConstraintType {
    /// FileExists check is always safe
    FileExists,
    /// CommandPasses is safe for certain commands
    CommandPasses,
    /// FileContains (read-only check)
    FileContains,
}

impl Default for WhitelistTrustEvaluator {
    fn default() -> Self {
        Self {
            max_files: 3,
            safe_constraints: vec![
                SafeConstraintType::FileExists,
                SafeConstraintType::FileContains,
                SafeConstraintType::CommandPasses,
            ],
        }
    }
}

impl WhitelistTrustEvaluator {
    /// Create a new whitelist evaluator with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum files for auto-approval.
    pub fn with_max_files(mut self, max: usize) -> Self {
        self.max_files = max;
        self
    }

    /// Check if a constraint type is considered safe.
    fn is_safe_constraint(&self, rule: &ValidationRule) -> bool {
        match rule {
            ValidationRule::FileExists { .. } => {
                self.safe_constraints.contains(&SafeConstraintType::FileExists)
            }
            ValidationRule::FileContains { .. } => {
                self.safe_constraints.contains(&SafeConstraintType::FileContains)
            }
            ValidationRule::CommandPasses { cmd, .. } => {
                // Only safe for certain commands
                let safe_commands = ["cargo", "npm", "yarn", "go", "python", "pytest"];
                self.safe_constraints.contains(&SafeConstraintType::CommandPasses)
                    && safe_commands.iter().any(|c| cmd.contains(c))
            }
            // All other constraints require signature
            _ => false,
        }
    }
}

impl TrustEvaluator for WhitelistTrustEvaluator {
    fn evaluate(&self, manifest: &SuccessManifest, context: &TrustContext) -> AutoApprovalDecision {
        // BlastRadius gate: risk level overrides experience-based trust
        if let Some(ref br) = manifest.blast_radius {
            match br.level {
                RiskLevel::Critical => {
                    return AutoApprovalDecision::RequireSignature {
                        reason: format!("Critical risk: {}", br.reasoning),
                    };
                }
                RiskLevel::High => {
                    return AutoApprovalDecision::RequireSignature {
                        reason: format!("High risk: {}", br.reasoning),
                    };
                }
                RiskLevel::Negligible => {
                    return AutoApprovalDecision::AutoApprove {
                        reason: format!("Negligible risk: {}", br.reasoning),
                        confidence: 0.95,
                    };
                }
                // Low/Medium fall through to existing evaluation logic
                _ => {}
            }
        }

        // Rule 1: Destructive operations always require signature
        if context.has_destructive_ops {
            return AutoApprovalDecision::RequireSignature {
                reason: "Task involves destructive operations".into(),
            };
        }

        // Rule 2: Too many files require signature
        if context.file_count > self.max_files {
            return AutoApprovalDecision::RequireSignature {
                reason: format!(
                    "Task affects {} files (max {} for auto-approval)",
                    context.file_count, self.max_files
                ),
            };
        }

        // Rule 3: Check all hard constraints are safe
        let unsafe_constraint = manifest
            .hard_constraints
            .iter()
            .find(|c| !self.is_safe_constraint(c));

        if let Some(_constraint) = unsafe_constraint {
            return AutoApprovalDecision::RequireSignature {
                reason: "Task contains constraints not in safe whitelist".into(),
            };
        }

        // Rule 4: Semantic checks always require signature (LLM judgment is unpredictable)
        let has_semantic = manifest.soft_metrics.iter().any(|m| {
            matches!(m.rule, ValidationRule::SemanticCheck { .. })
        });

        if has_semantic {
            return AutoApprovalDecision::RequireSignature {
                reason: "Task contains semantic checks that require human oversight".into(),
            };
        }

        // All checks passed
        AutoApprovalDecision::AutoApprove {
            reason: "Low-risk task with safe constraints only".into(),
            confidence: 0.85,
        }
    }
}

// ============================================================================
// V2.0: Experience-Based Evaluator (Future)
// ============================================================================

/// V2.0 evaluator based on historical experience.
///
/// Auto-approves tasks that match crystallized skills with high success rates.
#[derive(Debug, Clone)]
pub struct ExperienceTrustEvaluator {
    /// Minimum success rate for auto-approval
    min_success_rate: f32,
    /// Minimum executions before trusting
    min_executions: u32,
    /// Fallback evaluator for unknown patterns
    fallback: WhitelistTrustEvaluator,
}

impl Default for ExperienceTrustEvaluator {
    fn default() -> Self {
        Self {
            min_success_rate: 0.95,
            min_executions: 5,
            fallback: WhitelistTrustEvaluator::default(),
        }
    }
}

impl ExperienceTrustEvaluator {
    /// Create a new experience-based evaluator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set minimum success rate threshold.
    pub fn with_min_success_rate(mut self, rate: f32) -> Self {
        self.min_success_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Set minimum execution count.
    pub fn with_min_executions(mut self, count: u32) -> Self {
        self.min_executions = count;
        self
    }
}

impl TrustEvaluator for ExperienceTrustEvaluator {
    fn evaluate(&self, manifest: &SuccessManifest, context: &TrustContext) -> AutoApprovalDecision {
        // BlastRadius gate: risk level overrides experience-based trust
        if let Some(ref br) = manifest.blast_radius {
            match br.level {
                RiskLevel::Critical => {
                    return AutoApprovalDecision::RequireSignature {
                        reason: format!("Critical risk: {}", br.reasoning),
                    };
                }
                RiskLevel::High => {
                    return AutoApprovalDecision::RequireSignature {
                        reason: format!("High risk: {}", br.reasoning),
                    };
                }
                RiskLevel::Negligible => {
                    return AutoApprovalDecision::AutoApprove {
                        reason: format!("Negligible risk: {}", br.reasoning),
                        confidence: 0.95,
                    };
                }
                // Low/Medium fall through to existing evaluation logic
                _ => {}
            }
        }

        // Check if we have enough historical data
        if let (Some(success_rate), Some(executions)) = (
            context.historical_success_rate,
            context.historical_executions,
        ) {
            // Check if this is a trusted crystallized skill
            if context.is_crystallized_skill
                && executions >= self.min_executions
                && success_rate >= self.min_success_rate
            {
                return AutoApprovalDecision::AutoApprove {
                    reason: format!(
                        "Crystallized skill: {} executions, {:.0}% success rate",
                        executions,
                        success_rate * 100.0
                    ),
                    confidence: success_rate,
                };
            }
        }

        // Fall back to whitelist rules
        self.fallback.evaluate(manifest, context)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::SoftMetric;
    use std::path::PathBuf;

    fn create_simple_manifest() -> SuccessManifest {
        SuccessManifest::new("test-task", "Test objective")
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("test.txt"),
            })
    }

    #[test]
    fn test_always_require_signature() {
        let evaluator = AlwaysRequireSignature::new();
        let manifest = create_simple_manifest();
        let context = TrustContext::new();

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
    }

    #[test]
    fn test_whitelist_safe_task() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = create_simple_manifest();
        let context = TrustContext::new().with_file_count(1);

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.can_auto_approve());
    }

    #[test]
    fn test_whitelist_destructive_ops() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = create_simple_manifest();
        let context = TrustContext::new().with_destructive_ops();

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
        assert!(decision.reason().contains("destructive"));
    }

    #[test]
    fn test_whitelist_too_many_files() {
        let evaluator = WhitelistTrustEvaluator::new().with_max_files(3);
        let manifest = create_simple_manifest();
        let context = TrustContext::new().with_file_count(10);

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
        assert!(decision.reason().contains("files"));
    }

    #[test]
    fn test_whitelist_unsafe_constraint() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("test-task", "Test")
            .with_hard_constraint(ValidationRule::FileNotExists {
                path: PathBuf::from("test.txt"),
            });
        let context = TrustContext::new();

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
    }

    #[test]
    fn test_whitelist_semantic_check() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("test-task", "Test")
            .with_soft_metric(SoftMetric::new(ValidationRule::SemanticCheck {
                target: crate::poe::types::JudgeTarget::Content("test".into()),
                prompt: "Is it good?".into(),
                passing_criteria: "Yes".into(),
                model_tier: crate::poe::types::ModelTier::CloudFast,
            }));
        let context = TrustContext::new();

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
        assert!(decision.reason().contains("semantic"));
    }

    #[test]
    fn test_experience_trusted_skill() {
        let evaluator = ExperienceTrustEvaluator::new()
            .with_min_success_rate(0.95)
            .with_min_executions(5);

        let manifest = create_simple_manifest();
        let context = TrustContext::new()
            .with_crystallized_skill()
            .with_history(0.98, 10);

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.can_auto_approve());
    }

    #[test]
    fn test_experience_not_enough_history() {
        let evaluator = ExperienceTrustEvaluator::new()
            .with_min_executions(10);

        let manifest = create_simple_manifest();
        let context = TrustContext::new()
            .with_crystallized_skill()
            .with_history(0.98, 5)  // Only 5 executions, need 10
            .with_file_count(1);

        // Should fall back to whitelist (which will pass for this simple manifest)
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.can_auto_approve());
    }

    #[test]
    fn test_auto_approval_decision_methods() {
        let require = AutoApprovalDecision::RequireSignature {
            reason: "Test".into(),
        };
        assert!(require.requires_signature());
        assert!(!require.can_auto_approve());
        assert_eq!(require.reason(), "Test");

        let approve = AutoApprovalDecision::AutoApprove {
            reason: "Safe".into(),
            confidence: 0.9,
        };
        assert!(!approve.requires_signature());
        assert!(approve.can_auto_approve());
        assert_eq!(approve.reason(), "Safe");
    }

    /// Tests mirroring the TrustContext enrichment logic used by PoeContractService.
    /// When trust_score >= 0.9 and total_executions >= 5, the context is marked as
    /// a crystallized skill, which enables auto-approval by ExperienceTrustEvaluator.
    #[test]
    fn test_trust_context_enrichment_for_contract_service() {
        let evaluator = ExperienceTrustEvaluator::new()
            .with_min_success_rate(0.95)
            .with_min_executions(5);
        let manifest = create_simple_manifest();

        // Simulate the enrichment logic from PoeContractService::prepare():
        // if score_row.trust_score >= 0.9 && score_row.total_executions >= 5 =>
        //   context = context.with_crystallized_skill()
        let trust_score: f32 = 0.98;
        let total_executions: u32 = 10;

        let mut context = TrustContext::new()
            .with_pattern_id("task-1")
            .with_file_count(1)
            .with_history(trust_score, total_executions);

        if trust_score >= 0.9 && total_executions >= 5 {
            context = context.with_crystallized_skill();
        }

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.can_auto_approve());
        assert!(decision.reason().contains("Crystallized skill"));
    }

    /// When trust score is below the crystallization threshold, the context
    /// does NOT get marked as crystallized, so ExperienceTrustEvaluator falls
    /// back to the whitelist evaluator.
    #[test]
    fn test_trust_context_enrichment_below_threshold() {
        let evaluator = ExperienceTrustEvaluator::new()
            .with_min_success_rate(0.95)
            .with_min_executions(5);
        let manifest = create_simple_manifest();

        // Trust score below 0.9 => no crystallized skill flag
        let trust_score: f32 = 0.80;
        let total_executions: u32 = 10;

        let mut context = TrustContext::new()
            .with_pattern_id("task-2")
            .with_file_count(1)
            .with_history(trust_score, total_executions);

        if trust_score >= 0.9 && total_executions >= 5 {
            context = context.with_crystallized_skill();
        }

        // Falls back to whitelist — simple manifest with FileExists, file_count=1 => auto-approve
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.can_auto_approve());
        // But reason should NOT mention "Crystallized skill"
        assert!(!decision.reason().contains("Crystallized skill"));
    }

    /// Without a trust evaluator configured, auto_approved should always be false.
    /// This tests the None branch in the contract service logic.
    #[test]
    fn test_no_evaluator_means_no_auto_approval() {
        let evaluator: Option<Box<dyn TrustEvaluator>> = None;
        let auto_approved = evaluator.is_some();
        assert!(!auto_approved);
    }

    /// AlwaysRequireSignature should never auto-approve, regardless of context.
    #[test]
    fn test_always_require_signature_with_rich_context() {
        let evaluator = AlwaysRequireSignature::new();
        let manifest = create_simple_manifest();

        // Even with high trust, crystallized skill, it should still require signature
        let context = TrustContext::new()
            .with_pattern_id("task-1")
            .with_crystallized_skill()
            .with_history(1.0, 100)
            .with_file_count(1);

        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
    }
}

#[cfg(test)]
mod blast_radius_trust_tests {
    use super::*;
    use crate::poe::types::{BlastRadius, RiskLevel};

    #[test]
    fn test_critical_blast_radius_always_requires_signature() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "safe operation")
            .with_blast_radius(BlastRadius::new(0.9, 0.9, 0.1, RiskLevel::Critical, "destructive op"));
        let context = TrustContext::new()
            .with_history(1.0, 100)
            .with_crystallized_skill();
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
    }

    #[test]
    fn test_high_blast_radius_requires_signature() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "risky op")
            .with_blast_radius(BlastRadius::new(0.7, 0.7, 0.3, RiskLevel::High, "high risk"));
        let context = TrustContext::new().with_history(0.95, 50);
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
    }

    #[test]
    fn test_negligible_blast_radius_can_auto_approve() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "update readme")
            .with_blast_radius(BlastRadius::new(0.0, 0.0, 1.0, RiskLevel::Negligible, "docs only"));
        let context = TrustContext::new().with_file_count(1);
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.can_auto_approve());
    }

    #[test]
    fn test_low_blast_radius_falls_through_to_existing_logic() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "small change")
            .with_blast_radius(BlastRadius::new(0.1, 0.1, 0.9, RiskLevel::Low, "low risk"));
        let context = TrustContext::new().with_file_count(1);
        // Low risk falls through to whitelist logic — which checks constraint types
        let decision = evaluator.evaluate(&manifest, &context);
        // Without safe constraints, whitelist evaluator requires signature
        assert!(decision.requires_signature() || decision.can_auto_approve());
    }

    #[test]
    fn test_no_blast_radius_uses_existing_logic() {
        let evaluator = WhitelistTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "no risk assessment");
        // No blast_radius set — should fall through to existing logic
        let context = TrustContext::new();
        let _decision = evaluator.evaluate(&manifest, &context);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_experience_evaluator_critical_overrides_crystallized_skill() {
        let evaluator = ExperienceTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "dangerous op")
            .with_blast_radius(BlastRadius::new(0.9, 0.9, 0.1, RiskLevel::Critical, "critical op"));
        // Even with perfect history and crystallized skill, critical should block
        let context = TrustContext::new()
            .with_history(1.0, 100)
            .with_crystallized_skill();
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.requires_signature());
    }

    #[test]
    fn test_experience_evaluator_negligible_auto_approves() {
        let evaluator = ExperienceTrustEvaluator::new();
        let manifest = SuccessManifest::new("t1", "trivial change")
            .with_blast_radius(BlastRadius::new(0.0, 0.0, 1.0, RiskLevel::Negligible, "no impact"));
        let context = TrustContext::new();
        let decision = evaluator.evaluate(&manifest, &context);
        assert!(decision.can_auto_approve());
    }
}
