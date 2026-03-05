//! Core types for POE (Principle-Operation-Evaluation) architecture.
//!
//! This module defines all data structures used in goal-oriented agent execution:
//! - SuccessManifest: Defines success criteria before execution
//! - ValidationRule: Individual validation conditions (hard and soft)
//! - Verdict: Evaluation results from validation
//! - WorkerOutput: Results from worker execution
//! - Experience: For future crystallization of learned patterns

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================================
// Default Functions
// ============================================================================

/// Default timeout for command execution (30 seconds)
fn default_timeout_ms() -> u64 {
    30_000
}

/// Default maximum attempts for a task
fn default_max_attempts() -> u8 {
    5
}

/// Default weight for soft metrics
fn default_weight() -> f32 {
    1.0
}

/// Default threshold for soft metrics
fn default_threshold() -> f32 {
    0.8
}

// ============================================================================
// Success Manifest (成功契约)
// ============================================================================

/// Defines success criteria for a task before execution begins.
///
/// The manifest acts as a contract between the orchestrator and worker,
/// specifying what constitutes successful completion of a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessManifest {
    /// Unique identifier for this task
    pub task_id: String,

    /// Human-readable description of the goal
    pub objective: String,

    /// Rules that MUST pass for success (all must pass)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hard_constraints: Vec<ValidationRule>,

    /// Rules that contribute to quality score (weighted average)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub soft_metrics: Vec<SoftMetric>,

    /// Maximum retry attempts before giving up
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u8,

    /// Optional snapshot path for rollback on failure
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_snapshot: Option<PathBuf>,
}

impl SuccessManifest {
    /// Create a new SuccessManifest with the given task_id and objective.
    pub fn new(task_id: impl Into<String>, objective: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            objective: objective.into(),
            hard_constraints: Vec::new(),
            soft_metrics: Vec::new(),
            max_attempts: default_max_attempts(),
            rollback_snapshot: None,
        }
    }

    /// Add a hard constraint that must pass for success.
    pub fn with_hard_constraint(mut self, rule: ValidationRule) -> Self {
        self.hard_constraints.push(rule);
        self
    }

    /// Add a soft metric that contributes to quality score.
    pub fn with_soft_metric(mut self, metric: SoftMetric) -> Self {
        self.soft_metrics.push(metric);
        self
    }

    /// Set the maximum number of retry attempts.
    pub fn with_max_attempts(mut self, attempts: u8) -> Self {
        self.max_attempts = attempts;
        self
    }

    /// Set the rollback snapshot path.
    pub fn with_rollback_snapshot(mut self, path: PathBuf) -> Self {
        self.rollback_snapshot = Some(path);
        self
    }
}

// ============================================================================
// Soft Metric
// ============================================================================

/// A weighted validation rule that contributes to quality score.
///
/// Unlike hard constraints, soft metrics don't cause immediate failure.
/// Instead, they contribute to an overall quality score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftMetric {
    /// The validation rule to evaluate
    pub rule: ValidationRule,

    /// Weight of this metric (0.0 - 1.0)
    #[serde(default = "default_weight")]
    pub weight: f32,

    /// Minimum acceptable score threshold (e.g., 0.8)
    #[serde(default = "default_threshold")]
    pub threshold: f32,
}

impl SoftMetric {
    /// Create a new SoftMetric with default weight and threshold.
    pub fn new(rule: ValidationRule) -> Self {
        Self {
            rule,
            weight: default_weight(),
            threshold: default_threshold(),
        }
    }

    /// Set the weight for this metric.
    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight.clamp(0.0, 1.0);
        self
    }

    /// Set the threshold for this metric.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }
}

// ============================================================================
// Validation Rule
// ============================================================================

/// A rule that can be evaluated to determine success or failure.
///
/// Rules are categorized into:
/// - File system checks (existence, content)
/// - Command execution checks (exit code, output)
/// - Data validation (JSON schema)
/// - Semantic checks (LLM-based evaluation)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum ValidationRule {
    // ========== File System Rules ==========
    /// Check that a file exists at the given path
    FileExists { path: PathBuf },

    /// Check that a file does NOT exist at the given path
    FileNotExists { path: PathBuf },

    /// Check that a file contains a specific pattern (regex)
    FileContains { path: PathBuf, pattern: String },

    /// Check that a file does NOT contain a specific pattern (regex)
    FileNotContains { path: PathBuf, pattern: String },

    /// Check that directory structure matches expected layout
    /// `expected` is a simple pattern like "src/, tests/, Cargo.toml"
    DirStructureMatch { root: PathBuf, expected: String },

    // ========== Command Execution Rules ==========
    /// Check that a command exits successfully (exit code 0)
    CommandPasses {
        cmd: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },

    /// Check that a command's output contains a specific pattern
    CommandOutputContains {
        cmd: String,
        #[serde(default)]
        args: Vec<String>,
        pattern: String,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },

    // ========== Data Validation Rules ==========
    /// Validate a JSON file against a JSON Schema
    JsonSchemaValid {
        path: PathBuf,
        /// JSON Schema as a string (will be parsed during validation)
        schema: String,
    },

    // ========== Semantic (LLM Judge) Rules ==========
    /// Use an LLM to evaluate semantic correctness
    SemanticCheck {
        /// What to evaluate
        target: JudgeTarget,
        /// Prompt for the LLM judge
        prompt: String,
        /// Criteria that defines what "passing" means
        passing_criteria: String,
        /// Which model tier to use for evaluation
        #[serde(default)]
        model_tier: ModelTier,
    },
}

// ============================================================================
// Judge Target
// ============================================================================

/// Target for semantic (LLM-based) evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum JudgeTarget {
    /// Evaluate the contents of a file
    File(PathBuf),

    /// Evaluate a string of content directly
    Content(String),

    /// Evaluate the output of a command
    CommandOutput {
        cmd: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

// ============================================================================
// Model Tier
// ============================================================================

/// Model tier for LLM-based evaluation.
///
/// Different tiers trade off cost/speed vs capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ModelTier {
    /// Local fast model (e.g., Llama 3 8B)
    LocalFast,

    /// Cloud fast model (e.g., GPT-4o-mini, Claude Haiku)
    #[default]
    CloudFast,

    /// Cloud smart model (e.g., GPT-4o, Claude Sonnet)
    CloudSmart,

    /// Cloud deep reasoning model (e.g., o1, Claude Opus)
    CloudDeep,
}

impl ModelTier {
    /// Returns a human-readable name for this tier.
    pub fn name(&self) -> &'static str {
        match self {
            ModelTier::LocalFast => "local-fast",
            ModelTier::CloudFast => "cloud-fast",
            ModelTier::CloudSmart => "cloud-smart",
            ModelTier::CloudDeep => "cloud-deep",
        }
    }
}

// ============================================================================
// Verdict
// ============================================================================

/// Result of evaluating a SuccessManifest.
///
/// Contains both hard constraint results and soft metric scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    /// Overall pass/fail status (all hard constraints must pass)
    pub passed: bool,

    /// Distance from perfect success (0.0 = perfect, 1.0 = complete failure)
    pub distance_score: f32,

    /// Human-readable explanation of the verdict
    pub reason: String,

    /// Optional suggestion for improvement
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,

    /// Results for each hard constraint
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hard_results: Vec<RuleResult>,

    /// Results for each soft metric
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub soft_results: Vec<SoftRuleResult>,
}

impl Verdict {
    /// Create a successful verdict.
    pub fn success(reason: impl Into<String>) -> Self {
        Self {
            passed: true,
            distance_score: 0.0,
            reason: reason.into(),
            suggestion: None,
            hard_results: Vec::new(),
            soft_results: Vec::new(),
        }
    }

    /// Create a failed verdict.
    pub fn failure(reason: impl Into<String>) -> Self {
        Self {
            passed: false,
            distance_score: 1.0,
            reason: reason.into(),
            suggestion: None,
            hard_results: Vec::new(),
            soft_results: Vec::new(),
        }
    }

    /// Add a suggestion for improvement.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Set hard constraint results.
    pub fn with_hard_results(mut self, results: Vec<RuleResult>) -> Self {
        self.hard_results = results;
        self
    }

    /// Set soft metric results.
    pub fn with_soft_results(mut self, results: Vec<SoftRuleResult>) -> Self {
        self.soft_results = results;
        self
    }

    /// Set the distance score.
    pub fn with_distance_score(mut self, score: f32) -> Self {
        self.distance_score = score.clamp(0.0, 1.0);
        self
    }
}

// ============================================================================
// Rule Results
// ============================================================================

/// Result of evaluating a single validation rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleResult {
    /// The rule that was evaluated
    pub rule: ValidationRule,

    /// Whether the rule passed
    pub passed: bool,

    /// Error message if the rule failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RuleResult {
    /// Create a passing result.
    pub fn pass(rule: ValidationRule) -> Self {
        Self {
            rule,
            passed: true,
            error: None,
        }
    }

    /// Create a failing result.
    pub fn fail(rule: ValidationRule, error: impl Into<String>) -> Self {
        Self {
            rule,
            passed: false,
            error: Some(error.into()),
        }
    }
}

/// Result of evaluating a soft metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftRuleResult {
    /// The metric that was evaluated
    pub metric: SoftMetric,

    /// Score achieved (0.0 - 1.0)
    pub score: f32,

    /// Optional feedback from evaluation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

impl SoftRuleResult {
    /// Create a new soft rule result.
    pub fn new(metric: SoftMetric, score: f32) -> Self {
        Self {
            metric,
            score: score.clamp(0.0, 1.0),
            feedback: None,
        }
    }

    /// Add feedback to the result.
    pub fn with_feedback(mut self, feedback: impl Into<String>) -> Self {
        self.feedback = Some(feedback.into());
        self
    }
}

// ============================================================================
// Worker Output
// ============================================================================

/// Output from a worker's execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerOutput {
    /// Total tokens consumed during execution
    pub tokens_consumed: u32,

    /// Number of steps taken
    pub steps_taken: u32,

    /// Final state of the worker
    pub final_state: WorkerState,

    /// Artifacts produced during execution
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,

    /// Log of all steps taken
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub execution_log: Vec<StepLog>,
}

impl WorkerOutput {
    /// Create a new WorkerOutput with the given final state.
    pub fn new(final_state: WorkerState) -> Self {
        Self {
            tokens_consumed: 0,
            steps_taken: 0,
            final_state,
            artifacts: Vec::new(),
            execution_log: Vec::new(),
        }
    }

    /// Create a completed output with a summary.
    pub fn completed(summary: impl Into<String>) -> Self {
        Self::new(WorkerState::Completed {
            summary: summary.into(),
        })
    }

    /// Create a failed output with a reason.
    pub fn failed(reason: impl Into<String>) -> Self {
        Self::new(WorkerState::Failed {
            reason: reason.into(),
        })
    }
}

/// Final state of a worker after execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum WorkerState {
    /// Worker completed successfully
    Completed {
        /// Summary of what was accomplished
        summary: String,
    },

    /// Worker failed
    Failed {
        /// Reason for failure
        reason: String,
    },

    /// Worker needs input from user
    NeedsInput {
        /// Question for the user
        question: String,
    },
}

// ============================================================================
// Artifacts
// ============================================================================

/// An artifact produced during worker execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Path to the artifact
    pub path: PathBuf,

    /// Type of change made
    pub change_type: ChangeType,

    /// SHA-256 hash of the content
    pub content_hash: String,
}

impl Artifact {
    /// Create a new artifact.
    pub fn new(path: PathBuf, change_type: ChangeType, content_hash: impl Into<String>) -> Self {
        Self {
            path,
            change_type,
            content_hash: content_hash.into(),
        }
    }
}

/// Type of change made to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    /// File was created
    Created,
    /// File was modified
    Modified,
    /// File was deleted
    Deleted,
}

// ============================================================================
// Step Log
// ============================================================================

/// Log entry for a single step in worker execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepLog {
    /// Unique identifier for this step
    pub step_id: u32,

    /// Description of the action taken
    pub action: String,

    /// Result of the action
    pub result: String,

    /// Duration of the step in milliseconds
    pub duration_ms: u64,
}

impl StepLog {
    /// Create a new step log entry.
    pub fn new(
        step_id: u32,
        action: impl Into<String>,
        result: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            step_id,
            action: action.into(),
            result: result.into(),
            duration_ms,
        }
    }
}

// ============================================================================
// POE Task and Outcome
// ============================================================================

/// A complete POE task with manifest and instruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoeTask {
    /// Success criteria for the task
    pub manifest: SuccessManifest,

    /// Natural language instruction for the worker
    pub instruction: String,
}

impl PoeTask {
    /// Create a new POE task.
    pub fn new(manifest: SuccessManifest, instruction: impl Into<String>) -> Self {
        Self {
            manifest,
            instruction: instruction.into(),
        }
    }
}

/// Outcome of a POE task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome")]
pub enum PoeOutcome {
    /// Task completed successfully (verdict + worker's output summary)
    Success {
        verdict: Verdict,
        #[serde(default)]
        worker_summary: String,
    },

    /// Strategy switch needed (task too complex or wrong approach)
    StrategySwitch {
        /// Reason for switching strategy
        reason: String,
        /// Suggested alternative approach
        suggestion: String,
    },

    /// Budget exhausted without success
    BudgetExhausted {
        /// Number of attempts made
        attempts: u8,
        /// Error from the last attempt
        last_error: String,
    },
}

impl PoeOutcome {
    /// Create a successful outcome with worker summary.
    pub fn success(verdict: Verdict, worker_summary: impl Into<String>) -> Self {
        PoeOutcome::Success { verdict, worker_summary: worker_summary.into() }
    }

    /// Create a strategy switch outcome.
    pub fn strategy_switch(reason: impl Into<String>, suggestion: impl Into<String>) -> Self {
        PoeOutcome::StrategySwitch {
            reason: reason.into(),
            suggestion: suggestion.into(),
        }
    }

    /// Create a budget exhausted outcome.
    pub fn budget_exhausted(attempts: u8, last_error: impl Into<String>) -> Self {
        PoeOutcome::BudgetExhausted {
            attempts,
            last_error: last_error.into(),
        }
    }

    /// Check if the outcome is a success.
    pub fn is_success(&self) -> bool {
        matches!(self, PoeOutcome::Success { verdict: v, .. } if v.passed)
    }
}

// ============================================================================
// Experience Types (for future crystallization)
// ============================================================================

/// A crystallized experience from past task execution.
///
/// Experiences capture successful patterns that can be reused for similar tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    /// Unique identifier for this experience
    pub id: String,

    /// Pattern that matches tasks this experience applies to
    pub pattern: TaskPattern,

    /// The solution path that worked
    pub solution: SolutionPath,

    /// Outcome of applying this solution
    pub outcome: ExperienceOutcome,

    /// Number of times this experience has been successfully reused
    #[serde(default)]
    pub reuse_count: u32,

    /// When this experience was created
    pub created_at: DateTime<Utc>,

    /// When this experience was last used
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
}

impl Experience {
    /// Create a new experience.
    pub fn new(
        id: impl Into<String>,
        pattern: TaskPattern,
        solution: SolutionPath,
        outcome: ExperienceOutcome,
    ) -> Self {
        Self {
            id: id.into(),
            pattern,
            solution,
            outcome,
            reuse_count: 0,
            created_at: Utc::now(),
            last_used_at: None,
        }
    }
}

/// Pattern that describes what kind of tasks an experience applies to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPattern {
    /// Keywords that describe the task type
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,

    /// Domain of the task (e.g., "rust", "python", "devops")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,

    /// Semantic embedding of the task description (for similarity search)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,

    /// File patterns involved (e.g., "*.rs", "Cargo.toml")
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_patterns: Vec<String>,
}

impl TaskPattern {
    /// Create a new task pattern.
    pub fn new() -> Self {
        Self {
            keywords: Vec::new(),
            domain: None,
            embedding: None,
            file_patterns: Vec::new(),
        }
    }

    /// Add keywords to the pattern.
    pub fn with_keywords(mut self, keywords: Vec<String>) -> Self {
        self.keywords = keywords;
        self
    }

    /// Set the domain.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Set the embedding.
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }
}

impl Default for TaskPattern {
    fn default() -> Self {
        Self::new()
    }
}

/// The solution path that successfully completed a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionPath {
    /// High-level strategy used
    pub strategy: String,

    /// Sequence of tool calls that worked
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_sequence: Vec<String>,

    /// Key decisions made during execution
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_decisions: Vec<String>,

    /// Files that were created or modified
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected_files: Vec<PathBuf>,

    /// Total tokens used
    pub tokens_used: u32,

    /// Number of attempts before success
    pub attempts: u8,
}

impl SolutionPath {
    /// Create a new solution path.
    pub fn new(strategy: impl Into<String>) -> Self {
        Self {
            strategy: strategy.into(),
            tool_sequence: Vec::new(),
            key_decisions: Vec::new(),
            affected_files: Vec::new(),
            tokens_used: 0,
            attempts: 1,
        }
    }
}

/// Outcome metrics for an experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceOutcome {
    /// Whether the task succeeded
    pub success: bool,

    /// Final distance score (0.0 = perfect)
    pub distance_score: f32,

    /// Average score across soft metrics
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_metric_avg: Option<f32>,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,
}

impl ExperienceOutcome {
    /// Create a successful outcome.
    pub fn success(distance_score: f32, execution_time_ms: u64) -> Self {
        Self {
            success: true,
            distance_score,
            soft_metric_avg: None,
            execution_time_ms,
        }
    }

    /// Create a failed outcome.
    pub fn failure(execution_time_ms: u64) -> Self {
        Self {
            success: false,
            distance_score: 1.0,
            soft_metric_avg: None,
            execution_time_ms,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_success_manifest_builder() {
        let manifest = SuccessManifest::new("task-1", "Create a new file")
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("test.txt"),
            })
            .with_soft_metric(SoftMetric::new(ValidationRule::FileContains {
                path: PathBuf::from("test.txt"),
                pattern: "hello".to_string(),
            }))
            .with_max_attempts(3);

        assert_eq!(manifest.task_id, "task-1");
        assert_eq!(manifest.hard_constraints.len(), 1);
        assert_eq!(manifest.soft_metrics.len(), 1);
        assert_eq!(manifest.max_attempts, 3);
    }

    #[test]
    fn test_verdict_constructors() {
        let success = Verdict::success("Task completed")
            .with_distance_score(0.1)
            .with_suggestion("Consider adding tests");

        assert!(success.passed);
        assert_eq!(success.distance_score, 0.1);
        assert!(success.suggestion.is_some());

        let failure = Verdict::failure("File not found");
        assert!(!failure.passed);
        assert_eq!(failure.distance_score, 1.0);
    }

    #[test]
    fn test_validation_rule_serialization() {
        let rule = ValidationRule::FileExists {
            path: PathBuf::from("/tmp/test.txt"),
        };

        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("FileExists"));

        let parsed: ValidationRule = serde_json::from_str(&json).unwrap();
        match parsed {
            ValidationRule::FileExists { path } => {
                assert_eq!(path, PathBuf::from("/tmp/test.txt"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_worker_output() {
        let output = WorkerOutput::completed("Task done");
        assert_eq!(output.tokens_consumed, 0);

        match output.final_state {
            WorkerState::Completed { summary } => {
                assert_eq!(summary, "Task done");
            }
            _ => panic!("Wrong state"),
        }
    }

    #[test]
    fn test_poe_outcome() {
        let verdict = Verdict::success("All checks passed");
        let outcome = PoeOutcome::success(verdict);
        assert!(outcome.is_success());

        let switch = PoeOutcome::strategy_switch("Too complex", "Try smaller steps");
        assert!(!switch.is_success());

        let exhausted = PoeOutcome::budget_exhausted(5, "Max retries exceeded");
        assert!(!exhausted.is_success());
    }

    #[test]
    fn test_model_tier_default() {
        let tier = ModelTier::default();
        assert_eq!(tier, ModelTier::CloudFast);
        assert_eq!(tier.name(), "cloud-fast");
    }

    #[test]
    fn test_soft_metric_clamp() {
        let metric = SoftMetric::new(ValidationRule::FileExists {
            path: PathBuf::from("test.txt"),
        })
        .with_weight(1.5) // Should clamp to 1.0
        .with_threshold(-0.5); // Should clamp to 0.0

        assert_eq!(metric.weight, 1.0);
        assert_eq!(metric.threshold, 0.0);
    }

    #[test]
    fn test_experience_creation() {
        let pattern = TaskPattern::new()
            .with_keywords(vec!["rust".to_string(), "file".to_string()])
            .with_domain("rust");

        let solution = SolutionPath::new("direct-write");
        let outcome = ExperienceOutcome::success(0.0, 1000);

        let exp = Experience::new("exp-1", pattern, solution, outcome);

        assert_eq!(exp.id, "exp-1");
        assert_eq!(exp.reuse_count, 0);
        assert!(exp.last_used_at.is_none());
    }
}
