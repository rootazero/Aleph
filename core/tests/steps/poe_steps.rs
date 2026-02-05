//! Step definitions for POE (Principle-Operation-Evaluation) features

use crate::world::{AlephWorld, PoeContext, PoeConstraint, PoeOutcomeType};
use alephcore::Result;
use alephcore::poe::{
    CompositeValidator, PoeBudget, PoeConfig, PoeManager, PoeOutcome, PoeTask,
    SuccessManifest, ValidationRule, Worker,
};
use alephcore::poe::worker::StateSnapshot;
use alephcore::poe::WorkerOutput;
use alephcore::providers::MockProvider;
use async_trait::async_trait;
use cucumber::{given, then, when};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tempfile::tempdir;
use tokio::fs;

// ═══════════════════════════════════════════════════════════════════════════
// Test Worker Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Test worker for BDD tests
/// Simulates execution with configurable token consumption
pub struct TestWorker {
    /// Tokens to report per execute() call
    tokens_per_call: u32,
    /// Counter for number of executions
    execution_count: AtomicU32,
    /// Workspace for snapshots
    workspace: PathBuf,
}

impl TestWorker {
    /// Create a new TestWorker with specified tokens per call
    pub fn new(tokens_per_call: u32) -> Self {
        Self {
            tokens_per_call,
            execution_count: AtomicU32::new(0),
            workspace: PathBuf::from("/tmp/test-workspace"),
        }
    }

    /// Get the number of times execute() has been called
    pub fn execution_count(&self) -> u32 {
        self.execution_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Worker for TestWorker {
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        self.execution_count.fetch_add(1, Ordering::SeqCst);

        let mut output = WorkerOutput::completed(format!(
            "Test execution of: {}{}",
            instruction,
            previous_failure
                .map(|f| format!(" (retry after: {})", f))
                .unwrap_or_default()
        ));
        output.tokens_consumed = self.tokens_per_call;
        output.steps_taken = 1;
        Ok(output)
    }

    async fn abort(&self) -> Result<()> {
        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        Ok(StateSnapshot::new(self.workspace.clone()))
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> Result<()> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════

/// Create a PoeManager for testing
fn create_test_manager(
    tokens_per_call: u32,
    config: PoeConfig,
) -> PoeManager<TestWorker> {
    let worker = TestWorker::new(tokens_per_call);
    let provider = Arc::new(MockProvider::new(""));
    let validator = CompositeValidator::new(provider);
    PoeManager::new(worker, validator, config)
}

/// Convert PoeConstraint to ValidationRule
fn constraint_to_rule(constraint: &PoeConstraint, base_path: &std::path::Path) -> ValidationRule {
    match constraint {
        PoeConstraint::FileExists { path } => ValidationRule::FileExists {
            path: base_path.join(path),
        },
        PoeConstraint::FileContains { path, pattern } => ValidationRule::FileContains {
            path: base_path.join(path),
            pattern: pattern.clone(),
        },
        PoeConstraint::FileNotContains { path, pattern } => ValidationRule::FileNotContains {
            path: base_path.join(path),
            pattern: pattern.clone(),
        },
        PoeConstraint::DirStructureMatch { expected } => ValidationRule::DirStructureMatch {
            root: base_path.to_path_buf(),
            expected: expected.clone(),
        },
        PoeConstraint::JsonSchemaValid { path, schema } => ValidationRule::JsonSchemaValid {
            path: base_path.join(path),
            schema: schema.clone(),
        },
        PoeConstraint::CommandPasses { cmd, args } => ValidationRule::CommandPasses {
            cmd: cmd.clone(),
            args: args.clone(),
            timeout_ms: 5000,
        },
        PoeConstraint::CommandOutputContains { cmd, args, pattern } => {
            ValidationRule::CommandOutputContains {
                cmd: cmd.clone(),
                args: args.clone(),
                pattern: pattern.clone(),
                timeout_ms: 5000,
            }
        }
        PoeConstraint::Impossible => ValidationRule::FileExists {
            path: PathBuf::from("/nonexistent/impossible/path.txt"),
        },
    }
}

/// Parse a simple schema string like "name:string, version:string" into JSON Schema
fn parse_simple_schema(schema_str: &str) -> String {
    let mut props = Vec::new();
    for part in schema_str.split(',') {
        let part = part.trim();
        if let Some((name, typ)) = part.split_once(':') {
            props.push(format!(r#""{name}": "{typ}""#, name = name.trim(), typ = typ.trim()));
        }
    }
    format!("{{{}}}", props.join(", "))
}

// ═══════════════════════════════════════════════════════════════════════════
// Given Steps
// ═══════════════════════════════════════════════════════════════════════════

// Note: "a temporary directory" step is defined in common.rs
// We initialize POE temp_dir from w.temp_dir in the execution step

#[given(expr = "a file {string} with content {string}")]
async fn given_file_with_content(w: &mut AlephWorld, filename: String, content: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    // Just record the file to create - actual creation happens in "When I execute the POE task"
    ctx.add_file(filename, content);
}

#[given(expr = "directories {string} and {string} exist")]
async fn given_directories_exist(w: &mut AlephWorld, dir1: String, dir2: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    // Just record the directories to create - actual creation happens in "When I execute the POE task"
    ctx.add_dir(dir1);
    ctx.add_dir(dir2);
}

#[given(expr = "a POE task requiring file {string} to exist")]
async fn given_poe_task_file_exists(w: &mut AlephWorld, filename: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::FileExists { path: filename });
}

#[given(expr = "a POE task requiring file {string} to exist with max {int} attempt(s)")]
async fn given_poe_task_file_exists_max_attempts(w: &mut AlephWorld, filename: String, max_attempts: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::FileExists { path: filename });
    ctx.max_attempts = Some(max_attempts as u8);
}

#[given(expr = "a POE task requiring files {string} and {string} to exist")]
async fn given_poe_task_multiple_files(w: &mut AlephWorld, file1: String, file2: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::FileExists { path: file1 });
    ctx.add_constraint(PoeConstraint::FileExists { path: file2 });
}

#[given(expr = "a POE task requiring file {string} to contain pattern {string}")]
async fn given_poe_task_file_contains(w: &mut AlephWorld, filename: String, pattern: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::FileContains { path: filename, pattern });
}

#[given(expr = "a POE task requiring file {string} to not contain pattern {string}")]
async fn given_poe_task_file_not_contains(w: &mut AlephWorld, filename: String, pattern: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::FileNotContains { path: filename, pattern });
}

#[given(expr = "a POE task requiring directory structure {string}")]
async fn given_poe_task_dir_structure(w: &mut AlephWorld, expected: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::DirStructureMatch { expected });
}

#[given(expr = "a POE task requiring file {string} to match schema with fields {string}")]
async fn given_poe_task_json_schema(w: &mut AlephWorld, filename: String, fields: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    let schema = parse_simple_schema(&fields);
    ctx.add_constraint(PoeConstraint::JsonSchemaValid { path: filename, schema });
}

#[given(expr = "a POE task requiring command {string} to pass")]
async fn given_poe_task_command_passes(w: &mut AlephWorld, command: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    let parts: Vec<&str> = command.split_whitespace().collect();
    let cmd = parts.first().unwrap_or(&"").to_string();
    let args: Vec<String> = parts.iter().skip(1).map(|s| s.to_string()).collect();
    ctx.add_constraint(PoeConstraint::CommandPasses { cmd, args });
}

#[given(expr = "a POE task requiring command {string} to pass with max {int} attempts")]
async fn given_poe_task_command_passes_max(w: &mut AlephWorld, command: String, max_attempts: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    let parts: Vec<&str> = command.split_whitespace().collect();
    let cmd = parts.first().unwrap_or(&"").to_string();
    let args: Vec<String> = parts.iter().skip(1).map(|s| s.to_string()).collect();
    ctx.add_constraint(PoeConstraint::CommandPasses { cmd, args });
    ctx.max_attempts = Some(max_attempts as u8);
}

#[given(expr = "a POE task requiring command {string} output to contain {string}")]
async fn given_poe_task_command_output_contains(w: &mut AlephWorld, command: String, pattern: String) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    let parts: Vec<&str> = command.split_whitespace().collect();
    let cmd = parts.first().unwrap_or(&"").to_string();
    let args: Vec<String> = parts.iter().skip(1).map(|s| s.to_string()).collect();
    ctx.add_constraint(PoeConstraint::CommandOutputContains { cmd, args, pattern });
}

#[given(expr = "a POE task requiring file {string} and {string} to both exist with max {int} attempts")]
async fn given_poe_task_both_files_max(w: &mut AlephWorld, file1: String, file2: String, max_attempts: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::FileExists { path: file1 });
    ctx.add_constraint(PoeConstraint::FileExists { path: file2 });
    ctx.max_attempts = Some(max_attempts as u8);
}

#[given(expr = "a POE task with impossible constraint and max {int} attempts")]
async fn given_poe_task_impossible_max(w: &mut AlephWorld, max_attempts: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::Impossible);
    ctx.max_attempts = Some(max_attempts as u8);
}

#[given("a POE task with impossible constraint")]
async fn given_poe_task_impossible(w: &mut AlephWorld) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::Impossible);
}

#[given(expr = "a POE task with impossible constraint and stuck window {int}")]
async fn given_poe_task_impossible_stuck_window(w: &mut AlephWorld, window: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.add_constraint(PoeConstraint::Impossible);
    ctx.stuck_window = Some(window as usize);
}

#[given("a POE task with no constraints")]
async fn given_poe_task_no_constraints(w: &mut AlephWorld) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    // No constraints added
}

#[given(expr = "stuck window of {int}")]
async fn given_stuck_window(w: &mut AlephWorld, window: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.stuck_window = Some(window as usize);
}

#[given(expr = "worker consuming {int} tokens per call")]
async fn given_worker_tokens(w: &mut AlephWorld, tokens: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.tokens_per_call = Some(tokens as u32);
}

#[given(expr = "token budget of {int}")]
async fn given_token_budget(w: &mut AlephWorld, tokens: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.max_tokens = Some(tokens as u32);
}

#[given(expr = "max {int} attempts")]
async fn given_max_attempts(w: &mut AlephWorld, max_attempts: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.max_attempts = Some(max_attempts as u8);
}

#[given(expr = "a POE budget with max {int} attempts")]
async fn given_poe_budget(w: &mut AlephWorld, max_attempts: i32) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    ctx.budget = Some(PoeBudget::new(max_attempts as u8, 100_000));
}

// ═══════════════════════════════════════════════════════════════════════════
// When Steps
// ═══════════════════════════════════════════════════════════════════════════

#[when("I execute the POE task")]
async fn when_execute_poe_task(w: &mut AlephWorld) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);

    // Use POE context temp_dir if available, otherwise use world temp_dir
    let base_path = if ctx.temp_dir.is_some() {
        ctx.temp_path()
    } else if let Some(ref temp_dir) = w.temp_dir {
        temp_dir.path().to_path_buf()
    } else {
        PathBuf::from("/tmp")
    };
    for (name, content) in &ctx.files_to_create {
        let file_path = base_path.join(name);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await.ok();
        }
        fs::write(&file_path, content).await.expect("Failed to write file");
    }
    for dir_name in &ctx.dirs_to_create {
        fs::create_dir_all(base_path.join(dir_name)).await.expect("Failed to create dir");
    }

    // Build manifest
    let mut manifest = SuccessManifest::new("test-task", "Test task");
    for constraint in &ctx.hard_constraints {
        let rule = constraint_to_rule(constraint, &base_path);
        manifest = manifest.with_hard_constraint(rule);
    }
    if let Some(max) = ctx.max_attempts {
        manifest = manifest.with_max_attempts(max);
    }

    let task = PoeTask::new(manifest.clone(), "Execute the test task");

    // Build config
    let mut config = PoeConfig::default();
    if let Some(window) = ctx.stuck_window {
        config = config.with_stuck_window(window);
    }
    if let Some(tokens) = ctx.max_tokens {
        config = config.with_max_tokens(tokens);
    }

    // Create manager and execute
    let tokens_per_call = ctx.tokens_per_call.unwrap_or(100);
    let manager = create_test_manager(tokens_per_call, config);
    let outcome = manager.execute(task).await.expect("POE execution failed");

    // Store results
    ctx.worker_call_count = Some(manager.worker().execution_count());

    match outcome {
        PoeOutcome::Success(verdict) => {
            ctx.outcome = Some(PoeOutcomeType::Success {
                passed: verdict.passed,
                hard_results_count: verdict.hard_results.len(),
            });
        }
        PoeOutcome::BudgetExhausted { attempts, .. } => {
            ctx.outcome = Some(PoeOutcomeType::BudgetExhausted { attempts });
            ctx.attempts = Some(attempts);
        }
        PoeOutcome::StrategySwitch { reason, .. } => {
            ctx.outcome = Some(PoeOutcomeType::StrategySwitch { reason });
        }
    }
}

#[when(expr = "I record {int} attempts with same distance {float}")]
async fn when_record_same_distance(w: &mut AlephWorld, count: i32, distance: f64) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    let budget = ctx.budget.as_mut().expect("Budget not initialized");
    for _ in 0..count {
        budget.record_attempt(1000, distance as f32);
    }
}

#[when(expr = "I record attempts with decreasing distances {float}, {float}, {float}, {float}")]
async fn when_record_decreasing_distances(w: &mut AlephWorld, d1: f64, d2: f64, d3: f64, d4: f64) {
    let ctx = w.poe.get_or_insert_with(PoeContext::new);
    let budget = ctx.budget.as_mut().expect("Budget not initialized");
    budget.record_attempt(1000, d1 as f32);
    budget.record_attempt(1000, d2 as f32);
    budget.record_attempt(1000, d3 as f32);
    budget.record_attempt(1000, d4 as f32);
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps
// ═══════════════════════════════════════════════════════════════════════════

#[then("the outcome should be Success")]
async fn then_outcome_success(w: &mut AlephWorld) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    match &ctx.outcome {
        Some(PoeOutcomeType::Success { .. }) => {}
        other => panic!("Expected Success outcome, got {:?}", other),
    }
}

#[then("all hard constraints should pass")]
async fn then_all_hard_constraints_pass(w: &mut AlephWorld) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    match &ctx.outcome {
        Some(PoeOutcomeType::Success { passed, .. }) => {
            assert!(*passed, "Expected all hard constraints to pass");
        }
        other => panic!("Expected Success outcome, got {:?}", other),
    }
}

#[then("the outcome should be BudgetExhausted or StrategySwitch")]
async fn then_outcome_budget_or_strategy(w: &mut AlephWorld) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    match &ctx.outcome {
        Some(PoeOutcomeType::BudgetExhausted { .. }) => {}
        Some(PoeOutcomeType::StrategySwitch { .. }) => {}
        other => panic!("Expected BudgetExhausted or StrategySwitch outcome, got {:?}", other),
    }
}

#[then("the outcome should be BudgetExhausted")]
async fn then_outcome_budget_exhausted(w: &mut AlephWorld) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    match &ctx.outcome {
        Some(PoeOutcomeType::BudgetExhausted { .. }) => {}
        other => panic!("Expected BudgetExhausted outcome, got {:?}", other),
    }
}

#[then("the outcome should be StrategySwitch")]
async fn then_outcome_strategy_switch(w: &mut AlephWorld) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    match &ctx.outcome {
        Some(PoeOutcomeType::StrategySwitch { .. }) => {}
        other => panic!("Expected StrategySwitch outcome, got {:?}", other),
    }
}

#[then(expr = "attempts should be {int}")]
async fn then_attempts_should_be(w: &mut AlephWorld, expected: i32) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    match &ctx.outcome {
        Some(PoeOutcomeType::BudgetExhausted { attempts }) => {
            assert_eq!(*attempts, expected as u8, "Attempts mismatch");
        }
        other => panic!("Expected BudgetExhausted outcome, got {:?}", other),
    }
}

#[then(expr = "worker should be called {int} time(s)")]
async fn then_worker_called_times(w: &mut AlephWorld, expected: i32) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    let actual = ctx.worker_call_count.expect("Worker call count not set");
    assert_eq!(actual, expected as u32, "Worker call count mismatch: expected {}, got {}", expected, actual);
}

#[then(expr = "the budget should be stuck over window {int}")]
async fn then_budget_stuck(w: &mut AlephWorld, window: i32) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    let budget = ctx.budget.as_ref().expect("Budget not initialized");
    assert!(
        budget.is_stuck(window as usize),
        "Expected budget to be stuck over window {}",
        window
    );
}

#[then("the budget should not be stuck")]
async fn then_budget_not_stuck(w: &mut AlephWorld) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    let budget = ctx.budget.as_ref().expect("Budget not initialized");
    assert!(
        !budget.is_stuck(3),
        "Expected budget to not be stuck"
    );
}

#[then(expr = "best score should be approximately {float}")]
async fn then_best_score_approx(w: &mut AlephWorld, expected: f64) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    let budget = ctx.budget.as_ref().expect("Budget not initialized");
    let best = budget.best_score().expect("No best score");
    assert!(
        (best - expected as f32).abs() < 0.05,
        "Best score mismatch: expected approximately {}, got {}",
        expected,
        best
    );
}

#[then(expr = "switch reason should contain {string}")]
async fn then_switch_reason_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    match &ctx.outcome {
        Some(PoeOutcomeType::StrategySwitch { reason }) => {
            assert!(
                reason.contains(&expected),
                "Switch reason '{}' does not contain '{}'",
                reason,
                expected
            );
        }
        other => panic!("Expected StrategySwitch outcome, got {:?}", other),
    }
}
