//! Experience crystallization for POE execution.
//!
//! This module connects POE outcomes to the skill evolution system, enabling
//! learning from execution experiences. Successful patterns are recorded and
//! can be reused for similar tasks in the future.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌──────────────────────┐     ┌─────────────────────┐
//! │   PoeManager    │────▶│ ExperienceCrystallizer│────▶│  EvolutionTracker   │
//! │ (Execute Tasks) │     │  (Map POE -> Skills)  │     │ (Log & Aggregate)   │
//! └─────────────────┘     └──────────────────────┘     └─────────────────────┘
//! ```
//!
//! The crystallizer:
//! 1. Records all POE outcomes (success, failure, strategy switch)
//! 2. Maps POE types to skill execution records
//! 3. Generates pattern IDs from task objectives for grouping similar tasks
//! 4. Calculates satisfaction scores from validation verdicts
//!
//! ## Migrated from Cortex
//!
//! The following submodules were migrated from `crate::memory::cortex`:
//! - `experience` — Core types (Experience, DistillationTask, etc.)
//! - `distillation` — Distillation service for processing experiences
//! - `pattern_extractor` — LLM-based pattern extraction
//! - `clustering` — Experience clustering and deduplication
//! - `dreaming` — Background batch processing
//!
//! ## Thread Safety
//!
//! The `ExperienceRecorder` trait provides a `Send + Sync` interface for recording
//! experiences, allowing integration with async code like tokio::spawn. The concrete
//! `ExperienceCrystallizer` implementation handles the actual database operations.

// Submodules migrated from Cortex
pub mod clustering;
pub mod cognitive_entropy;
pub mod distillation;
pub mod dreaming;
pub mod experience;
pub mod experience_store;
pub mod idle_detector;
pub mod pattern_extractor;
pub mod pattern_model;
pub mod synthesis_backend;
pub mod provider_backend;

use crate::sync_primitives::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::{debug, error};

use crate::error::Result;
use crate::skill_evolution::{EvolutionTracker, ExecutionStatus, SkillExecution};

use super::types::{PoeOutcome, PoeTask, WorkerOutput};

// ============================================================================
// ExperienceRecorder Trait
// ============================================================================

/// Trait for recording POE experiences.
///
/// This trait provides a `Send + Sync` interface for experience recording,
/// allowing it to be used in async contexts that require `Send` futures.
///
/// The main implementation is `ChannelCrystallizer`, which uses a background
/// task to handle the actual database operations, avoiding Send/Sync issues
/// with rusqlite.
pub trait ExperienceRecorder: Send + Sync {
    /// Record a POE task execution.
    ///
    /// # Arguments
    ///
    /// * `task` - The POE task that was executed
    /// * `outcome` - The outcome of the execution
    /// * `output` - The worker output from execution
    fn record(
        &self,
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
    );

    /// Record a POE task execution with timing information.
    ///
    /// # Arguments
    ///
    /// * `task` - The POE task that was executed
    /// * `outcome` - The outcome of the execution
    /// * `output` - The worker output from execution
    /// * `start_time` - When execution started
    fn record_with_timing(
        &self,
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
        start_time: Instant,
    );
}

/// No-op recorder that discards all experiences.
///
/// This is useful when crystallization is disabled or not configured.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpRecorder;

impl ExperienceRecorder for NoOpRecorder {
    fn record(
        &self,
        _task: &PoeTask,
        _outcome: &PoeOutcome,
        _output: &WorkerOutput,
    ) {
        // No-op
    }

    fn record_with_timing(
        &self,
        _task: &PoeTask,
        _outcome: &PoeOutcome,
        _output: &WorkerOutput,
        _start_time: Instant,
    ) {
        // No-op
    }
}

// ============================================================================
// ExperienceRecord - Message type for channel
// ============================================================================

/// A record to be crystallized, sent via channel to the background worker.
#[derive(Debug, Clone)]
pub struct ExperienceRecord {
    /// The skill execution record
    pub execution: SkillExecution,
}

// ============================================================================
// ChannelCrystallizer - Send + Sync crystallizer using channels
// ============================================================================

/// A `Send + Sync` crystallizer that uses channels to communicate with a
/// background worker that handles database operations.
///
/// This design avoids the `Send` issues with `rusqlite::Connection` by
/// keeping the connection on a dedicated background task and using
/// message passing for communication.
///
/// ## Example
///
/// ```rust,ignore
/// use alephcore::poe::{ChannelCrystallizer, ExperienceRecorder};
/// use alephcore::skill_evolution::EvolutionTracker;
///
/// // Create the crystallizer and background worker
/// let tracker = EvolutionTracker::new("evolution.db")?;
/// let (crystallizer, worker) = ChannelCrystallizer::new();
///
/// // Spawn the worker on a dedicated task
/// tokio::spawn(worker.run(tracker));
///
/// // Use the crystallizer (it's Send + Sync)
/// crystallizer.record(&task, &outcome, &output);
/// ```
pub struct ChannelCrystallizer {
    /// Sender for experience records
    sender: mpsc::UnboundedSender<ExperienceRecord>,
}

impl ChannelCrystallizer {
    /// Create a new channel-based crystallizer.
    ///
    /// Returns both the crystallizer (for recording) and the worker (for
    /// running on a background task).
    pub fn new() -> (Arc<Self>, CrystallizerWorker) {
        let (tx, rx) = mpsc::unbounded_channel();
        let crystallizer = Arc::new(Self { sender: tx });
        let worker = CrystallizerWorker { receiver: rx };
        (crystallizer, worker)
    }

    /// Create a skill execution from POE types.
    fn create_execution(
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
        duration_ms: Option<u64>,
    ) -> SkillExecution {
        // Determine execution status and satisfaction from outcome
        let (status, satisfaction) = match outcome {
            PoeOutcome::Success { verdict, .. } if verdict.passed => {
                let sat = 1.0 - verdict.distance_score;
                (ExecutionStatus::Success, Some(sat))
            }
            PoeOutcome::Success { verdict, .. } => {
                let sat = (1.0 - verdict.distance_score) * 0.5;
                (ExecutionStatus::PartialSuccess, Some(sat))
            }
            PoeOutcome::StrategySwitch { .. } => {
                (ExecutionStatus::PartialSuccess, Some(0.3))
            }
            PoeOutcome::BudgetExhausted { .. } => {
                (ExecutionStatus::Failed, Some(0.0))
            }
            PoeOutcome::DecompositionRequired { .. } => {
                // Decomposition is not a failure — it's a signal to split; treat as partial
                (ExecutionStatus::PartialSuccess, Some(0.5))
            }
        };

        // Calculate output length from artifacts
        let output_length: u32 = output
            .artifacts
            .iter()
            .map(|a| a.path.to_string_lossy().len() as u32)
            .sum::<u32>()
            .saturating_add(output.execution_log.len() as u32 * 100);

        // Use provided duration or estimate from steps
        let duration = duration_ms.unwrap_or_else(|| output.steps_taken as u64 * 1000);

        SkillExecution {
            id: uuid::Uuid::new_v4().to_string(),
            skill_id: Self::generate_pattern_id(&task.manifest.objective),
            session_id: task.manifest.task_id.clone(),
            invoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            duration_ms: duration,
            status,
            satisfaction,
            context: task.manifest.objective.clone(),
            input_summary: truncate(&task.instruction, 100),
            output_length,
        }
    }

    /// Generate a pattern ID from an objective.
    fn generate_pattern_id(objective: &str) -> String {
        const STOP_WORDS: &[&str] = &[
            "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for",
            "of", "with", "by", "from", "into", "that", "this", "these", "those",
            "what", "which", "when", "where", "how", "why", "then", "than",
            "just", "only", "also", "some", "any", "each", "every", "all", "both",
        ];

        let lowercase = objective.to_lowercase();
        let keywords: Vec<String> = lowercase
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 3 && !STOP_WORDS.contains(w))
            .take(3)
            .map(String::from)
            .collect();

        if keywords.is_empty() {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(objective.as_bytes());
            let hash = format!("{:x}", hasher.finalize());
            format!("poe-task-{}", &hash[..8])
        } else {
            format!("poe-{}", keywords.join("-"))
        }
    }
}

impl ExperienceRecorder for ChannelCrystallizer {
    fn record(
        &self,
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
    ) {
        let execution = Self::create_execution(task, outcome, output, None);
        let record = ExperienceRecord { execution };

        debug!(
            skill_id = %record.execution.skill_id,
            status = ?record.execution.status,
            "Recording POE experience"
        );

        if let Err(e) = self.sender.send(record) {
            error!("Failed to send experience record: {}", e);
        }
    }

    fn record_with_timing(
        &self,
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
        start_time: Instant,
    ) {
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let execution = Self::create_execution(task, outcome, output, Some(duration_ms));
        let record = ExperienceRecord { execution };

        debug!(
            skill_id = %record.execution.skill_id,
            status = ?record.execution.status,
            duration_ms = duration_ms,
            "Recording POE experience with timing"
        );

        if let Err(e) = self.sender.send(record) {
            error!("Failed to send experience record: {}", e);
        }
    }
}

// ============================================================================
// CrystallizerWorker - Background worker for database operations
// ============================================================================

/// Background worker that processes experience records.
///
/// This worker receives records via channel and writes them to the
/// evolution tracker. It should be spawned on a dedicated task.
pub struct CrystallizerWorker {
    receiver: mpsc::UnboundedReceiver<ExperienceRecord>,
}

impl CrystallizerWorker {
    /// Run the worker, processing records until the channel is closed.
    ///
    /// Because `EvolutionTracker` contains `rusqlite::Connection` which is not `Send`,
    /// this method is synchronous and should be spawned with `spawn_blocking`:
    ///
    /// ```rust,ignore
    /// tokio::task::spawn_blocking(move || worker.run_blocking(tracker));
    /// ```
    pub fn run_blocking(mut self, tracker: EvolutionTracker) {
        // Use blocking recv since we're in a blocking context
        while let Some(record) = self.receiver.blocking_recv() {
            if let Err(e) = tracker.log_execution(&record.execution) {
                error!(
                    skill_id = %record.execution.skill_id,
                    error = %e,
                    "Failed to log experience"
                );
            }
        }
        debug!("Crystallizer worker shutting down");
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Truncate a string to a maximum length, adding ellipsis if needed (UTF-8 safe).
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let target = max_len.saturating_sub(3);
        let end = s.char_indices()
            .take_while(|(i, _)| *i <= target)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("{}...", &s[..end])
    }
}

// ============================================================================
// ExperienceCrystallizer
// ============================================================================

/// Crystallizes POE execution experiences into the skill evolution system.
///
/// The crystallizer bridges the POE execution framework with skill evolution,
/// recording task outcomes so the system can learn from experience and improve
/// future task handling.
///
/// ## Usage
///
/// ```rust,ignore
/// use alephcore::poe::ExperienceCrystallizer;
/// use alephcore::skill_evolution::EvolutionTracker;
///
/// let tracker = Arc::new(EvolutionTracker::new("evolution.db")?);
/// let crystallizer = ExperienceCrystallizer::new(tracker);
///
/// // After POE execution...
/// crystallizer.record(&task, &outcome, &output).await?;
/// ```
pub struct ExperienceCrystallizer {
    /// The evolution tracker that stores execution records
    tracker: Arc<EvolutionTracker>,
}

impl ExperienceCrystallizer {
    /// Create a new ExperienceCrystallizer with the given evolution tracker.
    pub fn new(tracker: Arc<EvolutionTracker>) -> Self {
        Self { tracker }
    }

    /// Record a POE task execution in the skill evolution system.
    ///
    /// This method maps the POE task, outcome, and worker output to a skill
    /// execution record and logs it via the evolution tracker.
    ///
    /// # Arguments
    ///
    /// * `task` - The POE task that was executed
    /// * `outcome` - The outcome of the execution
    /// * `output` - The worker output from execution
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Record was successfully logged
    /// * `Err(_)` - Failed to log the record
    pub fn record(
        &self,
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
    ) -> Result<()> {
        let execution = self.map_to_skill_execution(task, outcome, output);

        debug!(
            skill_id = %execution.skill_id,
            status = ?execution.status,
            satisfaction = ?execution.satisfaction,
            "Recording POE experience"
        );

        self.tracker.log_execution(&execution)
    }

    /// Record a POE task execution with timing information.
    ///
    /// This variant accepts a start time to calculate accurate duration_ms.
    ///
    /// # Arguments
    ///
    /// * `task` - The POE task that was executed
    /// * `outcome` - The outcome of the execution
    /// * `output` - The worker output from execution
    /// * `start_time` - When execution started
    pub fn record_with_timing(
        &self,
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
        start_time: Instant,
    ) -> Result<()> {
        let mut execution = self.map_to_skill_execution(task, outcome, output);
        execution.duration_ms = start_time.elapsed().as_millis() as u64;

        debug!(
            skill_id = %execution.skill_id,
            status = ?execution.status,
            duration_ms = execution.duration_ms,
            "Recording POE experience with timing"
        );

        self.tracker.log_execution(&execution)
    }

    /// Map POE types to a SkillExecution record.
    ///
    /// This method translates POE concepts to skill evolution concepts:
    /// - PoeOutcome::Success with passed verdict -> ExecutionStatus::Success
    /// - PoeOutcome::Success with failed verdict -> ExecutionStatus::PartialSuccess
    /// - PoeOutcome::StrategySwitch -> ExecutionStatus::PartialSuccess (learned something)
    /// - PoeOutcome::BudgetExhausted -> ExecutionStatus::Failed
    fn map_to_skill_execution(
        &self,
        task: &PoeTask,
        outcome: &PoeOutcome,
        output: &WorkerOutput,
    ) -> SkillExecution {
        // Determine execution status and satisfaction from outcome
        let (status, satisfaction) = match outcome {
            PoeOutcome::Success { verdict, .. } if verdict.passed => {
                // Full success: satisfaction based on distance score (0.0 = perfect -> 1.0 satisfaction)
                let sat = 1.0 - verdict.distance_score;
                (ExecutionStatus::Success, Some(sat))
            }
            PoeOutcome::Success { verdict, .. } => {
                // Completed but validation failed
                let sat = (1.0 - verdict.distance_score) * 0.5; // Lower satisfaction
                (ExecutionStatus::PartialSuccess, Some(sat))
            }
            PoeOutcome::StrategySwitch { .. } => {
                // Strategy switch - partial success because we learned the task needs different approach
                (ExecutionStatus::PartialSuccess, Some(0.3))
            }
            PoeOutcome::BudgetExhausted { .. } => {
                // Complete failure
                (ExecutionStatus::Failed, Some(0.0))
            }
            PoeOutcome::DecompositionRequired { .. } => {
                // Decomposition is not a failure — it's a signal to split; treat as partial
                (ExecutionStatus::PartialSuccess, Some(0.5))
            }
        };

        // Calculate output length from artifacts
        let output_length: u32 = output
            .artifacts
            .iter()
            .map(|a| a.path.to_string_lossy().len() as u32)
            .sum::<u32>()
            .saturating_add(output.execution_log.len() as u32 * 100);

        // Estimate duration from steps if not available elsewhere
        // Assume ~1 second per step as a rough estimate
        let duration_ms = output.steps_taken as u64 * 1000;

        SkillExecution {
            id: uuid::Uuid::new_v4().to_string(),
            skill_id: self.generate_pattern_id(&task.manifest.objective),
            session_id: task.manifest.task_id.clone(),
            invoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            duration_ms,
            status,
            satisfaction,
            context: task.manifest.objective.clone(),
            input_summary: truncate(&task.instruction, 100),
            output_length,
        }
    }

    /// Generate a pattern ID from an objective for grouping similar tasks.
    ///
    /// The pattern ID is derived by extracting meaningful keywords from the
    /// objective and combining them into a stable identifier. This allows
    /// similar tasks to be grouped together for analysis.
    ///
    /// # Algorithm
    ///
    /// 1. Lowercase the objective
    /// 2. Split into words
    /// 3. Filter out short words (<=3 chars) and stop words
    /// 4. Take the first 3 meaningful words
    /// 5. Join with hyphens, prefixed by "poe-"
    ///
    /// # Example
    ///
    /// "Create a new Rust file with unit tests" -> "poe-create-rust-file"
    fn generate_pattern_id(&self, objective: &str) -> String {
        // Common stop words to filter out
        const STOP_WORDS: &[&str] = &[
            "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for",
            "of", "with", "by", "from", "into", "that", "this", "these", "those",
            "what", "which", "when", "where", "how", "why", "then", "than",
            "just", "only", "also", "some", "any", "each", "every", "all", "both",
        ];

        // First lowercase the objective, then work with owned strings
        let lowercase = objective.to_lowercase();
        let keywords: Vec<String> = lowercase
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 3 && !STOP_WORDS.contains(w))
            .take(3)
            .map(String::from)
            .collect();

        if keywords.is_empty() {
            // Fallback: use hash of objective
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(objective.as_bytes());
            let hash = format!("{:x}", hasher.finalize());
            format!("poe-task-{}", &hash[..8])
        } else {
            format!("poe-{}", keywords.join("-"))
        }
    }

    /// Get a reference to the underlying evolution tracker.
    pub fn tracker(&self) -> &EvolutionTracker {
        &self.tracker
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::{SuccessManifest, Verdict};

    fn create_test_tracker() -> Arc<EvolutionTracker> {
        Arc::new(EvolutionTracker::in_memory().expect("Failed to create in-memory tracker"))
    }

    fn create_test_task() -> PoeTask {
        let manifest = SuccessManifest::new("test-task-1", "Create a new Rust file with unit tests");
        PoeTask::new(manifest, "Create src/lib.rs with basic tests")
    }

    fn create_test_output() -> WorkerOutput {
        let mut output = WorkerOutput::completed("Created file successfully");
        output.tokens_consumed = 500;
        output.steps_taken = 3;
        output
    }

    #[test]
    fn test_crystallizer_creation() {
        let tracker = create_test_tracker();
        let _crystallizer = ExperienceCrystallizer::new(tracker.clone());

        // Just verify creation works
        assert!(tracker.get_metrics("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_record_success() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker.clone());

        let task = create_test_task();
        let outcome = PoeOutcome::success(Verdict::success("All tests passed").with_distance_score(0.1), "");
        let output = create_test_output();

        // Record should succeed
        let result = crystallizer.record(&task, &outcome, &output);
        assert!(result.is_ok());

        // Verify metrics were updated
        let metrics = tracker.get_metrics("poe-create-rust-file").unwrap();
        assert!(metrics.is_some());
        let m = metrics.unwrap();
        assert_eq!(m.total_executions, 1);
        assert_eq!(m.successful_executions, 1);
    }

    #[test]
    fn test_record_failure() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker.clone());

        let task = create_test_task();
        let outcome = PoeOutcome::BudgetExhausted {
            attempts: 5,
            last_error: "Max retries exceeded".to_string(),
        };
        let output = WorkerOutput::failed("Could not complete task");

        let result = crystallizer.record(&task, &outcome, &output);
        assert!(result.is_ok());

        // Verify failure was recorded
        let metrics = tracker.get_metrics("poe-create-rust-file").unwrap();
        assert!(metrics.is_some());
        let m = metrics.unwrap();
        assert_eq!(m.total_executions, 1);
        assert_eq!(m.successful_executions, 0); // Failed doesn't count as success
    }

    #[test]
    fn test_record_strategy_switch() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker.clone());

        let task = create_test_task();
        let outcome = PoeOutcome::StrategySwitch {
            reason: "Task too complex".to_string(),
            suggestion: "Break into smaller steps".to_string(),
        };
        let output = create_test_output();

        let result = crystallizer.record(&task, &outcome, &output);
        assert!(result.is_ok());

        // Strategy switch is partial success
        let metrics = tracker.get_metrics("poe-create-rust-file").unwrap();
        assert!(metrics.is_some());
        let m = metrics.unwrap();
        assert_eq!(m.total_executions, 1);
        assert_eq!(m.successful_executions, 1); // Partial success counts
    }

    #[test]
    fn test_generate_pattern_id_basic() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker);

        let pattern = crystallizer.generate_pattern_id("Create a new Rust file");
        assert_eq!(pattern, "poe-create-rust-file");
    }

    #[test]
    fn test_generate_pattern_id_filters_stop_words() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker);

        // "the", "a", "with" are stop words
        let pattern = crystallizer.generate_pattern_id("Implement the authentication with JWT tokens");
        assert_eq!(pattern, "poe-implement-authentication-tokens");
    }

    #[test]
    fn test_generate_pattern_id_short_objective() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker);

        // "fix" is 3 chars, "bug" is 3 chars - both filtered
        let pattern = crystallizer.generate_pattern_id("Fix a bug");
        // Should fallback to hash
        assert!(pattern.starts_with("poe-task-"));
    }

    #[test]
    fn test_generate_pattern_id_special_chars() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker);

        let pattern = crystallizer.generate_pattern_id("Create src/lib.rs with unit-tests");
        // "src/lib.rs" splits on '/' and '.', "unit-tests" splits on '-'
        assert_eq!(pattern, "poe-create-unit-tests");
    }

    #[test]
    fn test_truncate_short() {
        let s = "Hello";
        assert_eq!(truncate(s, 10), "Hello");
    }

    #[test]
    fn test_truncate_long() {
        let s = "This is a very long string that needs truncation";
        assert_eq!(truncate(s, 20), "This is a very lo...");
    }

    #[test]
    fn test_map_execution_status_success() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker);

        let task = create_test_task();
        let verdict = Verdict::success("All passed").with_distance_score(0.0);
        let outcome = PoeOutcome::success(verdict, "");
        let output = create_test_output();

        let execution = crystallizer.map_to_skill_execution(&task, &outcome, &output);

        assert_eq!(execution.status, ExecutionStatus::Success);
        assert_eq!(execution.satisfaction, Some(1.0)); // Perfect score
    }

    #[test]
    fn test_map_execution_status_partial() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker);

        let task = create_test_task();
        // Verdict passed but with distance
        let verdict = Verdict::success("Mostly passed").with_distance_score(0.3);
        let outcome = PoeOutcome::success(verdict, "");
        let output = create_test_output();

        let execution = crystallizer.map_to_skill_execution(&task, &outcome, &output);

        assert_eq!(execution.status, ExecutionStatus::Success);
        // satisfaction = 1.0 - 0.3 = 0.7
        assert!((execution.satisfaction.unwrap() - 0.7).abs() < 0.01);
    }

    #[test]
    fn test_map_execution_status_failed() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker);

        let task = create_test_task();
        let outcome = PoeOutcome::BudgetExhausted {
            attempts: 5,
            last_error: "Failed".to_string(),
        };
        let output = WorkerOutput::failed("Error");

        let execution = crystallizer.map_to_skill_execution(&task, &outcome, &output);

        assert_eq!(execution.status, ExecutionStatus::Failed);
        assert_eq!(execution.satisfaction, Some(0.0));
    }

    #[test]
    fn test_input_summary_truncation() {
        let tracker = create_test_tracker();
        let crystallizer = ExperienceCrystallizer::new(tracker);

        let manifest = SuccessManifest::new("test-task", "Test objective");
        let long_instruction = "a".repeat(200);
        let task = PoeTask::new(manifest, long_instruction);
        let outcome = PoeOutcome::success(Verdict::success("Done"), "");
        let output = create_test_output();

        let execution = crystallizer.map_to_skill_execution(&task, &outcome, &output);

        // Should be truncated to ~100 chars + "..."
        assert!(execution.input_summary.len() <= 103);
        assert!(execution.input_summary.ends_with("..."));
    }

    // ========== ChannelCrystallizer Tests ==========

    #[test]
    fn test_channel_crystallizer_creation() {
        let (crystallizer, _worker) = ChannelCrystallizer::new();

        // Just verify creation works
        let task = create_test_task();
        let outcome = PoeOutcome::success(Verdict::success("Done"), "");
        let output = create_test_output();

        // This should not panic - sends to channel
        crystallizer.record(&task, &outcome, &output);
    }

    #[test]
    fn test_channel_crystallizer_generate_pattern_id() {
        let pattern = ChannelCrystallizer::generate_pattern_id("Create a new Rust file");
        assert_eq!(pattern, "poe-create-rust-file");
    }

    #[tokio::test]
    async fn test_channel_crystallizer_with_worker() {
        let (crystallizer, worker) = ChannelCrystallizer::new();

        // Spawn the worker using spawn_blocking
        // EvolutionTracker must be created inside the spawn_blocking closure
        // because rusqlite::Connection is !Send + !Sync
        let worker_handle = tokio::task::spawn_blocking(move || {
            let tracker = EvolutionTracker::in_memory().expect("Failed to create tracker");
            worker.run_blocking(tracker);
        });

        // Record some experiences
        let task = create_test_task();
        let outcome = PoeOutcome::success(Verdict::success("Done"), "");
        let output = create_test_output();

        crystallizer.record(&task, &outcome, &output);

        // Drop the crystallizer to close the channel
        drop(crystallizer);

        // Wait for worker to finish
        worker_handle.await.unwrap();
    }

    // ========== NoOpRecorder Tests ==========

    #[test]
    fn test_noop_recorder() {
        let recorder = NoOpRecorder;

        let task = create_test_task();
        let outcome = PoeOutcome::success(Verdict::success("Done"), "");
        let output = create_test_output();

        // Should not panic
        recorder.record(&task, &outcome, &output);
        recorder.record_with_timing(&task, &outcome, &output, Instant::now());
    }

    // ========== ExperienceRecorder Trait Tests ==========

    #[test]
    fn test_experience_recorder_trait_object() {
        // Verify the trait can be used as a trait object
        let recorder: Arc<dyn ExperienceRecorder> = Arc::new(NoOpRecorder);

        let task = create_test_task();
        let outcome = PoeOutcome::success(Verdict::success("Done"), "");
        let output = create_test_output();

        recorder.record(&task, &outcome, &output);
    }
}
