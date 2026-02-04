# POE Architecture Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the POE (Principle-Operation-Evaluation) architecture to transform Aleph from a chat-based assistant into a goal-oriented professional agent.

**Architecture:** Independent `poe/` module that wraps existing `AgentLoop` as a Worker. POE Manager orchestrates the P→O→E cycle with entropy-based budget control. Reuses `spec_driven::LlmJudge` for semantic validation.

**Tech Stack:** Rust, async-trait, serde, tokio, existing AiProvider abstraction

**Design Document:** `docs/plans/2026-02-01-poe-architecture-design.md`

**Worktree:** `/Volumes/TBU4/Workspace/Aleph/.worktrees/feat-poe-architecture`

---

## Task 1: Create POE Module Structure

**Files:**
- Create: `core/src/poe/mod.rs`
- Modify: `core/src/lib.rs` (add `pub mod poe;`)

**Step 1: Create the poe module directory**

```bash
mkdir -p core/src/poe/validation
```

**Step 2: Create mod.rs with submodule declarations**

Create `core/src/poe/mod.rs`:

```rust
//! POE (Principle-Operation-Evaluation) Architecture
//!
//! A goal-oriented agent execution framework that:
//! 1. **Principle**: Defines success criteria before execution (SuccessManifest)
//! 2. **Operation**: Executes with heuristic guidance (Worker abstraction)
//! 3. **Evaluation**: Validates results with mixed hard/semantic checks
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      POE Manager                             │
//! ├─────────────────────────────────────────────────────────────┤
//! │  P: ManifestBuilder → SuccessManifest                       │
//! │  O: Worker.execute() ← Experience retrieval                 │
//! │  E: CompositeValidator → Verdict                            │
//! │  Loop: Success → Crystallize | Stuck → Switch | Retry       │
//! └─────────────────────────────────────────────────────────────┘
//! ```

pub mod budget;
pub mod types;
pub mod validation;
pub mod worker;
pub mod manager;

// Re-exports
pub use budget::PoeBudget;
pub use types::{
    JudgeTarget, ModelTier, PoeOutcome, PoeTask, SoftMetric, SuccessManifest,
    ValidationRule, Verdict, WorkerOutput, WorkerState,
};
pub use validation::CompositeValidator;
pub use worker::{Worker, AgentLoopWorker};
pub use manager::PoeManager;
```

**Step 3: Add poe module to lib.rs**

In `core/src/lib.rs`, find the module declarations section and add:

```rust
pub mod poe;
```

**Step 4: Create placeholder files**

```bash
touch core/src/poe/types.rs
touch core/src/poe/budget.rs
touch core/src/poe/worker.rs
touch core/src/poe/manager.rs
touch core/src/poe/validation/mod.rs
touch core/src/poe/validation/hard.rs
touch core/src/poe/validation/semantic.rs
touch core/src/poe/validation/composite.rs
```

**Step 5: Verify compilation**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -5`
Expected: Errors about empty files (we'll fix in next tasks)

**Step 6: Commit**

```bash
git add core/src/poe/ core/src/lib.rs
git commit -m "poe: scaffold module structure

Create POE (Principle-Operation-Evaluation) module skeleton:
- mod.rs with submodule declarations
- Placeholder files for types, budget, worker, manager
- validation/ submodule for hard/semantic/composite validators"
```

---

## Task 2: Implement Core Types (`types.rs`)

**Files:**
- Create: `core/src/poe/types.rs`

**Step 1: Write types.rs with all core data structures**

Create `core/src/poe/types.rs`:

```rust
//! Core types for POE architecture.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

// =============================================================================
// P - Principle: Success Contract
// =============================================================================

/// Success Manifest: Defines what "done" means before execution starts.
///
/// This is the first-principles anchor - we define success criteria
/// before any work begins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessManifest {
    /// Unique task identifier
    pub task_id: String,

    /// First-principles objective statement
    /// Example: "Refactor auth module to use JWT tokens"
    pub objective: String,

    /// Hard constraints (AND logic - all must pass)
    /// These are deterministic checks executed by Rust code.
    pub hard_constraints: Vec<ValidationRule>,

    /// Soft metrics (weighted scoring for optimization direction)
    /// These use LLM evaluation for subjective quality.
    pub soft_metrics: Vec<SoftMetric>,

    /// Maximum retry attempts before giving up
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u8,

    /// Rollback snapshot path (for recovery on failure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_snapshot: Option<PathBuf>,
}

fn default_max_attempts() -> u8 {
    5
}

impl SuccessManifest {
    /// Create a new manifest with the given objective
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

    /// Add a hard constraint
    pub fn with_hard_constraint(mut self, rule: ValidationRule) -> Self {
        self.hard_constraints.push(rule);
        self
    }

    /// Add a soft metric
    pub fn with_soft_metric(mut self, metric: SoftMetric) -> Self {
        self.soft_metrics.push(metric);
        self
    }

    /// Set max attempts
    pub fn with_max_attempts(mut self, max: u8) -> Self {
        self.max_attempts = max;
        self
    }
}

/// Soft metric with weight and threshold
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftMetric {
    /// The validation rule to apply
    pub rule: ValidationRule,
    /// Weight for scoring (0.0 - 1.0)
    pub weight: f32,
    /// Minimum score to pass (e.g., 0.8)
    pub threshold: f32,
}

impl SoftMetric {
    pub fn new(rule: ValidationRule, weight: f32, threshold: f32) -> Self {
        Self { rule, weight, threshold }
    }
}

// =============================================================================
// Validation Rules
// =============================================================================

/// Validation rule types covering file system, execution, and semantic checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum ValidationRule {
    // --- File System Layer ---
    /// File must exist at path
    FileExists { path: PathBuf },
    /// File must NOT exist at path
    FileNotExists { path: PathBuf },
    /// File must contain pattern (regex)
    FileContains { path: PathBuf, pattern: String },
    /// File must NOT contain pattern
    FileNotContains { path: PathBuf, pattern: String },
    /// Directory structure must match expected JSON tree
    DirStructureMatch { root: PathBuf, expected: String },

    // --- Execution Layer ---
    /// Command must exit with code 0
    CommandPasses {
        cmd: String,
        args: Vec<String>,
        #[serde(default = "default_timeout")]
        timeout_ms: u64,
    },
    /// Command output must contain pattern
    CommandOutputContains {
        cmd: String,
        args: Vec<String>,
        pattern: String,
        #[serde(default = "default_timeout")]
        timeout_ms: u64,
    },

    // --- Data Layer ---
    /// JSON file must be valid against schema
    JsonSchemaValid { path: PathBuf, schema: String },

    // --- Semantic Layer (LLM Judge) ---
    /// LLM-based semantic evaluation
    SemanticCheck {
        target: JudgeTarget,
        prompt: String,
        passing_criteria: String,
        #[serde(default)]
        model_tier: ModelTier,
    },
}

fn default_timeout() -> u64 {
    60_000 // 60 seconds
}

/// Target for LLM judge evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum JudgeTarget {
    /// Evaluate a file's contents
    File(PathBuf),
    /// Evaluate raw content string
    Content(String),
    /// Evaluate command output
    CommandOutput { cmd: String, args: Vec<String> },
}

/// Model tier for LLM judge (cost/capability tradeoff)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelTier {
    /// Local fast model (Llama 3 8B)
    LocalFast,
    /// Cloud fast model (GPT-4o-mini, Claude Haiku)
    #[default]
    CloudFast,
    /// Cloud smart model (GPT-4o, Claude Sonnet)
    CloudSmart,
    /// Cloud deep model with extended thinking (o1, Claude Opus)
    CloudDeep,
}

// =============================================================================
// E - Evaluation: Verdict
// =============================================================================

/// Validation verdict returned by CompositeValidator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    /// Overall pass/fail
    pub passed: bool,
    /// Distance to goal (0.0 = perfect, 1.0 = complete failure)
    /// Used for entropy tracking
    pub distance_score: f32,
    /// Human-readable explanation
    pub reason: String,
    /// Suggestion for fixing (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// Individual hard constraint results
    pub hard_results: Vec<RuleResult>,
    /// Individual soft metric results
    pub soft_results: Vec<SoftRuleResult>,
}

impl Verdict {
    /// Create a passing verdict
    pub fn success() -> Self {
        Self {
            passed: true,
            distance_score: 0.0,
            reason: "All validations passed".into(),
            suggestion: None,
            hard_results: Vec::new(),
            soft_results: Vec::new(),
        }
    }

    /// Create a failing verdict
    pub fn failure(reason: impl Into<String>, suggestion: Option<String>) -> Self {
        Self {
            passed: false,
            distance_score: 1.0,
            reason: reason.into(),
            suggestion,
            hard_results: Vec::new(),
            soft_results: Vec::new(),
        }
    }
}

/// Result of a single hard validation rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleResult {
    /// The rule that was checked
    pub rule: ValidationRule,
    /// Whether it passed
    pub passed: bool,
    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of a single soft metric evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftRuleResult {
    /// The metric that was checked
    pub metric: SoftMetric,
    /// Score from 0.0 to 1.0
    pub score: f32,
    /// Feedback from LLM judge
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

// =============================================================================
// O - Operation: Worker Output
// =============================================================================

/// Output from a Worker execution
#[derive(Debug, Clone)]
pub struct WorkerOutput {
    /// Tokens consumed during execution
    pub tokens_consumed: u32,
    /// Number of steps taken
    pub steps_taken: u32,
    /// Final state of the worker
    pub final_state: WorkerState,
    /// Artifacts produced (files created/modified)
    pub artifacts: Vec<Artifact>,
    /// Execution log for debugging
    pub execution_log: Vec<StepLog>,
}

/// Worker's final state
#[derive(Debug, Clone)]
pub enum WorkerState {
    /// Successfully completed
    Completed { summary: String },
    /// Failed with error
    Failed { reason: String },
    /// Needs user input
    NeedsInput { question: String },
}

/// An artifact produced by the worker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// File path
    pub path: PathBuf,
    /// Type of change
    pub change_type: ChangeType,
    /// Content hash for comparison
    pub content_hash: String,
}

/// Type of file change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
}

/// A single step in the execution log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepLog {
    /// Step number
    pub step_id: u32,
    /// Action taken
    pub action: String,
    /// Result of the action
    pub result: String,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

// =============================================================================
// POE Task and Outcome
// =============================================================================

/// A complete POE task ready for execution
#[derive(Debug, Clone)]
pub struct PoeTask {
    /// The success contract
    pub manifest: SuccessManifest,
    /// The instruction to execute
    pub instruction: String,
}

impl PoeTask {
    pub fn new(manifest: SuccessManifest, instruction: impl Into<String>) -> Self {
        Self {
            manifest,
            instruction: instruction.into(),
        }
    }
}

/// Final outcome of POE execution
#[derive(Debug, Clone)]
pub enum PoeOutcome {
    /// Task completed successfully
    Success(Verdict),
    /// Strategy switch needed (stuck in local optimum)
    StrategySwitch { reason: String, suggestion: String },
    /// Budget exhausted, human intervention needed
    BudgetExhausted { attempts: u8, last_error: String },
}

// =============================================================================
// Experience (for Crystallizer - future task)
// =============================================================================

/// Experience record for crystallization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    pub id: String,
    pub task_pattern: TaskPattern,
    pub solution_path: SolutionPath,
    pub outcome: ExperienceOutcome,
    pub created_at: DateTime<Utc>,
    pub usage_count: u32,
    pub success_rate: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPattern {
    pub objective_keywords: Vec<String>,
    pub constraint_types: Vec<String>,
    pub context_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionPath {
    pub strategy_summary: String,
    pub key_steps: Vec<String>,
    pub tools_used: Vec<String>,
    pub pitfalls_avoided: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceOutcome {
    pub attempts_needed: u8,
    pub final_score: f32,
    pub tokens_consumed: u32,
    pub duration_ms: u64,
}
```

**Step 2: Verify compilation**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -10`
Expected: Errors about missing budget/validation/worker/manager modules (expected, we'll implement next)

**Step 3: Commit**

```bash
git add core/src/poe/types.rs
git commit -m "poe: implement core types

Add comprehensive type definitions:
- SuccessManifest with hard constraints and soft metrics
- ValidationRule enum covering file/command/semantic checks
- JudgeTarget and ModelTier for LLM evaluation
- Verdict with distance_score for entropy tracking
- WorkerOutput and WorkerState
- PoeTask and PoeOutcome for execution flow
- Experience types for future crystallization"
```

---

## Task 3: Implement Budget Manager (`budget.rs`)

**Files:**
- Create: `core/src/poe/budget.rs`

**Step 1: Write budget.rs**

Create `core/src/poe/budget.rs`:

```rust
//! Entropy-based budget management for POE execution.
//!
//! Tracks attempts, tokens, and entropy history to:
//! - Prevent infinite retry loops
//! - Detect when we're stuck (no progress)
//! - Enforce resource limits

use serde::{Deserialize, Serialize};

/// POE execution budget with entropy tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoeBudget {
    /// Maximum allowed attempts
    pub max_attempts: u8,
    /// Current attempt number (1-indexed)
    pub current_attempt: u8,
    /// Maximum tokens allowed
    pub max_tokens: u32,
    /// Tokens consumed so far
    pub tokens_used: u32,
    /// History of distance scores (entropy values)
    /// 0.0 = perfect, 1.0 = complete failure
    #[serde(default)]
    pub entropy_history: Vec<f32>,
}

impl Default for PoeBudget {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            current_attempt: 0,
            max_tokens: 100_000,
            tokens_used: 0,
            entropy_history: Vec::new(),
        }
    }
}

impl PoeBudget {
    /// Create a new budget with specified limits
    pub fn new(max_attempts: u8, max_tokens: u32) -> Self {
        Self {
            max_attempts,
            current_attempt: 0,
            max_tokens,
            tokens_used: 0,
            entropy_history: Vec::new(),
        }
    }

    /// Check if budget is exhausted (attempts or tokens)
    pub fn exhausted(&self) -> bool {
        self.current_attempt >= self.max_attempts || self.tokens_used >= self.max_tokens
    }

    /// Check if we're stuck (no entropy reduction over window)
    ///
    /// Returns true if the last `window` attempts show no improvement.
    /// This indicates we're in a local optimum and need strategy change.
    pub fn is_stuck(&self, window: usize) -> bool {
        if self.entropy_history.len() < window {
            return false;
        }

        // Get the last `window` entropy values
        let recent: Vec<f32> = self.entropy_history
            .iter()
            .rev()
            .take(window)
            .copied()
            .collect();

        // Check if entropy is not decreasing
        // (values are in reverse order, so we check if each is <= previous)
        recent.windows(2).all(|w| w[0] >= w[1])
    }

    /// Record an attempt with its entropy (distance_score)
    pub fn record_attempt(&mut self, tokens: u32, distance_score: f32) {
        self.current_attempt += 1;
        self.tokens_used += tokens;
        self.entropy_history.push(distance_score);
    }

    /// Get remaining attempts
    pub fn remaining_attempts(&self) -> u8 {
        self.max_attempts.saturating_sub(self.current_attempt)
    }

    /// Get remaining tokens
    pub fn remaining_tokens(&self) -> u32 {
        self.max_tokens.saturating_sub(self.tokens_used)
    }

    /// Calculate entropy trend over last N attempts
    /// Returns: negative = improving, positive = degrading, zero = stuck
    pub fn entropy_trend(&self, window: usize) -> f32 {
        if self.entropy_history.len() < 2 {
            return 0.0;
        }

        let recent: Vec<f32> = self.entropy_history
            .iter()
            .rev()
            .take(window.min(self.entropy_history.len()))
            .copied()
            .collect();

        if recent.len() < 2 {
            return 0.0;
        }

        // Calculate average change (positive = entropy increasing = getting worse)
        let changes: Vec<f32> = recent
            .windows(2)
            .map(|w| w[0] - w[1]) // reversed order, so this is new - old
            .collect();

        changes.iter().sum::<f32>() / changes.len() as f32
    }

    /// Get a human-readable status
    pub fn status(&self) -> BudgetStatus {
        if self.exhausted() {
            BudgetStatus::Exhausted
        } else if self.is_stuck(3) {
            BudgetStatus::Stuck
        } else if self.entropy_trend(3) > 0.1 {
            BudgetStatus::Degrading
        } else if self.entropy_trend(3) < -0.1 {
            BudgetStatus::Improving
        } else {
            BudgetStatus::Stable
        }
    }
}

/// Budget status for decision making
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetStatus {
    /// Making progress toward goal
    Improving,
    /// No significant change
    Stable,
    /// Getting worse
    Degrading,
    /// Stuck in local optimum
    Stuck,
    /// Budget exhausted
    Exhausted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_exhausted_by_attempts() {
        let mut budget = PoeBudget::new(3, 100_000);
        assert!(!budget.exhausted());

        budget.record_attempt(1000, 0.8);
        budget.record_attempt(1000, 0.6);
        budget.record_attempt(1000, 0.4);

        assert!(budget.exhausted());
    }

    #[test]
    fn test_budget_exhausted_by_tokens() {
        let mut budget = PoeBudget::new(10, 5000);
        assert!(!budget.exhausted());

        budget.record_attempt(3000, 0.8);
        budget.record_attempt(3000, 0.6);

        assert!(budget.exhausted());
    }

    #[test]
    fn test_is_stuck_detects_no_progress() {
        let mut budget = PoeBudget::new(10, 100_000);

        // Improving initially
        budget.record_attempt(1000, 0.9);
        budget.record_attempt(1000, 0.8);
        assert!(!budget.is_stuck(3));

        // Now stuck
        budget.record_attempt(1000, 0.8);
        budget.record_attempt(1000, 0.8);
        budget.record_attempt(1000, 0.85); // slight regression

        assert!(budget.is_stuck(3));
    }

    #[test]
    fn test_entropy_trend() {
        let mut budget = PoeBudget::new(10, 100_000);

        // Improving trend (entropy decreasing)
        budget.record_attempt(1000, 0.9);
        budget.record_attempt(1000, 0.7);
        budget.record_attempt(1000, 0.5);

        let trend = budget.entropy_trend(3);
        assert!(trend < 0.0, "Expected negative trend (improving), got {}", trend);
    }

    #[test]
    fn test_remaining_resources() {
        let mut budget = PoeBudget::new(5, 10000);
        budget.record_attempt(3000, 0.8);

        assert_eq!(budget.remaining_attempts(), 4);
        assert_eq!(budget.remaining_tokens(), 7000);
    }
}
```

**Step 2: Verify compilation**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -5`
Expected: Still errors about missing validation/worker/manager (expected)

**Step 3: Run tests**

Run: `cargo test -p alephcore budget:: 2>&1 | tail -20`
Expected: Budget tests should pass

**Step 4: Commit**

```bash
git add core/src/poe/budget.rs
git commit -m "poe: implement entropy-based budget manager

Add PoeBudget with:
- Attempt and token tracking
- Entropy history for progress monitoring
- is_stuck() detection for local optimum
- entropy_trend() calculation
- BudgetStatus enum for decision making

Includes comprehensive tests."
```

---

## Task 4: Implement Hard Validation (`validation/hard.rs`)

**Files:**
- Create: `core/src/poe/validation/hard.rs`
- Create: `core/src/poe/validation/mod.rs`

**Step 1: Create validation/mod.rs**

Create `core/src/poe/validation/mod.rs`:

```rust
//! Validation subsystem for POE architecture.
//!
//! Provides both deterministic (hard) and semantic (LLM) validation.

pub mod hard;
pub mod semantic;
pub mod composite;

pub use hard::HardValidator;
pub use semantic::SemanticValidator;
pub use composite::CompositeValidator;
```

**Step 2: Write hard.rs**

Create `core/src/poe/validation/hard.rs`:

```rust
//! Hard validation rules executed deterministically by Rust code.
//!
//! These checks are fast, cheap, and definitive. They run first
//! to fail fast before spending LLM tokens on semantic checks.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::error::Result;
use crate::poe::types::{RuleResult, ValidationRule};

/// Hard validator for deterministic checks
pub struct HardValidator;

impl HardValidator {
    pub fn new() -> Self {
        Self
    }

    /// Validate all hard constraints, returning results for each
    pub async fn validate_all(&self, rules: &[ValidationRule]) -> Result<Vec<RuleResult>> {
        let mut results = Vec::with_capacity(rules.len());

        for rule in rules {
            let result = self.validate_single(rule).await;
            results.push(result);
        }

        Ok(results)
    }

    /// Validate a single rule
    pub async fn validate_single(&self, rule: &ValidationRule) -> RuleResult {
        match rule {
            ValidationRule::FileExists { path } => {
                self.check_file_exists(path, rule)
            }
            ValidationRule::FileNotExists { path } => {
                self.check_file_not_exists(path, rule)
            }
            ValidationRule::FileContains { path, pattern } => {
                self.check_file_contains(path, pattern, rule).await
            }
            ValidationRule::FileNotContains { path, pattern } => {
                self.check_file_not_contains(path, pattern, rule).await
            }
            ValidationRule::DirStructureMatch { root, expected } => {
                self.check_dir_structure(root, expected, rule).await
            }
            ValidationRule::CommandPasses { cmd, args, timeout_ms } => {
                self.check_command_passes(cmd, args, *timeout_ms, rule).await
            }
            ValidationRule::CommandOutputContains { cmd, args, pattern, timeout_ms } => {
                self.check_command_output_contains(cmd, args, pattern, *timeout_ms, rule).await
            }
            ValidationRule::JsonSchemaValid { path, schema } => {
                self.check_json_schema(path, schema, rule).await
            }
            // Semantic checks are handled by SemanticValidator
            ValidationRule::SemanticCheck { .. } => {
                RuleResult {
                    rule: rule.clone(),
                    passed: true, // Skip - handled by SemanticValidator
                    error: None,
                }
            }
        }
    }

    fn check_file_exists(&self, path: &Path, rule: &ValidationRule) -> RuleResult {
        let exists = path.exists();
        debug!(path = %path.display(), exists, "Checking file exists");

        RuleResult {
            rule: rule.clone(),
            passed: exists,
            error: if exists {
                None
            } else {
                Some(format!("File not found: {}", path.display()))
            },
        }
    }

    fn check_file_not_exists(&self, path: &Path, rule: &ValidationRule) -> RuleResult {
        let exists = path.exists();
        debug!(path = %path.display(), exists, "Checking file not exists");

        RuleResult {
            rule: rule.clone(),
            passed: !exists,
            error: if !exists {
                None
            } else {
                Some(format!("File should not exist: {}", path.display()))
            },
        }
    }

    async fn check_file_contains(
        &self,
        path: &Path,
        pattern: &str,
        rule: &ValidationRule,
    ) -> RuleResult {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                let regex = match Regex::new(pattern) {
                    Ok(r) => r,
                    Err(e) => {
                        return RuleResult {
                            rule: rule.clone(),
                            passed: false,
                            error: Some(format!("Invalid regex pattern: {}", e)),
                        };
                    }
                };

                let found = regex.is_match(&content);
                debug!(path = %path.display(), pattern, found, "Checking file contains");

                RuleResult {
                    rule: rule.clone(),
                    passed: found,
                    error: if found {
                        None
                    } else {
                        Some(format!(
                            "Pattern '{}' not found in {}",
                            pattern,
                            path.display()
                        ))
                    },
                }
            }
            Err(e) => RuleResult {
                rule: rule.clone(),
                passed: false,
                error: Some(format!("Failed to read file: {}", e)),
            },
        }
    }

    async fn check_file_not_contains(
        &self,
        path: &Path,
        pattern: &str,
        rule: &ValidationRule,
    ) -> RuleResult {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                let regex = match Regex::new(pattern) {
                    Ok(r) => r,
                    Err(e) => {
                        return RuleResult {
                            rule: rule.clone(),
                            passed: false,
                            error: Some(format!("Invalid regex pattern: {}", e)),
                        };
                    }
                };

                let found = regex.is_match(&content);
                debug!(path = %path.display(), pattern, found, "Checking file not contains");

                RuleResult {
                    rule: rule.clone(),
                    passed: !found,
                    error: if !found {
                        None
                    } else {
                        Some(format!(
                            "Pattern '{}' should not be in {}",
                            pattern,
                            path.display()
                        ))
                    },
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File not existing means pattern is definitely not in it
                RuleResult {
                    rule: rule.clone(),
                    passed: true,
                    error: None,
                }
            }
            Err(e) => RuleResult {
                rule: rule.clone(),
                passed: false,
                error: Some(format!("Failed to read file: {}", e)),
            },
        }
    }

    async fn check_dir_structure(
        &self,
        root: &Path,
        expected: &str,
        rule: &ValidationRule,
    ) -> RuleResult {
        // Parse expected structure as JSON
        let expected_tree: serde_json::Value = match serde_json::from_str(expected) {
            Ok(v) => v,
            Err(e) => {
                return RuleResult {
                    rule: rule.clone(),
                    passed: false,
                    error: Some(format!("Invalid expected structure JSON: {}", e)),
                };
            }
        };

        // Check structure recursively
        match self.verify_structure(root, &expected_tree).await {
            Ok(()) => RuleResult {
                rule: rule.clone(),
                passed: true,
                error: None,
            },
            Err(e) => RuleResult {
                rule: rule.clone(),
                passed: false,
                error: Some(e),
            },
        }
    }

    async fn verify_structure(
        &self,
        path: &Path,
        expected: &serde_json::Value,
    ) -> std::result::Result<(), String> {
        match expected {
            serde_json::Value::Object(map) => {
                for (name, child) in map {
                    let child_path = path.join(name);
                    if !child_path.exists() {
                        return Err(format!("Missing: {}", child_path.display()));
                    }
                    Box::pin(self.verify_structure(&child_path, child)).await?;
                }
                Ok(())
            }
            serde_json::Value::Null => {
                // Null means "must exist" (file or empty dir)
                if path.exists() {
                    Ok(())
                } else {
                    Err(format!("Missing: {}", path.display()))
                }
            }
            _ => Err(format!("Invalid structure spec at {}", path.display())),
        }
    }

    async fn check_command_passes(
        &self,
        cmd: &str,
        args: &[String],
        timeout_ms: u64,
        rule: &ValidationRule,
    ) -> RuleResult {
        debug!(cmd, ?args, timeout_ms, "Running command");

        let result = timeout(
            Duration::from_millis(timeout_ms),
            Command::new(cmd)
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) if output.status.success() => {
                debug!(cmd, "Command passed");
                RuleResult {
                    rule: rule.clone(),
                    passed: true,
                    error: None,
                }
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                warn!(cmd, code = ?output.status.code(), "Command failed");

                RuleResult {
                    rule: rule.clone(),
                    passed: false,
                    error: Some(format!(
                        "Command failed with exit code {:?}\nstderr: {}\nstdout: {}",
                        output.status.code(),
                        stderr.chars().take(500).collect::<String>(),
                        stdout.chars().take(500).collect::<String>(),
                    )),
                }
            }
            Ok(Err(e)) => RuleResult {
                rule: rule.clone(),
                passed: false,
                error: Some(format!("Failed to execute command: {}", e)),
            },
            Err(_) => RuleResult {
                rule: rule.clone(),
                passed: false,
                error: Some(format!("Command timed out after {}ms", timeout_ms)),
            },
        }
    }

    async fn check_command_output_contains(
        &self,
        cmd: &str,
        args: &[String],
        pattern: &str,
        timeout_ms: u64,
        rule: &ValidationRule,
    ) -> RuleResult {
        let result = timeout(
            Duration::from_millis(timeout_ms),
            Command::new(cmd)
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = format!("{}{}", stdout, stderr);

                let regex = match Regex::new(pattern) {
                    Ok(r) => r,
                    Err(e) => {
                        return RuleResult {
                            rule: rule.clone(),
                            passed: false,
                            error: Some(format!("Invalid regex pattern: {}", e)),
                        };
                    }
                };

                let found = regex.is_match(&combined);

                RuleResult {
                    rule: rule.clone(),
                    passed: found,
                    error: if found {
                        None
                    } else {
                        Some(format!(
                            "Pattern '{}' not found in command output",
                            pattern
                        ))
                    },
                }
            }
            Ok(Err(e)) => RuleResult {
                rule: rule.clone(),
                passed: false,
                error: Some(format!("Failed to execute command: {}", e)),
            },
            Err(_) => RuleResult {
                rule: rule.clone(),
                passed: false,
                error: Some(format!("Command timed out after {}ms", timeout_ms)),
            },
        }
    }

    async fn check_json_schema(
        &self,
        path: &Path,
        schema: &str,
        rule: &ValidationRule,
    ) -> RuleResult {
        // Read the JSON file
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                return RuleResult {
                    rule: rule.clone(),
                    passed: false,
                    error: Some(format!("Failed to read JSON file: {}", e)),
                };
            }
        };

        // Parse as JSON
        let json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                return RuleResult {
                    rule: rule.clone(),
                    passed: false,
                    error: Some(format!("Invalid JSON: {}", e)),
                };
            }
        };

        // Parse schema
        let schema_value: serde_json::Value = match serde_json::from_str(schema) {
            Ok(v) => v,
            Err(e) => {
                return RuleResult {
                    rule: rule.clone(),
                    passed: false,
                    error: Some(format!("Invalid schema JSON: {}", e)),
                };
            }
        };

        // For now, do basic type checking
        // TODO: Use jsonschema crate for full validation
        let type_match = match schema_value.get("type") {
            Some(serde_json::Value::String(t)) => match t.as_str() {
                "object" => json.is_object(),
                "array" => json.is_array(),
                "string" => json.is_string(),
                "number" => json.is_number(),
                "boolean" => json.is_boolean(),
                "null" => json.is_null(),
                _ => true,
            },
            _ => true,
        };

        RuleResult {
            rule: rule.clone(),
            passed: type_match,
            error: if type_match {
                None
            } else {
                Some("JSON does not match schema type".into())
            },
        }
    }
}

impl Default for HardValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_file_exists() {
        let validator = HardValidator::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // File doesn't exist yet
        let rule = ValidationRule::FileExists { path: file_path.clone() };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);

        // Create the file
        tokio::fs::write(&file_path, "hello").await.unwrap();
        let result = validator.validate_single(&rule).await;
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_file_contains() {
        let validator = HardValidator::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        tokio::fs::write(&file_path, "hello world").await.unwrap();

        let rule = ValidationRule::FileContains {
            path: file_path.clone(),
            pattern: "world".into(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed);

        let rule = ValidationRule::FileContains {
            path: file_path,
            pattern: "foo".into(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_command_passes() {
        let validator = HardValidator::new();

        // echo should pass
        let rule = ValidationRule::CommandPasses {
            cmd: "echo".into(),
            args: vec!["hello".into()],
            timeout_ms: 5000,
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed);

        // false should fail
        let rule = ValidationRule::CommandPasses {
            cmd: "false".into(),
            args: vec![],
            timeout_ms: 5000,
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
    }
}
```

**Step 3: Add regex to Cargo.toml if not present**

Check and add if needed:
```bash
grep -q "^regex" core/Cargo.toml || echo 'regex = "1"' >> core/Cargo.toml
```

**Step 4: Verify compilation**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -5`

**Step 5: Run tests**

Run: `cargo test -p alephcore hard:: 2>&1 | tail -20`
Expected: Hard validation tests pass

**Step 6: Commit**

```bash
git add core/src/poe/validation/
git commit -m "poe: implement hard validation rules

Add HardValidator with deterministic checks:
- FileExists / FileNotExists
- FileContains / FileNotContains (regex)
- DirStructureMatch (JSON tree)
- CommandPasses / CommandOutputContains
- JsonSchemaValid (basic type checking)

Includes async execution with timeouts."
```

---

## Task 5: Implement Semantic Validation (`validation/semantic.rs`)

**Files:**
- Create: `core/src/poe/validation/semantic.rs`

**Step 1: Write semantic.rs**

Create `core/src/poe/validation/semantic.rs`:

```rust
//! Semantic validation using LLM judges.
//!
//! Reuses the existing spec_driven::LlmJudge infrastructure
//! with POE-specific prompting.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::error::{AlephError, Result};
use crate::providers::AiProvider;
use crate::agents::thinking::ThinkLevel;
use crate::poe::types::{JudgeTarget, ModelTier, SoftMetric, SoftRuleResult, ValidationRule};

/// System prompt for POE semantic evaluation
const POE_JUDGE_SYSTEM_PROMPT: &str = r#"You are an impartial AI judge evaluating content against specific criteria.

Your job is to evaluate ONLY against the criteria given. Be objective and precise.

Output ONLY valid JSON in this exact format:
{
  "passed": true/false,
  "score": 0-100,
  "reason": "Brief explanation",
  "suggestion": "How to fix if failed (or null if passed)"
}

Scoring:
- 90-100: Excellent, fully meets criteria
- 70-89: Good, minor issues
- 50-69: Acceptable, needs improvement
- 30-49: Poor, significant issues
- 0-29: Fails to meet criteria

Output ONLY JSON, no markdown."#;

/// Semantic validator using LLM judges
pub struct SemanticValidator {
    provider: Arc<dyn AiProvider>,
}

impl SemanticValidator {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Validate all soft metrics, returning results for each
    pub async fn validate_all(&self, metrics: &[SoftMetric]) -> Result<Vec<SoftRuleResult>> {
        let mut results = Vec::with_capacity(metrics.len());

        // TODO: Run in parallel with futures::future::try_join_all
        // For now, sequential to avoid rate limits
        for metric in metrics {
            let result = self.validate_single(metric).await?;
            results.push(result);
        }

        Ok(results)
    }

    /// Validate a single soft metric
    pub async fn validate_single(&self, metric: &SoftMetric) -> Result<SoftRuleResult> {
        match &metric.rule {
            ValidationRule::SemanticCheck {
                target,
                prompt,
                passing_criteria,
                model_tier,
            } => {
                // Resolve target to content
                let content = self.resolve_target(target).await?;

                // Build evaluation prompt
                let eval_prompt = format!(
                    "## Evaluation Criteria\n{}\n\n\
                     ## Passing Standard\n{}\n\n\
                     ## Content to Evaluate\n```\n{}\n```",
                    prompt,
                    passing_criteria,
                    content.chars().take(10000).collect::<String>(), // Truncate if too long
                );

                // Determine thinking level based on model tier
                let think_level = match model_tier {
                    ModelTier::LocalFast | ModelTier::CloudFast => ThinkLevel::Off,
                    ModelTier::CloudSmart => ThinkLevel::Low,
                    ModelTier::CloudDeep => ThinkLevel::High,
                };

                debug!(
                    prompt_len = eval_prompt.len(),
                    ?think_level,
                    "Calling LLM judge"
                );

                // Call LLM
                let response = if self.provider.supports_thinking() && think_level != ThinkLevel::Off {
                    self.provider
                        .process_with_thinking(&eval_prompt, Some(POE_JUDGE_SYSTEM_PROMPT), think_level)
                        .await?
                } else {
                    self.provider
                        .process(&eval_prompt, Some(POE_JUDGE_SYSTEM_PROMPT))
                        .await?
                };

                // Parse response
                let verdict = self.parse_response(&response)?;

                info!(
                    score = verdict.score,
                    passed = verdict.passed,
                    "Semantic validation complete"
                );

                Ok(SoftRuleResult {
                    metric: metric.clone(),
                    score: verdict.score as f32 / 100.0,
                    feedback: Some(verdict.reason),
                })
            }
            _ => {
                // Non-semantic rules shouldn't be in soft metrics
                warn!("Non-semantic rule in soft metrics, skipping");
                Ok(SoftRuleResult {
                    metric: metric.clone(),
                    score: 1.0,
                    feedback: None,
                })
            }
        }
    }

    /// Resolve a JudgeTarget to its content string
    async fn resolve_target(&self, target: &JudgeTarget) -> Result<String> {
        match target {
            JudgeTarget::File(path) => {
                tokio::fs::read_to_string(path)
                    .await
                    .map_err(|e| AlephError::internal(format!(
                        "Failed to read file {}: {}",
                        path.display(),
                        e
                    )))
            }
            JudgeTarget::Content(content) => Ok(content.clone()),
            JudgeTarget::CommandOutput { cmd, args } => {
                let output = timeout(
                    Duration::from_secs(60),
                    Command::new(cmd)
                        .args(args)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .output(),
                )
                .await
                .map_err(|_| AlephError::internal("Command timed out"))?
                .map_err(|e| AlephError::internal(format!("Command failed: {}", e)))?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                Ok(format!("STDOUT:\n{}\n\nSTDERR:\n{}", stdout, stderr))
            }
        }
    }

    /// Parse LLM response into verdict
    fn parse_response(&self, response: &str) -> Result<JudgeVerdict> {
        // Try to extract JSON from response
        let json_str = extract_json(response);

        serde_json::from_str(&json_str).map_err(|e| {
            AlephError::internal(format!(
                "Failed to parse judge response: {}. Response: {}",
                e,
                response.chars().take(200).collect::<String>()
            ))
        })
    }
}

/// Internal verdict structure from LLM
#[derive(Debug, serde::Deserialize)]
struct JudgeVerdict {
    passed: bool,
    score: u8,
    reason: String,
    #[serde(default)]
    suggestion: Option<String>,
}

/// Extract JSON from potentially markdown-wrapped response
fn extract_json(text: &str) -> String {
    let trimmed = text.trim();

    // Try to find JSON block
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            return trimmed[start..=end].to_string();
        }
    }

    // Try markdown code block
    if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 3 {
            let content: String = lines[1..lines.len() - 1].join("\n");
            if let Some(start) = content.find('{') {
                if let Some(end) = content.rfind('}') {
                    return content[start..=end].to_string();
                }
            }
        }
    }

    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_raw() {
        let input = r#"{"passed": true, "score": 85, "reason": "Good"}"#;
        let result = extract_json(input);
        assert!(result.contains("passed"));
    }

    #[test]
    fn test_extract_json_with_text() {
        let input = r#"Here is my evaluation:
{"passed": true, "score": 85, "reason": "Good", "suggestion": null}
That's my verdict."#;
        let result = extract_json(input);
        assert!(result.starts_with('{'));
        assert!(result.ends_with('}'));
    }

    #[test]
    fn test_extract_json_markdown() {
        let input = r#"```json
{"passed": false, "score": 40, "reason": "Issues found"}
```"#;
        let result = extract_json(input);
        assert!(result.contains("passed"));
    }
}
```

**Step 2: Verify compilation**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -5`

**Step 3: Commit**

```bash
git add core/src/poe/validation/semantic.rs
git commit -m "poe: implement semantic validation with LLM judge

Add SemanticValidator that:
- Resolves JudgeTarget (File/Content/CommandOutput)
- Calls LLM with POE-specific system prompt
- Supports different ModelTier think levels
- Parses structured JSON verdict

Reuses existing AiProvider infrastructure."
```

---

## Task 6: Implement Composite Validator (`validation/composite.rs`)

**Files:**
- Create: `core/src/poe/validation/composite.rs`

**Step 1: Write composite.rs**

Create `core/src/poe/validation/composite.rs`:

```rust
//! Composite validator that orchestrates hard and semantic validation.
//!
//! Implements the two-phase validation pipeline:
//! 1. Hard validation first (fast fail)
//! 2. Semantic validation only if hard passes

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::error::Result;
use crate::providers::AiProvider;
use crate::poe::types::{
    RuleResult, SoftRuleResult, SuccessManifest, ValidationRule, Verdict, WorkerOutput,
};

use super::hard::HardValidator;
use super::semantic::SemanticValidator;

/// Composite validator combining hard and semantic checks
pub struct CompositeValidator {
    hard_validator: HardValidator,
    semantic_validator: SemanticValidator,
}

impl CompositeValidator {
    /// Create a new composite validator with the given AI provider
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            hard_validator: HardValidator::new(),
            semantic_validator: SemanticValidator::new(provider),
        }
    }

    /// Validate worker output against the success manifest
    pub async fn validate(
        &self,
        manifest: &SuccessManifest,
        _output: &WorkerOutput,
    ) -> Result<Verdict> {
        info!(task_id = %manifest.task_id, "Starting validation");

        // Phase 1: Hard validation (fast fail)
        let hard_results = self
            .hard_validator
            .validate_all(&manifest.hard_constraints)
            .await?;

        let hard_failures: Vec<&RuleResult> = hard_results
            .iter()
            .filter(|r| !r.passed)
            .collect();

        if !hard_failures.is_empty() {
            let reason = self.summarize_hard_failures(&hard_failures);
            let suggestion = self.suggest_hard_fix(&hard_failures);

            warn!(
                task_id = %manifest.task_id,
                failures = hard_failures.len(),
                "Hard validation failed"
            );

            return Ok(Verdict {
                passed: false,
                distance_score: 1.0, // Complete failure
                reason,
                suggestion: Some(suggestion),
                hard_results,
                soft_results: Vec::new(),
            });
        }

        debug!(
            task_id = %manifest.task_id,
            "Hard validation passed, proceeding to semantic"
        );

        // Phase 2: Semantic validation (only if hard passes)
        let soft_results = if manifest.soft_metrics.is_empty() {
            Vec::new()
        } else {
            self.semantic_validator
                .validate_all(&manifest.soft_metrics)
                .await?
        };

        // Calculate weighted score
        let (weighted_score, all_above_threshold) = self.calculate_soft_score(&soft_results);
        let distance_score = 1.0 - weighted_score;

        let passed = all_above_threshold;
        let reason = if passed {
            "All validations passed".to_string()
        } else {
            self.summarize_soft_failures(&soft_results)
        };
        let suggestion = if passed {
            None
        } else {
            Some(self.suggest_soft_fix(&soft_results))
        };

        info!(
            task_id = %manifest.task_id,
            passed,
            distance_score,
            "Validation complete"
        );

        Ok(Verdict {
            passed,
            distance_score,
            reason,
            suggestion,
            hard_results,
            soft_results,
        })
    }

    /// Summarize hard validation failures
    fn summarize_hard_failures(&self, failures: &[&RuleResult]) -> String {
        let messages: Vec<String> = failures
            .iter()
            .filter_map(|r| r.error.clone())
            .take(3) // Limit to 3 errors
            .collect();

        if messages.is_empty() {
            "Hard validation failed".to_string()
        } else {
            format!("Hard validation failed:\n- {}", messages.join("\n- "))
        }
    }

    /// Suggest fix for hard validation failures
    fn suggest_hard_fix(&self, failures: &[&RuleResult]) -> String {
        let suggestions: Vec<String> = failures
            .iter()
            .map(|r| match &r.rule {
                ValidationRule::FileExists { path } => {
                    format!("Create file: {}", path.display())
                }
                ValidationRule::FileNotExists { path } => {
                    format!("Delete file: {}", path.display())
                }
                ValidationRule::FileContains { path, pattern } => {
                    format!("Add '{}' to {}", pattern, path.display())
                }
                ValidationRule::FileNotContains { path, pattern } => {
                    format!("Remove '{}' from {}", pattern, path.display())
                }
                ValidationRule::CommandPasses { cmd, args, .. } => {
                    format!("Fix issues causing '{}' to fail", cmd)
                }
                ValidationRule::CommandOutputContains { cmd, pattern, .. } => {
                    format!("Ensure '{}' output contains '{}'", cmd, pattern)
                }
                ValidationRule::JsonSchemaValid { path, .. } => {
                    format!("Fix JSON structure in {}", path.display())
                }
                ValidationRule::DirStructureMatch { root, .. } => {
                    format!("Fix directory structure under {}", root.display())
                }
                ValidationRule::SemanticCheck { .. } => {
                    "Review semantic requirements".to_string()
                }
            })
            .take(3)
            .collect();

        suggestions.join("; ")
    }

    /// Calculate weighted score from soft results
    fn calculate_soft_score(&self, results: &[SoftRuleResult]) -> (f32, bool) {
        if results.is_empty() {
            return (1.0, true);
        }

        let total_weight: f32 = results.iter().map(|r| r.metric.weight).sum();
        if total_weight == 0.0 {
            return (1.0, true);
        }

        let weighted_sum: f32 = results
            .iter()
            .map(|r| r.score * r.metric.weight)
            .sum();

        let weighted_score = weighted_sum / total_weight;

        let all_above_threshold = results
            .iter()
            .all(|r| r.score >= r.metric.threshold);

        (weighted_score, all_above_threshold)
    }

    /// Summarize soft metric failures
    fn summarize_soft_failures(&self, results: &[SoftRuleResult]) -> String {
        let failures: Vec<String> = results
            .iter()
            .filter(|r| r.score < r.metric.threshold)
            .filter_map(|r| r.feedback.clone())
            .take(3)
            .collect();

        if failures.is_empty() {
            "Soft metrics below threshold".to_string()
        } else {
            format!("Quality issues:\n- {}", failures.join("\n- "))
        }
    }

    /// Suggest fix for soft metric failures
    fn suggest_soft_fix(&self, results: &[SoftRuleResult]) -> String {
        let suggestions: Vec<String> = results
            .iter()
            .filter(|r| r.score < r.metric.threshold)
            .filter_map(|r| {
                if let ValidationRule::SemanticCheck { prompt, .. } = &r.metric.rule {
                    Some(format!("Improve: {}", prompt.chars().take(50).collect::<String>()))
                } else {
                    None
                }
            })
            .take(3)
            .collect();

        if suggestions.is_empty() {
            "Review and improve quality".to_string()
        } else {
            suggestions.join("; ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::SoftMetric;

    #[test]
    fn test_calculate_soft_score_empty() {
        let validator = CompositeValidator {
            hard_validator: HardValidator::new(),
            semantic_validator: SemanticValidator::new(Arc::new(MockProvider)),
        };

        let (score, passed) = validator.calculate_soft_score(&[]);
        assert_eq!(score, 1.0);
        assert!(passed);
    }

    #[test]
    fn test_calculate_soft_score_weighted() {
        let validator = CompositeValidator {
            hard_validator: HardValidator::new(),
            semantic_validator: SemanticValidator::new(Arc::new(MockProvider)),
        };

        let results = vec![
            SoftRuleResult {
                metric: SoftMetric {
                    rule: ValidationRule::FileExists { path: "a".into() },
                    weight: 0.5,
                    threshold: 0.8,
                },
                score: 0.9,
                feedback: None,
            },
            SoftRuleResult {
                metric: SoftMetric {
                    rule: ValidationRule::FileExists { path: "b".into() },
                    weight: 0.5,
                    threshold: 0.8,
                },
                score: 0.7,
                feedback: None,
            },
        ];

        let (score, passed) = validator.calculate_soft_score(&results);
        assert!((score - 0.8).abs() < 0.01);
        assert!(!passed); // Second result below threshold
    }

    // Mock provider for tests
    struct MockProvider;

    #[async_trait::async_trait]
    impl AiProvider for MockProvider {
        fn name(&self) -> &str { "mock" }
        fn default_model(&self) -> &str { "mock" }
        fn supports_thinking(&self) -> bool { false }

        async fn process(&self, _prompt: &str, _system: Option<&str>) -> Result<String> {
            Ok(r#"{"passed": true, "score": 85, "reason": "Good"}"#.to_string())
        }

        async fn process_with_thinking(
            &self,
            prompt: &str,
            system: Option<&str>,
            _level: ThinkLevel,
        ) -> Result<String> {
            self.process(prompt, system).await
        }
    }
}
```

**Step 2: Verify compilation**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -5`

**Step 3: Commit**

```bash
git add core/src/poe/validation/composite.rs
git commit -m "poe: implement composite validator

Add CompositeValidator that:
- Runs hard validation first (fast fail)
- Only runs semantic validation if hard passes
- Calculates weighted soft scores
- Generates actionable fix suggestions
- Computes distance_score for entropy tracking"
```

---

## Task 7: Implement Worker Abstraction (`worker.rs`)

**Files:**
- Create: `core/src/poe/worker.rs`

**Step 1: Write worker.rs**

Create `core/src/poe/worker.rs`:

```rust
//! Worker abstraction for POE execution.
//!
//! Workers execute the actual operations. The POE Manager
//! treats them as black boxes that take instructions and
//! produce outputs.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::poe::types::{Artifact, ChangeType, StepLog, WorkerOutput, WorkerState};

/// Worker trait for POE execution
///
/// Workers are responsible for executing instructions and
/// producing outputs that can be validated against the manifest.
#[async_trait]
pub trait Worker: Send + Sync {
    /// Execute an instruction, optionally with feedback from previous failure
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput>;

    /// Abort the current execution
    async fn abort(&self) -> Result<()>;

    /// Take a snapshot of current state (for rollback)
    async fn snapshot(&self) -> Result<StateSnapshot>;

    /// Restore from a previous snapshot
    async fn restore(&self, snapshot: &StateSnapshot) -> Result<()>;
}

/// State snapshot for rollback
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub timestamp: DateTime<Utc>,
    pub workspace: PathBuf,
    pub file_hashes: Vec<(PathBuf, String)>,
}

/// Placeholder AgentLoop worker
///
/// TODO: This will wrap the actual AgentLoop once we integrate.
/// For now, it's a stub that demonstrates the interface.
pub struct AgentLoopWorker {
    workspace: PathBuf,
}

impl AgentLoopWorker {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Worker for AgentLoopWorker {
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        // Build enhanced instruction with failure context
        let enhanced = match previous_failure {
            Some(failure) => format!(
                "{}\n\n⚠️ Previous attempt failed: {}",
                instruction, failure
            ),
            None => instruction.to_string(),
        };

        // TODO: Actually run AgentLoop here
        // For now, return a placeholder output

        Ok(WorkerOutput {
            tokens_consumed: 0,
            steps_taken: 0,
            final_state: WorkerState::Completed {
                summary: format!("Executed: {}", enhanced.chars().take(50).collect::<String>()),
            },
            artifacts: Vec::new(),
            execution_log: vec![StepLog {
                step_id: 1,
                action: "placeholder".into(),
                result: "stub implementation".into(),
                duration_ms: 0,
            }],
        })
    }

    async fn abort(&self) -> Result<()> {
        // TODO: Implement abort
        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        Ok(StateSnapshot {
            timestamp: Utc::now(),
            workspace: self.workspace.clone(),
            file_hashes: Vec::new(), // TODO: Hash files
        })
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> Result<()> {
        // TODO: Implement restore
        Ok(())
    }
}

/// Simple test worker for unit tests
#[cfg(test)]
pub struct MockWorker {
    pub should_succeed: bool,
    pub tokens_per_call: u32,
}

#[cfg(test)]
#[async_trait]
impl Worker for MockWorker {
    async fn execute(
        &self,
        _instruction: &str,
        _previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        Ok(WorkerOutput {
            tokens_consumed: self.tokens_per_call,
            steps_taken: 1,
            final_state: if self.should_succeed {
                WorkerState::Completed {
                    summary: "Mock success".into(),
                }
            } else {
                WorkerState::Failed {
                    reason: "Mock failure".into(),
                }
            },
            artifacts: Vec::new(),
            execution_log: Vec::new(),
        })
    }

    async fn abort(&self) -> Result<()> {
        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        Ok(StateSnapshot {
            timestamp: Utc::now(),
            workspace: PathBuf::from("/tmp"),
            file_hashes: Vec::new(),
        })
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> Result<()> {
        Ok(())
    }
}
```

**Step 2: Verify compilation**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -5`

**Step 3: Commit**

```bash
git add core/src/poe/worker.rs
git commit -m "poe: implement worker abstraction

Add Worker trait and AgentLoopWorker:
- execute() with previous failure injection
- abort() for cancellation
- snapshot()/restore() for rollback
- MockWorker for testing

AgentLoopWorker is a placeholder for now."
```

---

## Task 8: Implement POE Manager (`manager.rs`)

**Files:**
- Create: `core/src/poe/manager.rs`

**Step 1: Write manager.rs**

Create `core/src/poe/manager.rs`:

```rust
//! POE Manager - the orchestrator of Principle-Operation-Evaluation cycles.
//!
//! This is the core control loop that:
//! 1. Manages the execution budget
//! 2. Orchestrates Worker execution
//! 3. Validates results
//! 4. Decides on retry, strategy switch, or completion

use std::sync::Arc;

use tracing::{debug, info, warn, error};

use crate::error::Result;
use crate::poe::budget::{PoeBudget, BudgetStatus};
use crate::poe::types::{PoeOutcome, PoeTask, SuccessManifest, Verdict, WorkerOutput};
use crate::poe::validation::CompositeValidator;
use crate::poe::worker::Worker;

/// POE Manager configuration
#[derive(Debug, Clone)]
pub struct PoeConfig {
    /// Window size for stuck detection
    pub stuck_window: usize,
    /// Maximum tokens per execution
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

/// POE Manager - orchestrates the P→O→E cycle
pub struct PoeManager<W: Worker> {
    worker: W,
    validator: CompositeValidator,
    config: PoeConfig,
}

impl<W: Worker> PoeManager<W> {
    /// Create a new POE Manager
    pub fn new(worker: W, validator: CompositeValidator, config: PoeConfig) -> Self {
        Self {
            worker,
            validator,
            config,
        }
    }

    /// Execute a POE task
    pub async fn execute(&self, task: PoeTask) -> Result<PoeOutcome> {
        let mut budget = PoeBudget::new(
            task.manifest.max_attempts,
            self.config.max_tokens,
        );
        let mut instruction = task.instruction.clone();
        let mut last_failure: Option<String> = None;

        info!(
            task_id = %task.manifest.task_id,
            objective = %task.manifest.objective,
            max_attempts = task.manifest.max_attempts,
            "Starting POE execution"
        );

        while !budget.exhausted() {
            let attempt = budget.current_attempt + 1;
            info!(
                task_id = %task.manifest.task_id,
                attempt,
                remaining = budget.remaining_attempts(),
                "Starting attempt"
            );

            // === O: Operation ===
            let output = self.worker
                .execute(&instruction, last_failure.as_deref())
                .await?;

            // === E: Evaluation ===
            let verdict = self.validator
                .validate(&task.manifest, &output)
                .await?;

            // Record attempt
            budget.record_attempt(output.tokens_consumed, verdict.distance_score);

            debug!(
                task_id = %task.manifest.task_id,
                attempt,
                passed = verdict.passed,
                distance = verdict.distance_score,
                "Validation complete"
            );

            if verdict.passed {
                info!(
                    task_id = %task.manifest.task_id,
                    attempts = attempt,
                    tokens = budget.tokens_used,
                    "POE execution succeeded"
                );
                return Ok(PoeOutcome::Success(verdict));
            }

            // Check if stuck
            if budget.is_stuck(self.config.stuck_window) {
                warn!(
                    task_id = %task.manifest.task_id,
                    attempts = attempt,
                    "Stuck in local optimum, suggesting strategy switch"
                );

                return Ok(PoeOutcome::StrategySwitch {
                    reason: format!(
                        "No progress after {} attempts. Last error: {}",
                        attempt,
                        verdict.reason
                    ),
                    suggestion: verdict.suggestion.unwrap_or_else(|| {
                        "Consider a completely different approach".to_string()
                    }),
                });
            }

            // Prepare for retry
            instruction = self.build_retry_prompt(&task, &verdict);
            last_failure = Some(verdict.reason);

            debug!(
                task_id = %task.manifest.task_id,
                "Prepared retry prompt"
            );
        }

        // Budget exhausted
        error!(
            task_id = %task.manifest.task_id,
            attempts = budget.current_attempt,
            tokens = budget.tokens_used,
            "Budget exhausted"
        );

        Ok(PoeOutcome::BudgetExhausted {
            attempts: budget.current_attempt,
            last_error: last_failure.unwrap_or_else(|| "Unknown error".to_string()),
        })
    }

    /// Build retry prompt with failure context
    fn build_retry_prompt(&self, task: &PoeTask, verdict: &Verdict) -> String {
        format!(
            "## Previous Attempt Failed\n\
             **Reason**: {}\n\
             **Suggestion**: {}\n\n\
             ## Original Objective\n\
             {}\n\n\
             ## Your Task\n\
             Fix the issue and try again. Do NOT repeat the same approach that failed.",
            verdict.reason,
            verdict.suggestion.as_deref().unwrap_or("None"),
            task.manifest.objective
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::types::{SuccessManifest, ValidationRule};
    use crate::poe::worker::MockWorker;
    use crate::providers::AiProvider;
    use crate::agents::thinking::ThinkLevel;

    // Mock provider for tests
    struct MockProvider;

    #[async_trait::async_trait]
    impl AiProvider for MockProvider {
        fn name(&self) -> &str { "mock" }
        fn default_model(&self) -> &str { "mock" }
        fn supports_thinking(&self) -> bool { false }

        async fn process(&self, _prompt: &str, _system: Option<&str>) -> Result<String> {
            Ok(r#"{"passed": true, "score": 85, "reason": "Good"}"#.to_string())
        }

        async fn process_with_thinking(
            &self,
            prompt: &str,
            system: Option<&str>,
            _level: ThinkLevel,
        ) -> Result<String> {
            self.process(prompt, system).await
        }
    }

    #[tokio::test]
    async fn test_poe_manager_success_on_first_try() {
        // Create a manifest with no hard constraints (always passes)
        let manifest = SuccessManifest::new("test-1", "Test objective");

        let worker = MockWorker {
            should_succeed: true,
            tokens_per_call: 1000,
        };

        let validator = CompositeValidator::new(Arc::new(MockProvider));
        let manager = PoeManager::new(worker, validator, PoeConfig::default());

        let task = PoeTask::new(manifest, "Do something");
        let outcome = manager.execute(task).await.unwrap();

        match outcome {
            PoeOutcome::Success(_) => {}
            _ => panic!("Expected success"),
        }
    }

    #[tokio::test]
    async fn test_poe_manager_budget_exhausted() {
        // Create a manifest with impossible constraint
        let manifest = SuccessManifest::new("test-2", "Test objective")
            .with_hard_constraint(ValidationRule::FileExists {
                path: "/nonexistent/file/that/doesnt/exist".into(),
            })
            .with_max_attempts(2);

        let worker = MockWorker {
            should_succeed: true,
            tokens_per_call: 1000,
        };

        let validator = CompositeValidator::new(Arc::new(MockProvider));
        let manager = PoeManager::new(worker, validator, PoeConfig::default());

        let task = PoeTask::new(manifest, "Do something");
        let outcome = manager.execute(task).await.unwrap();

        match outcome {
            PoeOutcome::BudgetExhausted { attempts, .. } => {
                assert_eq!(attempts, 2);
            }
            _ => panic!("Expected budget exhausted"),
        }
    }
}
```

**Step 2: Verify compilation**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -5`

**Step 3: Run tests**

Run: `cargo test -p alephcore poe:: 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/poe/manager.rs
git commit -m "poe: implement POE manager

Add PoeManager that orchestrates P→O→E cycle:
- Manages PoeBudget for attempt/token tracking
- Calls Worker.execute() with failure injection
- Validates with CompositeValidator
- Handles Success, StrategySwitch, BudgetExhausted
- Builds retry prompts with context

Includes integration tests."
```

---

## Task 9: Fix Module Exports and Final Integration

**Files:**
- Modify: `core/src/poe/mod.rs`
- Verify: `core/src/lib.rs`

**Step 1: Update mod.rs with correct exports**

Update `core/src/poe/mod.rs`:

```rust
//! POE (Principle-Operation-Evaluation) Architecture
//!
//! A goal-oriented agent execution framework that:
//! 1. **Principle**: Defines success criteria before execution (SuccessManifest)
//! 2. **Operation**: Executes with heuristic guidance (Worker abstraction)
//! 3. **Evaluation**: Validates results with mixed hard/semantic checks
//!
//! # Example
//!
//! ```ignore
//! use alephcore::poe::{PoeManager, PoeTask, SuccessManifest, ValidationRule};
//!
//! // Define success criteria
//! let manifest = SuccessManifest::new("task-1", "Add user authentication")
//!     .with_hard_constraint(ValidationRule::CommandPasses {
//!         cmd: "cargo".into(),
//!         args: vec!["test".into()],
//!         timeout_ms: 60_000,
//!     })
//!     .with_max_attempts(5);
//!
//! // Create task
//! let task = PoeTask::new(manifest, "Implement JWT-based auth");
//!
//! // Execute with POE Manager
//! let outcome = manager.execute(task).await?;
//! ```

pub mod budget;
pub mod manager;
pub mod types;
pub mod validation;
pub mod worker;

// Re-exports for convenience
pub use budget::{PoeBudget, BudgetStatus};
pub use manager::{PoeManager, PoeConfig};
pub use types::{
    Artifact, ChangeType, Experience, ExperienceOutcome, JudgeTarget, ModelTier,
    PoeOutcome, PoeTask, RuleResult, SoftMetric, SoftRuleResult, SolutionPath,
    StepLog, SuccessManifest, TaskPattern, ValidationRule, Verdict, WorkerOutput,
    WorkerState,
};
pub use validation::{CompositeValidator, HardValidator, SemanticValidator};
pub use worker::{AgentLoopWorker, StateSnapshot, Worker};
```

**Step 2: Verify lib.rs has the module**

Ensure `core/src/lib.rs` contains:
```rust
pub mod poe;
```

**Step 3: Full compilation check**

Run: `cargo build -p alephcore 2>&1 | tail -20`
Expected: Successful build with only warnings

**Step 4: Run all POE tests**

Run: `cargo test -p alephcore poe 2>&1 | tail -30`
Expected: All tests pass

**Step 5: Commit**

```bash
git add core/src/poe/mod.rs core/src/lib.rs
git commit -m "poe: finalize module exports and integration

Complete POE module with:
- All types re-exported for convenience
- Documentation with usage example
- Module properly registered in lib.rs

POE architecture implementation complete."
```

---

## Task 10: Create Integration Test

**Files:**
- Create: `core/src/poe/tests.rs` or add to existing test file

**Step 1: Add integration test**

Add to `core/src/poe/mod.rs`:

```rust
#[cfg(test)]
mod tests;
```

Create `core/src/poe/tests.rs`:

```rust
//! Integration tests for POE module

use std::sync::Arc;
use tempfile::tempdir;

use crate::poe::{
    AgentLoopWorker, CompositeValidator, PoeConfig, PoeManager, PoeOutcome,
    PoeTask, SuccessManifest, ValidationRule,
};
use crate::providers::AiProvider;
use crate::agents::thinking::ThinkLevel;
use crate::error::Result;

// Mock provider for integration tests
struct MockProvider;

#[async_trait::async_trait]
impl AiProvider for MockProvider {
    fn name(&self) -> &str { "mock" }
    fn default_model(&self) -> &str { "mock" }
    fn supports_thinking(&self) -> bool { false }

    async fn process(&self, _prompt: &str, _system: Option<&str>) -> Result<String> {
        Ok(r#"{"passed": true, "score": 90, "reason": "Good quality"}"#.to_string())
    }

    async fn process_with_thinking(
        &self,
        prompt: &str,
        system: Option<&str>,
        _level: ThinkLevel,
    ) -> Result<String> {
        self.process(prompt, system).await
    }
}

#[tokio::test]
async fn test_full_poe_cycle_with_file_constraint() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("output.txt");

    // Create manifest requiring the file to exist
    let manifest = SuccessManifest::new("integration-1", "Create output file")
        .with_hard_constraint(ValidationRule::FileExists {
            path: file_path.clone(),
        })
        .with_max_attempts(3);

    // Worker that doesn't actually create the file
    let worker = AgentLoopWorker::new(dir.path().to_path_buf());
    let validator = CompositeValidator::new(Arc::new(MockProvider));
    let manager = PoeManager::new(worker, validator, PoeConfig::default());

    let task = PoeTask::new(manifest, "Create the output file");
    let outcome = manager.execute(task).await.unwrap();

    // Should exhaust budget since file is never created
    match outcome {
        PoeOutcome::BudgetExhausted { attempts, .. } => {
            assert_eq!(attempts, 3);
        }
        other => panic!("Expected BudgetExhausted, got {:?}", other),
    }
}

#[tokio::test]
async fn test_poe_succeeds_when_constraints_met() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("existing.txt");

    // Pre-create the file
    tokio::fs::write(&file_path, "content").await.unwrap();

    // Create manifest that should pass
    let manifest = SuccessManifest::new("integration-2", "Verify existing file")
        .with_hard_constraint(ValidationRule::FileExists {
            path: file_path.clone(),
        })
        .with_hard_constraint(ValidationRule::FileContains {
            path: file_path,
            pattern: "content".into(),
        })
        .with_max_attempts(1);

    let worker = AgentLoopWorker::new(dir.path().to_path_buf());
    let validator = CompositeValidator::new(Arc::new(MockProvider));
    let manager = PoeManager::new(worker, validator, PoeConfig::default());

    let task = PoeTask::new(manifest, "Verify the file");
    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
            assert_eq!(verdict.distance_score, 0.0);
        }
        other => panic!("Expected Success, got {:?}", other),
    }
}

#[tokio::test]
async fn test_poe_command_validation() {
    let manifest = SuccessManifest::new("integration-3", "Run echo command")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "echo".into(),
            args: vec!["hello".into()],
            timeout_ms: 5000,
        })
        .with_hard_constraint(ValidationRule::CommandOutputContains {
            cmd: "echo".into(),
            args: vec!["world".into()],
            pattern: "world".into(),
            timeout_ms: 5000,
        })
        .with_max_attempts(1);

    let worker = AgentLoopWorker::new(std::env::temp_dir());
    let validator = CompositeValidator::new(Arc::new(MockProvider));
    let manager = PoeManager::new(worker, validator, PoeConfig::default());

    let task = PoeTask::new(manifest, "Run commands");
    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(_) => {}
        other => panic!("Expected Success, got {:?}", other),
    }
}
```

**Step 2: Run integration tests**

Run: `cargo test -p alephcore poe::tests 2>&1 | tail -30`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/poe/tests.rs core/src/poe/mod.rs
git commit -m "poe: add integration tests

Add comprehensive integration tests:
- test_full_poe_cycle_with_file_constraint
- test_poe_succeeds_when_constraints_met
- test_poe_command_validation

All tests verify the full P→O→E cycle."
```

---

## Summary

This plan implements the POE architecture in 10 tasks:

| Task | Component | Description |
|------|-----------|-------------|
| 1 | Module Structure | Scaffold `poe/` directory |
| 2 | types.rs | Core data structures |
| 3 | budget.rs | Entropy-based budget manager |
| 4 | validation/hard.rs | Deterministic validation |
| 5 | validation/semantic.rs | LLM judge validation |
| 6 | validation/composite.rs | Two-phase validation pipeline |
| 7 | worker.rs | Worker abstraction |
| 8 | manager.rs | POE orchestrator |
| 9 | mod.rs | Final exports |
| 10 | tests.rs | Integration tests |

Each task follows TDD where applicable and includes a commit.

---

**Plan complete and saved to `docs/plans/2026-02-01-poe-implementation-plan.md`.**

**Two execution options:**

1. **Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

2. **Parallel Session (separate)** - Open new session in worktree with executing-plans skill, batch execution with checkpoints

**Which approach?**
