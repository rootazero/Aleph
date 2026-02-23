//! Collaborative Solidification Pipeline
//!
//! Extends the basic SolidificationPipeline with collaborative evolution features:
//! 1. Generate SuccessManifest (soft constraints)
//! 2. Generate Capabilities (hard constraints)
//! 3. Validate constraints match using ConstraintValidator
//! 4. Request LLM to fix mismatches
//! 5. Queue proposals for user approval
//!
//! # Example
//!
//! ```rust,no_run
//! use alephcore::skill_evolution::collaborative_pipeline::CollaborativeSolidificationPipeline;
//! use alephcore::skill_evolution::tracker::EvolutionTracker;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let tracker = Arc::new(EvolutionTracker::new("evolution.db")?);
//! let pipeline = CollaborativeSolidificationPipeline::new(tracker);
//!
//! // Run detection and generation
//! let result = pipeline.run().await?;
//!
//! for proposal in result.proposals {
//!     println!("Skill: {}", proposal.manifest.metadata.skill_id);
//!     println!("Validation: {} errors, {} warnings",
//!         proposal.validation.errors.len(),
//!         proposal.validation.warnings.len()
//!     );
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture
//!
//! The pipeline orchestrates the collaborative evolution workflow:
//!
//! ```text
//! ┌─────────────────┐
//! │ Detect Patterns │
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │ Generate Proposal│
//! │ (Manifest + Caps)│
//! └────────┬────────┘
//!          │
//! ┌────────▼────────┐
//! │ Validate        │
//! │ Constraints     │
//! └────────┬────────┘
//!          │
//!     ┌────▼────┐
//!     │ Errors? │
//!     └────┬────┘
//!          │
//!     ┌────▼────┐
//!     │ Fix (LLM)│
//!     └────┬────┘
//!          │
//! ┌────────▼────────┐
//! │ Queue for       │
//! │ User Approval   │
//! └─────────────────┘
//! ```

use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::error::{AlephError, Result};
use crate::exec::sandbox::capabilities::{
    Capabilities, FileSystemCapability, NetworkCapability,
};
use crate::providers::AiProvider;

use super::constraint_validator::{ConstraintValidator, ValidationReport};
use super::success_manifest::SuccessManifest;
use super::tracker::EvolutionTracker;
use super::types::{SkillMetrics, SolidificationConfig};

/// A skill proposal including both soft and hard constraints
#[derive(Debug, Clone)]
pub struct SkillProposal {
    /// Unique proposal ID
    pub id: String,
    /// Success manifest (soft constraints)
    pub manifest: SuccessManifest,
    /// Capabilities (hard constraints)
    pub capabilities: Capabilities,
    /// Validation report
    pub validation: ValidationReport,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Source metrics that triggered this proposal
    pub source_metrics: SkillMetrics,
}

/// Result of a collaborative pipeline run
#[derive(Debug, Clone)]
pub struct CollaborativePipelineResult {
    /// Skill proposals generated
    pub proposals: Vec<SkillProposal>,
    /// Number of candidates detected
    pub candidates_detected: usize,
    /// Number of proposals that failed validation
    pub validation_failures: usize,
    /// Number of proposals that were fixed after validation failure
    pub fixed_proposals: usize,
}

/// Collaborative Solidification Pipeline
///
/// This pipeline extends the basic solidification with:
/// - SuccessManifest generation (soft constraints)
/// - Capabilities generation (hard constraints)
/// - Constraint validation
/// - Automatic fixing of constraint mismatches
pub struct CollaborativeSolidificationPipeline {
    /// Evolution tracker
    tracker: Arc<EvolutionTracker>,
    /// AI provider for generating manifests and fixing constraints
    provider: Option<Arc<dyn AiProvider>>,
    /// Solidification configuration
    config: SolidificationConfig,
    /// Minimum confidence threshold
    min_confidence: f32,
    /// Maximum proposals per run
    max_proposals: usize,
    /// Maximum fix attempts for validation failures
    max_fix_attempts: usize,
}

impl CollaborativeSolidificationPipeline {
    /// Create a new collaborative pipeline
    pub fn new(tracker: Arc<EvolutionTracker>) -> Self {
        Self {
            tracker,
            provider: None,
            config: SolidificationConfig {
                min_success_count: 3,
                min_success_rate: 0.7,
                min_age_days: 1,
                max_idle_days: 30,
            },
            min_confidence: 0.7,
            max_proposals: 10,
            max_fix_attempts: 3,
        }
    }

    /// Set the AI provider
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set the solidification configuration
    pub fn with_config(mut self, config: SolidificationConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the minimum confidence threshold
    pub fn with_min_confidence(mut self, threshold: f32) -> Self {
        self.min_confidence = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set the maximum number of proposals
    pub fn with_max_proposals(mut self, max: usize) -> Self {
        self.max_proposals = max.max(1);
        self
    }

    /// Set the maximum fix attempts
    pub fn with_max_fix_attempts(mut self, max: usize) -> Self {
        self.max_fix_attempts = max.max(1);
        self
    }

    /// Run the collaborative pipeline
    ///
    /// This will:
    /// 1. Detect solidification candidates
    /// 2. Generate SuccessManifest and Capabilities for each
    /// 3. Validate constraints match
    /// 4. Fix mismatches if possible
    /// 5. Return validated proposals
    pub async fn run(&self) -> Result<CollaborativePipelineResult> {
        info!("Running collaborative solidification pipeline");

        // Phase 1: Detection
        let candidates = self.tracker.get_solidification_candidates(&self.config)?;
        let candidates_count = candidates.len();
        debug!(count = candidates_count, "Detected solidification candidates");

        if candidates.is_empty() {
            return Ok(CollaborativePipelineResult {
                proposals: vec![],
                candidates_detected: 0,
                validation_failures: 0,
                fixed_proposals: 0,
            });
        }

        // Phase 2: Generation and Validation
        let mut proposals = Vec::new();
        let mut validation_failures = 0;
        let mut fixed_proposals = 0;

        for metrics in candidates.iter().take(self.max_proposals) {
            match self.generate_and_validate_proposal(metrics).await {
                Ok(proposal) => {
                    if proposal.validation.has_errors() {
                        validation_failures += 1;
                        // Try to fix the proposal
                        match self.fix_proposal(&proposal).await {
                            Ok(fixed) => {
                                if !fixed.validation.has_errors() {
                                    fixed_proposals += 1;
                                    proposals.push(fixed);
                                } else {
                                    warn!(
                                        skill_id = %metrics.skill_id,
                                        "Failed to fix proposal after {} attempts",
                                        self.max_fix_attempts
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    skill_id = %metrics.skill_id,
                                    error = %e,
                                    "Failed to fix proposal"
                                );
                            }
                        }
                    } else {
                        proposals.push(proposal);
                    }
                }
                Err(e) => {
                    warn!(
                        skill_id = %metrics.skill_id,
                        error = %e,
                        "Failed to generate proposal"
                    );
                }
            }
        }

        // Phase 3: Filtering by confidence
        proposals.retain(|p| p.confidence >= self.min_confidence);

        // Sort by confidence (highest first)
        proposals.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        info!(
            candidates = candidates_count,
            proposals = proposals.len(),
            validation_failures,
            fixed_proposals,
            "Collaborative pipeline run complete"
        );

        Ok(CollaborativePipelineResult {
            proposals,
            candidates_detected: candidates_count,
            validation_failures,
            fixed_proposals,
        })
    }

    /// Generate and validate a skill proposal
    async fn generate_and_validate_proposal(
        &self,
        metrics: &SkillMetrics,
    ) -> Result<SkillProposal> {
        // Generate SuccessManifest
        let manifest = self.generate_manifest(metrics).await?;

        // Generate Capabilities
        let capabilities = self.generate_capabilities(metrics, &manifest).await?;

        // Validate constraints
        let validation = match ConstraintValidator::validate(&manifest, &capabilities) {
            Ok(report) => report,
            Err(mismatch) => {
                // Extract the report from the error
                match mismatch {
                    super::constraint_validator::ConstraintMismatch::ValidationFailed(report) => {
                        report
                    }
                }
            }
        };

        Ok(SkillProposal {
            id: uuid::Uuid::new_v4().to_string(),
            manifest,
            capabilities,
            validation,
            confidence: metrics.success_rate(),
            source_metrics: metrics.clone(),
        })
    }

    /// Generate SuccessManifest from metrics
    async fn generate_manifest(&self, metrics: &SkillMetrics) -> Result<SuccessManifest> {
        // For now, generate a basic manifest from metrics
        // In the future, this will use LLM to generate a more detailed manifest
        let mut manifest = SuccessManifest::new(
            &metrics.skill_id,
            format!("Skill for {}", metrics.skill_id),
        );

        // Infer allowed operations from context
        // This is a simplified version - in production, we'd use LLM
        manifest.allowed_operations.filesystem.read_paths = vec![
            "/tmp/**".to_string(),
        ];
        manifest.allowed_operations.filesystem.write_paths = vec![
            "/tmp/**".to_string(),
        ];
        manifest.allowed_operations.filesystem.allow_temp_workspace = true;

        manifest.allowed_operations.script_execution.languages = vec![
            "python".to_string(),
            "bash".to_string(),
        ];

        // Set default prohibited operations
        manifest.prohibited_operations.network.prohibit_all = true;
        manifest.prohibited_operations.network.reason =
            "Default: prohibit all network access for security".to_string();

        manifest.prohibited_operations.process.prohibit_fork = true;
        manifest.prohibited_operations.process.reason =
            "Prevent spawning uncontrolled processes".to_string();

        Ok(manifest)
    }

    /// Generate Capabilities from metrics and manifest
    async fn generate_capabilities(
        &self,
        _metrics: &SkillMetrics,
        manifest: &SuccessManifest,
    ) -> Result<Capabilities> {
        // Generate capabilities that match the manifest
        let mut capabilities = Capabilities::default();

        // Filesystem capabilities
        capabilities.filesystem.clear();
        for read_path in &manifest.allowed_operations.filesystem.read_paths {
            capabilities.filesystem.push(FileSystemCapability::ReadOnly {
                path: std::path::PathBuf::from(read_path.trim_end_matches("/**").trim_end_matches("/*")),
            });
        }
        for write_path in &manifest.allowed_operations.filesystem.write_paths {
            capabilities.filesystem.push(FileSystemCapability::ReadWrite {
                path: std::path::PathBuf::from(write_path.trim_end_matches("/**").trim_end_matches("/*")),
            });
        }
        if manifest.allowed_operations.filesystem.allow_temp_workspace {
            capabilities.filesystem.push(FileSystemCapability::TempWorkspace);
        }

        // Network capabilities
        if manifest.prohibited_operations.network.prohibit_all {
            capabilities.network = NetworkCapability::Deny;
        } else {
            capabilities.network = NetworkCapability::AllowAll;
        }

        // Process capabilities
        capabilities.process.no_fork = manifest.prohibited_operations.process.prohibit_fork;

        Ok(capabilities)
    }

    /// Fix a proposal that failed validation
    async fn fix_proposal(&self, proposal: &SkillProposal) -> Result<SkillProposal> {
        let mut current_proposal = proposal.clone();

        for attempt in 0..self.max_fix_attempts {
            debug!(
                attempt = attempt + 1,
                max_attempts = self.max_fix_attempts,
                "Attempting to fix proposal"
            );

            // Analyze validation errors and fix them
            current_proposal = self.apply_fixes(&current_proposal)?;

            // Re-validate
            let validation = match ConstraintValidator::validate(
                &current_proposal.manifest,
                &current_proposal.capabilities,
            ) {
                Ok(report) => report,
                Err(mismatch) => match mismatch {
                    super::constraint_validator::ConstraintMismatch::ValidationFailed(report) => {
                        report
                    }
                },
            };

            current_proposal.validation = validation.clone();

            if !validation.has_errors() {
                info!(
                    attempts = attempt + 1,
                    "Successfully fixed proposal"
                );
                return Ok(current_proposal);
            }
        }

        // Failed to fix after max attempts
        Err(AlephError::Other {
            message: format!(
                "Failed to fix proposal after {} attempts",
                self.max_fix_attempts
            ),
            suggestion: Some("Review the validation errors and adjust the manifest or capabilities manually".to_string()),
        })
    }

    /// Apply fixes to a proposal based on validation errors
    fn apply_fixes(&self, proposal: &SkillProposal) -> Result<SkillProposal> {
        let mut fixed = proposal.clone();

        for error in &proposal.validation.errors {
            match error {
                super::constraint_validator::ValidationError::NetworkMismatch { .. } => {
                    // Fix: Align capabilities with manifest
                    if fixed.manifest.prohibited_operations.network.prohibit_all {
                        fixed.capabilities.network = NetworkCapability::Deny;
                    }
                }
                super::constraint_validator::ValidationError::FileSystemMismatch {
                    manifest_path,
                    operation,
                    ..
                } => {
                    // Fix: Add missing filesystem capability
                    let path = std::path::PathBuf::from(
                        manifest_path.trim_end_matches("/**").trim_end_matches("/*"),
                    );
                    if operation == "write" {
                        fixed.capabilities.filesystem.push(FileSystemCapability::ReadWrite { path });
                    } else {
                        fixed.capabilities.filesystem.push(FileSystemCapability::ReadOnly { path });
                    }
                }
                super::constraint_validator::ValidationError::UnauthorizedPermission {
                    capability,
                    ..
                } => {
                    // Fix: Remove unauthorized capability or add to manifest
                    // For now, we'll remove the capability
                    if capability.starts_with("ReadWrite:") {
                        let path_str = capability.trim_start_matches("ReadWrite: ");
                        fixed.capabilities.filesystem.retain(|cap| {
                            !matches!(cap, FileSystemCapability::ReadWrite { path } if path.to_string_lossy() == path_str)
                        });
                    }
                }
                super::constraint_validator::ValidationError::ProcessMismatch { .. } => {
                    // Fix: Align process capabilities with manifest
                    fixed.capabilities.process.no_fork =
                        fixed.manifest.prohibited_operations.process.prohibit_fork;
                }
            }
        }

        Ok(fixed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::types::{ExecutionStatus, SkillExecution};

    fn create_test_tracker() -> Arc<EvolutionTracker> {
        Arc::new(EvolutionTracker::in_memory().expect("Failed to create tracker"))
    }

    #[test]
    fn test_collaborative_pipeline_creation() {
        let tracker = create_test_tracker();
        let pipeline = CollaborativeSolidificationPipeline::new(tracker);
        assert_eq!(pipeline.min_confidence, 0.7);
        assert_eq!(pipeline.max_proposals, 10);
        assert_eq!(pipeline.max_fix_attempts, 3);
    }

    #[tokio::test]
    async fn test_collaborative_pipeline_empty() {
        let tracker = create_test_tracker();
        let pipeline = CollaborativeSolidificationPipeline::new(tracker);

        let result = pipeline.run().await.unwrap();
        assert!(result.proposals.is_empty());
        assert_eq!(result.candidates_detected, 0);
    }

    #[tokio::test]
    async fn test_collaborative_pipeline_with_candidates() {
        let tracker = create_test_tracker();

        // Add enough executions to trigger detection
        for i in 0..5 {
            let exec = SkillExecution {
                id: format!("exec-{}", i),
                skill_id: "test-pattern".to_string(),
                session_id: format!("session-{}", i),
                invoked_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
                duration_ms: 1000,
                status: ExecutionStatus::Success,
                satisfaction: Some(0.9),
                context: "test context".to_string(),
                input_summary: "test input".to_string(),
                output_length: 100,
            };
            tracker.log_execution(&exec).unwrap();
        }

        let config = SolidificationConfig {
            min_success_count: 3,
            min_success_rate: 0.7,
            min_age_days: 0,
            max_idle_days: 100,
        };

        let pipeline = CollaborativeSolidificationPipeline::new(tracker)
            .with_config(config)
            .with_min_confidence(0.0); // Very low threshold to ensure proposals pass

        let result = pipeline.run().await.unwrap();

        // The pipeline should detect candidates and generate proposals
        assert_eq!(result.candidates_detected, 1);

        // Note: proposals might be empty if generation fails, but we should have detected candidates
        if !result.proposals.is_empty() {
            // Check that proposals have valid manifests and capabilities
            for proposal in &result.proposals {
                assert!(!proposal.manifest.metadata.skill_id.is_empty());
                assert!(!proposal.capabilities.filesystem.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn test_manifest_generation() {
        let tracker = create_test_tracker();
        let pipeline = CollaborativeSolidificationPipeline::new(tracker);

        let metrics = SkillMetrics {
            skill_id: "test-skill".to_string(),
            total_executions: 10,
            successful_executions: 9,
            avg_duration_ms: 1000.0,
            avg_satisfaction: Some(0.85),
            failure_rate: 0.1,
            last_used: 100,
            first_used: 0,
            context_frequency: std::collections::HashMap::new(),
        };

        let manifest = pipeline.generate_manifest(&metrics).await.unwrap();
        assert_eq!(manifest.metadata.skill_id, "test-skill");
        assert!(manifest.prohibited_operations.network.prohibit_all);
        assert!(manifest.prohibited_operations.process.prohibit_fork);
    }

    #[tokio::test]
    async fn test_capabilities_generation() {
        let tracker = create_test_tracker();
        let pipeline = CollaborativeSolidificationPipeline::new(tracker);

        let metrics = SkillMetrics {
            skill_id: "test-skill".to_string(),
            total_executions: 10,
            successful_executions: 9,
            avg_duration_ms: 1000.0,
            avg_satisfaction: Some(0.85),
            failure_rate: 0.1,
            last_used: 100,
            first_used: 0,
            context_frequency: std::collections::HashMap::new(),
        };

        let manifest = pipeline.generate_manifest(&metrics).await.unwrap();
        let capabilities = pipeline.generate_capabilities(&metrics, &manifest).await.unwrap();

        assert!(matches!(capabilities.network, NetworkCapability::Deny));
        assert!(capabilities.process.no_fork);
        assert!(!capabilities.filesystem.is_empty());
    }
}
