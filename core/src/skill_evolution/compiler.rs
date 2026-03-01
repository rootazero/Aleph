//! Skill Compiler - orchestrates the full skill generation workflow.
//!
//! The compiler brings together all skill evolution components:
//! 1. Pipeline for detection and suggestion generation
//! 2. Approval workflow for user consent
//! 3. Skill generation (SKILL.md creation)
//! 4. Git commit (optional)
//! 5. Registry reload
//!
//! ## Usage
//!
//! ```rust,ignore
//! use alephcore::skill_evolution::{SkillCompiler, EvolutionTracker};
//! use alephcore::skills::SkillsRegistry;
//!
//! let tracker = Arc::new(EvolutionTracker::new("evolution.db")?);
//! let registry = Arc::new(SkillsRegistry::with_auto_discover(None)?);
//!
//! let compiler = SkillCompiler::new(tracker, registry);
//!
//! // Run detection and submit for approval
//! let pending = compiler.detect_and_submit().await?;
//!
//! // List pending approvals
//! for req in compiler.list_pending()? {
//!     println!("Pending: {}", req.suggestion.suggested_name);
//! }
//!
//! // Approve and compile a skill
//! let result = compiler.approve_and_compile("request-id").await?;
//! ```

use crate::sync_primitives::Arc;

use tracing::{debug, info, warn};

use crate::config::EvolutionConfig;
use crate::error::{AlephError, Result};
use crate::providers::AiProvider;
use crate::skills::SkillsRegistry;

use super::approval::{ApprovalManager, ApprovalRequest, ApprovalStatus};
use super::generator::SkillGenerator;
use super::git::GitCommitter;
use super::pipeline::{PipelineResult, PipelineStatus, SolidificationPipeline};
use super::tracker::EvolutionTracker;
use super::types::{CommitResult, GenerationResult, SolidificationSuggestion};

/// Result of compiling a skill
#[derive(Debug, Clone)]
pub struct CompilationResult {
    /// The generated skill ID
    pub skill_id: String,
    /// Path to the generated SKILL.md
    pub file_path: String,
    /// Whether the skill was committed to git
    pub committed: bool,
    /// Commit hash if committed
    pub commit_hash: Option<String>,
    /// Whether the registry was reloaded
    pub registry_reloaded: bool,
}

/// Status of the skill compiler
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompilerStatus {
    /// Whether the compiler is enabled
    pub enabled: bool,
    /// Pipeline status (patterns, candidates, etc.)
    pub pipeline: PipelineStatus,
    /// Number of pending approvals
    pub pending_approvals: usize,
    /// Total skills compiled
    pub total_compiled: usize,
    /// Last compilation time (unix timestamp)
    pub last_compilation: Option<i64>,
    /// Last error if any
    pub last_error: Option<String>,
}

/// Skill Compiler - orchestrates the full skill evolution workflow.
///
/// Combines detection, approval, generation, and registry reload into
/// a cohesive workflow for converting execution patterns into skills.
pub struct SkillCompiler {
    /// Pipeline for detection and suggestion
    pipeline: SolidificationPipeline,
    /// Approval manager for user consent
    approvals: ApprovalManager,
    /// Skill generator for creating SKILL.md files
    generator: SkillGenerator,
    /// Git committer for version control
    git: Option<GitCommitter>,
    /// Skills registry for hot reload
    registry: Arc<SkillsRegistry>,
    /// Configuration
    config: EvolutionConfig,
    /// Compilation counter
    total_compiled: std::sync::atomic::AtomicUsize,
    /// Last compilation timestamp
    last_compilation: std::sync::atomic::AtomicI64,
}

impl SkillCompiler {
    /// Create a new skill compiler with default configuration.
    pub fn new(tracker: Arc<EvolutionTracker>, registry: Arc<SkillsRegistry>) -> Self {
        let config = EvolutionConfig::default();
        let skills_dir = registry.skills_dir().clone();

        Self {
            pipeline: SolidificationPipeline::new(tracker),
            approvals: ApprovalManager::new(),
            generator: SkillGenerator::new(&skills_dir),
            git: None,
            registry,
            config,
            total_compiled: std::sync::atomic::AtomicUsize::new(0),
            last_compilation: std::sync::atomic::AtomicI64::new(0),
        }
    }

    /// Create a compiler from configuration.
    pub fn from_config(
        tracker: Arc<EvolutionTracker>,
        registry: Arc<SkillsRegistry>,
        config: &EvolutionConfig,
    ) -> Self {
        let skills_dir = config
            .get_skills_output_dir(&crate::config::SkillsConfig::default())
            .to_path_buf();

        let mut compiler = Self {
            pipeline: SolidificationPipeline::from_config(tracker, config),
            approvals: ApprovalManager::new(),
            generator: SkillGenerator::new(&skills_dir),
            git: None,
            registry,
            config: config.clone(),
            total_compiled: std::sync::atomic::AtomicUsize::new(0),
            last_compilation: std::sync::atomic::AtomicI64::new(0),
        };

        // Configure git if auto-commit is enabled
        if config.auto_commit {
            let mut git = GitCommitter::new(skills_dir.to_string_lossy().to_string());
            if config.auto_push {
                git = git.with_auto_push(true);
                if !config.remote.is_empty() {
                    git = git.with_remote(&config.remote);
                }
                if !config.branch.is_empty() {
                    git = git.with_branch(&config.branch);
                }
            }
            compiler.git = Some(git);
        }

        compiler
    }

    /// Set the AI provider for generating better suggestions.
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.pipeline = self.pipeline.with_provider(provider);
        self
    }

    /// Enable git auto-commit.
    pub fn with_auto_commit(mut self) -> Self {
        let skills_dir = self.generator.skills_dir();
        self.git = Some(GitCommitter::new(skills_dir.to_string_lossy().to_string()));
        self
    }

    /// Run the detection pipeline and submit suggestions for approval.
    ///
    /// Returns the number of new pending approvals created.
    pub async fn detect_and_submit(&self) -> Result<usize> {
        if !self.config.enabled {
            return Ok(0);
        }

        let result = self.pipeline.run().await?;

        if result.suggestions.is_empty() {
            debug!("No suggestions to submit");
            return Ok(0);
        }

        let ids = self.approvals.submit_batch(result.suggestions)?;

        info!(
            count = ids.len(),
            "Submitted suggestions for approval"
        );

        Ok(ids.len())
    }

    /// List all pending approval requests.
    pub fn list_pending(&self) -> Result<Vec<ApprovalRequest>> {
        self.approvals.list_pending()
    }

    /// Get a specific approval request.
    pub fn get_pending(&self, request_id: &str) -> Result<Option<ApprovalRequest>> {
        self.approvals.get(request_id)
    }

    /// Preview what a skill would look like without creating it.
    pub fn preview_skill(&self, request_id: &str) -> Result<String> {
        let request = self.approvals.get(request_id)?.ok_or_else(|| {
            AlephError::Other {
                message: format!("Request not found: {}", request_id),
                suggestion: None,
            }
        })?;

        if request.status != ApprovalStatus::Pending {
            return Err(AlephError::Other {
                message: format!("Request is not pending: {:?}", request.status),
                suggestion: None,
            });
        }

        Ok(self.generator.preview(&request.suggestion))
    }

    /// Approve a pending request and compile the skill.
    ///
    /// This is the main compilation workflow:
    /// 1. Approve the request
    /// 2. Generate SKILL.md
    /// 3. Commit to git (if configured)
    /// 4. Reload the skills registry
    pub async fn approve_and_compile(&self, request_id: &str) -> Result<CompilationResult> {
        // Step 1: Approve
        let suggestion = self.approvals.approve(request_id)?;

        // Step 2: Generate
        let generation = self.generator.generate(&suggestion)?;

        let (skill_id, file_path) = match generation {
            GenerationResult::Generated {
                skill_id,
                file_path,
                ..
            } => (skill_id, file_path),
            GenerationResult::AlreadyExists { skill_id } => {
                return Err(AlephError::Other {
                    message: format!("Skill '{}' already exists", skill_id),
                    suggestion: Some("Use a different name or delete the existing skill".to_string()),
                });
            }
            GenerationResult::Failed { reason } => {
                return Err(AlephError::Other {
                    message: format!("Failed to generate skill: {}", reason),
                    suggestion: None,
                });
            }
        };

        // Step 3: Git commit (if configured)
        let (committed, commit_hash) = if let Some(ref git) = self.git {
            match git.commit_skill(&file_path, &skill_id) {
                Ok(CommitResult::Committed { commit_hash, .. }) => (true, Some(commit_hash)),
                Ok(CommitResult::NothingToCommit) => (false, None),
                Ok(CommitResult::Failed { reason }) => {
                    warn!(reason = %reason, "Git commit failed, continuing");
                    (false, None)
                }
                Err(e) => {
                    warn!(error = %e, "Git commit error, continuing");
                    (false, None)
                }
            }
        } else {
            (false, None)
        };

        // Step 4: Reload registry
        let registry_reloaded = match self.registry.reload() {
            Ok(()) => {
                info!(skill_id = %skill_id, "Registry reloaded with new skill");
                true
            }
            Err(e) => {
                warn!(error = %e, "Failed to reload registry");
                false
            }
        };

        // Update stats
        self.total_compiled
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        self.last_compilation
            .store(now, std::sync::atomic::Ordering::Relaxed);

        info!(
            skill_id = %skill_id,
            file_path = %file_path,
            committed = committed,
            "Skill compiled successfully"
        );

        Ok(CompilationResult {
            skill_id,
            file_path,
            committed,
            commit_hash,
            registry_reloaded,
        })
    }

    /// Reject a pending approval request.
    pub fn reject(&self, request_id: &str, reason: Option<String>) -> Result<()> {
        self.approvals.reject_with_reason(request_id, reason)
    }

    /// Compile a suggestion directly (without approval workflow).
    ///
    /// Use this for testing or when user has already approved via other means.
    pub async fn compile_direct(
        &self,
        suggestion: &SolidificationSuggestion,
    ) -> Result<CompilationResult> {
        // Generate
        let generation = self.generator.generate(suggestion)?;

        let (skill_id, file_path) = match generation {
            GenerationResult::Generated {
                skill_id,
                file_path,
                ..
            } => (skill_id, file_path),
            GenerationResult::AlreadyExists { skill_id } => {
                return Err(AlephError::Other {
                    message: format!("Skill '{}' already exists", skill_id),
                    suggestion: None,
                });
            }
            GenerationResult::Failed { reason } => {
                return Err(AlephError::Other {
                    message: format!("Failed to generate skill: {}", reason),
                    suggestion: None,
                });
            }
        };

        // Git commit
        let (committed, commit_hash) = if let Some(ref git) = self.git {
            match git.commit_skill(&file_path, &skill_id) {
                Ok(CommitResult::Committed { commit_hash, .. }) => (true, Some(commit_hash)),
                _ => (false, None),
            }
        } else {
            (false, None)
        };

        // Reload registry
        let registry_reloaded = self.registry.reload().is_ok();

        // Update stats
        self.total_compiled
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        self.last_compilation
            .store(now, std::sync::atomic::Ordering::Relaxed);

        Ok(CompilationResult {
            skill_id,
            file_path,
            committed,
            commit_hash,
            registry_reloaded,
        })
    }

    /// Get the current compiler status.
    pub fn status(&self) -> Result<CompilerStatus> {
        let pipeline_status = self.pipeline.status()?;
        let pending = self.approvals.pending_count()?;
        let total = self
            .total_compiled
            .load(std::sync::atomic::Ordering::Relaxed);
        let last = self
            .last_compilation
            .load(std::sync::atomic::Ordering::Relaxed);

        Ok(CompilerStatus {
            enabled: self.config.enabled,
            pipeline: pipeline_status,
            pending_approvals: pending,
            total_compiled: total,
            last_compilation: if last > 0 { Some(last) } else { None },
            last_error: None,
        })
    }

    /// Check if there are any pending candidates or approvals.
    pub fn has_pending(&self) -> Result<bool> {
        let has_candidates = self.pipeline.has_candidates()?;
        let has_approvals = self.approvals.pending_count()? > 0;
        Ok(has_candidates || has_approvals)
    }

    /// Run a full detection cycle (for scheduled/cron execution).
    ///
    /// Returns the pipeline result with any new suggestions.
    pub async fn run_detection(&self) -> Result<PipelineResult> {
        self.pipeline.run().await
    }

    /// Get reference to the approval manager.
    pub fn approvals(&self) -> &ApprovalManager {
        &self.approvals
    }

    /// Get reference to the generator.
    pub fn generator(&self) -> &SkillGenerator {
        &self.generator
    }

    /// Get reference to the pipeline.
    pub fn pipeline(&self) -> &SolidificationPipeline {
        &self.pipeline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::types::SkillMetrics;
    use tempfile::TempDir;

    fn create_test_compiler() -> (SkillCompiler, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let tracker = Arc::new(EvolutionTracker::in_memory().unwrap());
        let registry = Arc::new(SkillsRegistry::new(skills_dir));

        let compiler = SkillCompiler::new(tracker, registry);
        (compiler, temp_dir)
    }

    fn create_test_suggestion() -> SolidificationSuggestion {
        SolidificationSuggestion {
            pattern_id: "test-pattern".to_string(),
            suggested_name: "test-skill".to_string(),
            suggested_description: "A test skill".to_string(),
            confidence: 0.9,
            metrics: SkillMetrics::new("test-pattern"),
            sample_contexts: vec!["test context".to_string()],
            instructions_preview: "# Instructions\n\nTest instructions.".to_string(),
        }
    }

    #[test]
    fn test_compiler_creation() {
        let (compiler, _temp) = create_test_compiler();
        let status = compiler.status().unwrap();
        assert!(status.enabled);
        assert_eq!(status.pending_approvals, 0);
    }

    #[tokio::test]
    async fn test_compile_direct() {
        let (compiler, _temp) = create_test_compiler();
        let suggestion = create_test_suggestion();

        let result = compiler.compile_direct(&suggestion).await.unwrap();

        assert_eq!(result.skill_id, "test-skill");
        assert!(!result.committed); // No git configured
        assert!(std::path::Path::new(&result.file_path).exists());
    }

    #[tokio::test]
    async fn test_approval_workflow() {
        let (compiler, _temp) = create_test_compiler();
        let suggestion = create_test_suggestion();

        // Submit for approval
        let id = compiler.approvals.submit(suggestion).unwrap();

        // List pending
        let pending = compiler.list_pending().unwrap();
        assert_eq!(pending.len(), 1);

        // Preview
        let preview = compiler.preview_skill(&id).unwrap();
        assert!(preview.contains("test-skill"));

        // Approve and compile
        let result = compiler.approve_and_compile(&id).await.unwrap();
        assert_eq!(result.skill_id, "test-skill");

        // No more pending
        let pending = compiler.list_pending().unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_reject() {
        let (compiler, _temp) = create_test_compiler();
        let suggestion = create_test_suggestion();

        let id = compiler.approvals.submit(suggestion).unwrap();
        compiler.reject(&id, Some("Not useful".to_string())).unwrap();

        let request = compiler.get_pending(&id).unwrap().unwrap();
        assert_eq!(request.status, ApprovalStatus::Rejected);
    }

    #[tokio::test]
    async fn test_status_tracking() {
        let (compiler, _temp) = create_test_compiler();

        let status = compiler.status().unwrap();
        assert_eq!(status.total_compiled, 0);
        assert!(status.last_compilation.is_none());

        // Compile a skill
        let suggestion = create_test_suggestion();
        compiler.compile_direct(&suggestion).await.unwrap();

        let status = compiler.status().unwrap();
        assert_eq!(status.total_compiled, 1);
        assert!(status.last_compilation.is_some());
    }
}
