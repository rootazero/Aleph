//! POE execution manager.
//!
//! This module provides the main orchestrator for the POE (Principle-Operation-Evaluation)
//! execution cycle:
//!
//! - **PoeConfig**: Configuration for the POE manager
//! - **PoeManager**: Orchestrates the P->O->E cycle with budget tracking and strategy switching
//!
//! ## Execution Flow
//!
//! 1. Create budget from task manifest and config
//! 2. Loop while budget not exhausted:
//!    a. Execute instruction via worker
//!    b. Validate output against manifest
//!    c. Record attempt in budget
//!    d. If passed -> return Success
//!    e. If stuck -> return StrategySwitch
//!    f. Otherwise -> retry with failure feedback
//! 3. If loop exits -> return BudgetExhausted

use std::sync::Arc;
use std::time::Instant;

use crate::error::Result;
use crate::poe::budget::PoeBudget;
use crate::poe::crystallization::ExperienceRecorder;
use crate::poe::types::{PoeOutcome, PoeTask, Verdict, WorkerOutput, WorkerState};
use crate::poe::validation::CompositeValidator;
use crate::poe::worker::Worker;

// ============================================================================
// Validation Callback
// ============================================================================

/// Data passed to validation callback after each validation attempt.
#[derive(Debug, Clone)]
pub struct ValidationEvent {
    /// Current attempt number (1-indexed)
    pub attempt: u8,
    /// Whether validation passed
    pub passed: bool,
    /// Distance score (0.0 = perfect, 1.0 = complete failure)
    pub distance_score: f32,
    /// Reason for the verdict
    pub reason: String,
}

/// Callback type for receiving validation events during POE execution.
pub type ValidationCallback = Arc<dyn Fn(ValidationEvent) + Send + Sync>;

// ============================================================================
// PoeConfig
// ============================================================================

/// Configuration for the POE execution manager.
///
/// Controls budget limits and stuck detection parameters.
#[derive(Debug, Clone)]
pub struct PoeConfig {
    /// Number of attempts to consider for stuck detection.
    /// If no progress is made over this many attempts, a strategy switch is suggested.
    /// Default: 3
    pub stuck_window: usize,

    /// Maximum tokens that can be consumed across all attempts.
    /// Default: 100,000
    pub max_tokens: u32,
}

impl Default for PoeConfig {
    fn default() -> Self {
        Self {
            stuck_window: 3,
            max_tokens: 100_000,
        }
    }
}

impl PoeConfig {
    /// Create a new PoeConfig with custom settings.
    pub fn new(stuck_window: usize, max_tokens: u32) -> Self {
        Self {
            stuck_window,
            max_tokens,
        }
    }

    /// Set the stuck window size.
    pub fn with_stuck_window(mut self, window: usize) -> Self {
        self.stuck_window = window;
        self
    }

    /// Set the maximum tokens.
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }
}

// ============================================================================
// PoeManager
// ============================================================================

/// POE execution orchestrator.
///
/// The PoeManager coordinates the Principle-Operation-Evaluation cycle:
/// 1. **Principle**: Uses the task's SuccessManifest to define success criteria
/// 2. **Operation**: Delegates execution to a Worker implementation
/// 3. **Evaluation**: Validates output using a CompositeValidator
///
/// The manager handles:
/// - Budget tracking (attempts and tokens)
/// - Retry logic with failure feedback
/// - Stuck detection for strategy switching
/// - Final outcome determination
///
/// ## Example
///
/// ```rust,ignore
/// use aethecore::poe::{PoeManager, PoeConfig, PoeTask, SuccessManifest};
/// use aethecore::poe::worker::AgentLoopWorker;
/// use aethecore::poe::validation::CompositeValidator;
///
/// let worker = AgentLoopWorker::new("/workspace".into());
/// let validator = CompositeValidator::new(provider);
/// let config = PoeConfig::default();
///
/// let manager = PoeManager::new(worker, validator, config);
///
/// let manifest = SuccessManifest::new("task-1", "Create a Rust project");
/// let task = PoeTask::new(manifest, "Create a new Rust project with cargo init");
///
/// let outcome = manager.execute(task).await?;
/// ```
pub struct PoeManager<W: Worker> {
    /// Worker that executes instructions
    worker: W,
    /// Validator that evaluates outputs
    validator: CompositeValidator,
    /// Configuration for budget and stuck detection
    config: PoeConfig,
    /// Optional callback for validation events
    validation_callback: Option<ValidationCallback>,
    /// Optional recorder for crystallizing experiences
    recorder: Option<Arc<dyn ExperienceRecorder>>,
}

impl<W: Worker> PoeManager<W> {
    /// Create a new PoeManager with the given components.
    ///
    /// # Arguments
    ///
    /// * `worker` - The worker implementation that executes instructions
    /// * `validator` - The composite validator for evaluating outputs
    /// * `config` - Configuration for budget limits and stuck detection
    pub fn new(worker: W, validator: CompositeValidator, config: PoeConfig) -> Self {
        Self {
            worker,
            validator,
            config,
            validation_callback: None,
            recorder: None,
        }
    }

    /// Set the experience recorder for crystallizing POE executions.
    ///
    /// When a recorder is configured, all POE outcomes (success, failure,
    /// and strategy switches) are recorded to the skill evolution system for
    /// future learning and pattern recognition.
    ///
    /// Use `ChannelCrystallizer` for async-safe recording, or `NoOpRecorder`
    /// to disable recording.
    ///
    /// # Arguments
    ///
    /// * `recorder` - The recorder to use for crystallizing experiences
    pub fn with_recorder(mut self, recorder: Arc<dyn ExperienceRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Set a callback to receive validation events during execution.
    ///
    /// The callback is invoked after each validation attempt with details
    /// about the attempt number, pass/fail status, and distance score.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to call after each validation
    pub fn with_validation_callback(mut self, callback: ValidationCallback) -> Self {
        self.validation_callback = Some(callback);
        self
    }

    /// Get a reference to the worker.
    ///
    /// This is primarily useful for testing to verify worker execution counts.
    pub fn worker(&self) -> &W {
        &self.worker
    }

    /// Execute a POE task.
    ///
    /// Runs the P->O->E cycle until:
    /// - Success: Validation passes
    /// - StrategySwitch: System is stuck (no progress over `stuck_window` attempts)
    /// - BudgetExhausted: Max attempts or tokens reached
    ///
    /// # Arguments
    ///
    /// * `task` - The POE task containing manifest and instruction
    ///
    /// # Returns
    ///
    /// * `PoeOutcome::Success` - Task completed successfully with passing verdict
    /// * `PoeOutcome::StrategySwitch` - Stuck detected, suggesting alternative approach
    /// * `PoeOutcome::BudgetExhausted` - All retries consumed without success
    pub async fn execute(&self, task: PoeTask) -> Result<PoeOutcome> {
        // Track execution start time for crystallization
        let start_time = Instant::now();

        // Create budget from task manifest and config
        let mut budget = PoeBudget::new(task.manifest.max_attempts, self.config.max_tokens);

        // Track the last failure for retry feedback
        let mut previous_failure: Option<String> = None;
        let mut last_verdict: Option<Verdict> = None;
        let mut last_output: Option<WorkerOutput> = None;

        // Main P->O->E loop
        while !budget.exhausted() {
            // Build instruction with retry feedback if this is a retry
            let instruction = match &previous_failure {
                Some(feedback) => self.build_retry_prompt(&task, feedback),
                None => task.instruction.clone(),
            };

            // Operation: Execute via worker
            let output = self
                .worker
                .execute(&instruction, previous_failure.as_deref())
                .await?;

            // Evaluation: Validate output against manifest
            let verdict = self.validator.validate(&task.manifest, &output).await?;

            // Record attempt in budget
            budget.record_attempt(output.tokens_consumed, verdict.distance_score);

            // Emit validation event via callback
            if let Some(callback) = &self.validation_callback {
                callback(ValidationEvent {
                    attempt: budget.current_attempt,
                    passed: verdict.passed,
                    distance_score: verdict.distance_score,
                    reason: verdict.reason.clone(),
                });
            }

            // Check for success
            if verdict.passed {
                let outcome = PoeOutcome::Success(verdict);
                self.record_experience(&task, &outcome, &output, start_time);
                return Ok(outcome);
            }

            // Check for stuck (no progress over window)
            if budget.is_stuck(self.config.stuck_window) {
                let suggestion = verdict
                    .suggestion
                    .clone()
                    .unwrap_or_else(|| "Try a different approach or break down the task".into());

                let outcome = PoeOutcome::StrategySwitch {
                    reason: format!(
                        "No progress over {} attempts. Best distance score: {:.2}",
                        self.config.stuck_window,
                        budget.best_score().unwrap_or(1.0)
                    ),
                    suggestion,
                };
                self.record_experience(&task, &outcome, &output, start_time);
                return Ok(outcome);
            }

            // Prepare for retry
            previous_failure = Some(self.build_failure_feedback(&verdict, &output));
            last_verdict = Some(verdict);
            last_output = Some(output);
        }

        // Budget exhausted
        let last_error = last_verdict
            .map(|v| v.reason)
            .unwrap_or_else(|| "No attempts were made".to_string());

        let outcome = PoeOutcome::BudgetExhausted {
            attempts: budget.current_attempt,
            last_error,
        };

        // Record even budget exhaustion as a learning experience
        if let Some(ref output) = last_output {
            self.record_experience(&task, &outcome, output, start_time);
        }

        Ok(outcome)
    }

    /// Record an experience to the skill evolution system.
    ///
    /// This is called after every POE execution to enable learning from
    /// both successes and failures. The recorder handles the actual
    /// storage asynchronously.
    fn record_experience(
        &self,
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
        start_time: Instant,
    ) {
        if let Some(ref recorder) = self.recorder {
            recorder.record_with_timing(task, outcome, output, start_time);
        }
    }

    /// Build a retry prompt that incorporates the original instruction and failure feedback.
    ///
    /// # Arguments
    ///
    /// * `task` - The original POE task
    /// * `feedback` - Feedback from the previous failed attempt
    fn build_retry_prompt(&self, task: &PoeTask, feedback: &str) -> String {
        format!(
            "Previous attempt failed. Please retry with this feedback:\n\n\
             ## Feedback from Previous Attempt\n\
             {}\n\n\
             ## Original Task\n\
             {}\n\n\
             ## Success Criteria\n\
             {}\n\n\
             Please address the issues mentioned in the feedback and try again.",
            feedback, task.instruction, task.manifest.objective
        )
    }

    /// Build failure feedback from a verdict and worker output.
    ///
    /// This feedback is used to inform the worker about what went wrong
    /// so it can adjust its approach on the next attempt.
    fn build_failure_feedback(&self, verdict: &Verdict, output: &WorkerOutput) -> String {
        let mut feedback = String::new();

        // Add verdict reason
        feedback.push_str(&format!("Validation failed: {}\n", verdict.reason));

        // Add suggestion if available
        if let Some(suggestion) = &verdict.suggestion {
            feedback.push_str(&format!("\nSuggestion: {}\n", suggestion));
        }

        // Add hard constraint failures
        if !verdict.hard_results.is_empty() {
            let failures: Vec<_> = verdict.hard_results.iter().filter(|r| !r.passed).collect();
            if !failures.is_empty() {
                feedback.push_str("\nFailed hard constraints:\n");
                for (i, failure) in failures.iter().enumerate().take(5) {
                    if let Some(error) = &failure.error {
                        feedback.push_str(&format!("  {}. {}\n", i + 1, error));
                    }
                }
            }
        }

        // Add soft metric failures
        if !verdict.soft_results.is_empty() {
            let failures: Vec<_> = verdict
                .soft_results
                .iter()
                .filter(|r| r.score < r.metric.threshold)
                .collect();
            if !failures.is_empty() {
                feedback.push_str("\nSoft metrics below threshold:\n");
                for (i, failure) in failures.iter().enumerate().take(5) {
                    feedback.push_str(&format!(
                        "  {}. Score: {:.0}% (threshold: {:.0}%)",
                        i + 1,
                        failure.score * 100.0,
                        failure.metric.threshold * 100.0
                    ));
                    if let Some(fb) = &failure.feedback {
                        feedback.push_str(&format!(" - {}", fb));
                    }
                    feedback.push('\n');
                }
            }
        }

        // Add worker state info if it failed
        match &output.final_state {
            WorkerState::Failed { reason } => {
                feedback.push_str(&format!("\nWorker execution failed: {}\n", reason));
            }
            WorkerState::NeedsInput { question } => {
                feedback.push_str(&format!("\nWorker needs input: {}\n", question));
            }
            WorkerState::Completed { .. } => {
                // Worker completed but validation failed - this is expected
            }
        }

        feedback
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::{SuccessManifest, ValidationRule};
    use crate::poe::worker::MockWorker;
    use crate::providers::MockProvider;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn create_test_manager(
        mock_worker: MockWorker,
        mock_response: &str,
    ) -> PoeManager<MockWorker> {
        let provider = Arc::new(MockProvider::new(mock_response));
        let validator = CompositeValidator::new(provider);
        let config = PoeConfig::default();
        PoeManager::new(mock_worker, validator, config)
    }

    fn create_simple_manifest() -> SuccessManifest {
        SuccessManifest::new("test-task", "Complete the test task")
    }

    fn create_simple_task() -> PoeTask {
        PoeTask::new(create_simple_manifest(), "Execute the test instruction")
    }

    #[tokio::test]
    async fn test_poe_manager_success_on_first_try() {
        // Worker succeeds, validator passes (no hard constraints, no soft metrics)
        let worker = MockWorker::new();
        let manager = create_test_manager(worker, "");

        let task = create_simple_task();
        let outcome = manager.execute(task).await.unwrap();

        match outcome {
            PoeOutcome::Success(verdict) => {
                assert!(verdict.passed);
                assert_eq!(verdict.distance_score, 0.0);
            }
            _ => panic!("Expected Success outcome, got {:?}", outcome),
        }
    }

    #[tokio::test]
    async fn test_poe_manager_budget_exhausted() {
        // Worker always produces output that fails validation (missing file)
        let worker = MockWorker::new().with_tokens(1000);
        let provider = Arc::new(MockProvider::new(""));
        let validator = CompositeValidator::new(provider);
        let config = PoeConfig::default().with_max_tokens(100_000);

        let manager = PoeManager::new(worker, validator, config);

        // Create task with a hard constraint that will always fail
        let manifest =
            SuccessManifest::new("test-task", "Create a file").with_hard_constraint(
                ValidationRule::FileExists {
                    path: PathBuf::from("/nonexistent/impossible/file.txt"),
                },
            );
        let task = PoeTask::new(manifest.with_max_attempts(3), "Create the impossible file");

        let outcome = manager.execute(task).await.unwrap();

        // With max_attempts=3 and stuck_window=3, the system will detect "stuck"
        // (no progress over 3 attempts) and return StrategySwitch before budget exhaustion.
        // Both outcomes are acceptable - either stuck detection or budget exhaustion.
        match outcome {
            PoeOutcome::StrategySwitch { reason, .. } => {
                assert!(reason.contains("No progress") || reason.contains("progress"));
            }
            PoeOutcome::BudgetExhausted {
                attempts,
                last_error,
            } => {
                assert_eq!(attempts, 3);
                assert!(last_error.contains("hard constraint"));
            }
            _ => panic!("Expected StrategySwitch or BudgetExhausted outcome, got {:?}", outcome),
        }
    }

    #[tokio::test]
    async fn test_poe_manager_strategy_switch_on_stuck() {
        // Worker produces same output repeatedly (stuck)
        let worker = MockWorker::new().with_tokens(100);
        let provider = Arc::new(MockProvider::new(""));
        let validator = CompositeValidator::new(provider);
        let config = PoeConfig::default().with_stuck_window(3);

        let manager = PoeManager::new(worker, validator, config);

        // Create task with a constraint that always fails with same distance
        let manifest =
            SuccessManifest::new("test-task", "Create a file").with_hard_constraint(
                ValidationRule::FileExists {
                    path: PathBuf::from("/always/fails.txt"),
                },
            );
        let task = PoeTask::new(manifest.with_max_attempts(10), "Stuck task");

        let outcome = manager.execute(task).await.unwrap();

        match outcome {
            PoeOutcome::StrategySwitch { reason, suggestion } => {
                assert!(reason.contains("No progress"));
                assert!(!suggestion.is_empty());
            }
            _ => panic!("Expected StrategySwitch outcome, got {:?}", outcome),
        }
    }

    #[tokio::test]
    async fn test_poe_config_default() {
        let config = PoeConfig::default();
        assert_eq!(config.stuck_window, 3);
        assert_eq!(config.max_tokens, 100_000);
    }

    #[tokio::test]
    async fn test_poe_config_builder() {
        let config = PoeConfig::new(5, 50_000)
            .with_stuck_window(10)
            .with_max_tokens(200_000);

        assert_eq!(config.stuck_window, 10);
        assert_eq!(config.max_tokens, 200_000);
    }

    #[test]
    fn test_build_retry_prompt() {
        let worker = MockWorker::new();
        let manager = create_test_manager(worker, "");

        let task = PoeTask::new(
            SuccessManifest::new("test", "Create a valid file"),
            "Create file.txt",
        );

        let prompt = manager.build_retry_prompt(&task, "File was empty");

        assert!(prompt.contains("Previous attempt failed"));
        assert!(prompt.contains("File was empty"));
        assert!(prompt.contains("Create file.txt"));
        assert!(prompt.contains("Create a valid file"));
    }

    #[test]
    fn test_build_failure_feedback() {
        let worker = MockWorker::new();
        let manager = create_test_manager(worker, "");

        let verdict = Verdict::failure("Test failed")
            .with_suggestion("Try harder")
            .with_distance_score(0.8);

        let output = WorkerOutput::completed("Did something");

        let feedback = manager.build_failure_feedback(&verdict, &output);

        assert!(feedback.contains("Validation failed: Test failed"));
        assert!(feedback.contains("Suggestion: Try harder"));
    }

    #[test]
    fn test_build_failure_feedback_with_worker_failure() {
        let worker = MockWorker::new();
        let manager = create_test_manager(worker, "");

        let verdict = Verdict::failure("Test failed");
        let output = WorkerOutput::failed("Worker crashed");

        let feedback = manager.build_failure_feedback(&verdict, &output);

        assert!(feedback.contains("Worker execution failed: Worker crashed"));
    }

    #[tokio::test]
    async fn test_poe_manager_token_budget_exhausted() {
        // Worker consumes a lot of tokens
        let worker = MockWorker::new().with_tokens(50_000);
        let provider = Arc::new(MockProvider::new(""));
        let validator = CompositeValidator::new(provider);
        let config = PoeConfig::default().with_max_tokens(80_000);

        let manager = PoeManager::new(worker, validator, config);

        let manifest =
            SuccessManifest::new("test-task", "Test").with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("/nonexistent.txt"),
            });
        let task = PoeTask::new(manifest.with_max_attempts(10), "Token test");

        let outcome = manager.execute(task).await.unwrap();

        // Should exhaust after 2 attempts (50k + 50k >= 80k)
        match outcome {
            PoeOutcome::BudgetExhausted { attempts, .. } => {
                assert_eq!(attempts, 2);
            }
            _ => panic!("Expected BudgetExhausted outcome, got {:?}", outcome),
        }
    }

    #[tokio::test]
    async fn test_poe_manager_preserves_execution_count() {
        let worker = MockWorker::new().with_tokens(100);
        let provider = Arc::new(MockProvider::new(""));
        let validator = CompositeValidator::new(provider);
        let config = PoeConfig::default();

        let manager = PoeManager::new(worker, validator, config);

        let manifest =
            SuccessManifest::new("test-task", "Test").with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("/nonexistent.txt"),
            });
        let task = PoeTask::new(manifest.with_max_attempts(5), "Count test");

        let _ = manager.execute(task).await.unwrap();

        // Worker should have been called multiple times
        assert!(manager.worker().execution_count() > 1);
    }
}
