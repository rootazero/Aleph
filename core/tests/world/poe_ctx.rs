//! POE test context

use tempfile::TempDir;
use std::path::PathBuf;

/// Outcome type for POE tests
#[derive(Debug, Clone)]
pub enum PoeOutcomeType {
    /// Successful completion with verdict
    Success {
        passed: bool,
        hard_results_count: usize,
    },
    /// Budget exhausted after max attempts
    BudgetExhausted {
        attempts: u8,
    },
    /// Strategy switch due to stuck detection
    StrategySwitch {
        reason: String,
    },
}

/// POE test context
/// Stores state for BDD scenario execution
#[derive(Default)]
pub struct PoeContext {
    // ═══ Temp Directory ═══
    /// Temporary directory for file operations
    pub temp_dir: Option<TempDir>,

    // ═══ Task Configuration ═══
    /// Files to create in the temp directory
    pub files_to_create: Vec<(String, String)>,
    /// Directories to create in the temp directory
    pub dirs_to_create: Vec<String>,
    /// Hard constraints for the task
    pub hard_constraints: Vec<PoeConstraint>,
    /// Max attempts for the task
    pub max_attempts: Option<u8>,
    /// Stuck window for the config
    pub stuck_window: Option<usize>,
    /// Max tokens for the config
    pub max_tokens: Option<u32>,
    /// Tokens consumed per worker call
    pub tokens_per_call: Option<u32>,

    // ═══ Results ═══
    /// Outcome of the POE task
    pub outcome: Option<PoeOutcomeType>,
    /// Number of attempts made
    pub attempts: Option<u8>,
    /// Number of times worker was called
    pub worker_call_count: Option<u32>,

    // ═══ Budget State (for direct budget tests) ═══
    /// Budget for direct budget tests
    pub budget: Option<alephcore::poe::PoeBudget>,
    /// Whether the budget is stuck
    pub budget_is_stuck: Option<bool>,
    /// Best score from the budget
    pub budget_best_score: Option<f32>,
}

impl std::fmt::Debug for PoeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoeContext")
            .field("temp_dir", &self.temp_dir.as_ref().map(|d| d.path()))
            .field("files_to_create_count", &self.files_to_create.len())
            .field("dirs_to_create_count", &self.dirs_to_create.len())
            .field("hard_constraints_count", &self.hard_constraints.len())
            .field("max_attempts", &self.max_attempts)
            .field("stuck_window", &self.stuck_window)
            .field("max_tokens", &self.max_tokens)
            .field("tokens_per_call", &self.tokens_per_call)
            .field("outcome", &self.outcome)
            .field("attempts", &self.attempts)
            .field("worker_call_count", &self.worker_call_count)
            .field("budget", &self.budget.is_some())
            .field("budget_is_stuck", &self.budget_is_stuck)
            .field("budget_best_score", &self.budget_best_score)
            .finish()
    }
}

/// POE constraint representation for tests
#[derive(Debug, Clone)]
pub enum PoeConstraint {
    /// File must exist
    FileExists { path: String },
    /// File must contain pattern
    FileContains { path: String, pattern: String },
    /// File must not contain pattern
    FileNotContains { path: String, pattern: String },
    /// Directory structure must match
    DirStructureMatch { expected: String },
    /// JSON must match schema
    JsonSchemaValid { path: String, schema: String },
    /// Command must pass
    CommandPasses { cmd: String, args: Vec<String> },
    /// Command output must contain pattern
    CommandOutputContains { cmd: String, args: Vec<String>, pattern: String },
    /// Impossible constraint (for failure testing)
    Impossible,
}

impl PoeContext {
    /// Create a new context with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize the temporary directory
    pub fn init_temp_dir(&mut self) -> &std::path::Path {
        if self.temp_dir.is_none() {
            self.temp_dir = Some(tempfile::tempdir().expect("Failed to create temp dir"));
        }
        self.temp_dir.as_ref().unwrap().path()
    }

    /// Get the temp directory path
    pub fn temp_path(&self) -> PathBuf {
        self.temp_dir
            .as_ref()
            .map(|d| d.path().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }

    /// Add a file to create
    pub fn add_file(&mut self, name: String, content: String) {
        self.files_to_create.push((name, content));
    }

    /// Add a directory to create
    pub fn add_dir(&mut self, name: String) {
        self.dirs_to_create.push(name);
    }

    /// Add a hard constraint
    pub fn add_constraint(&mut self, constraint: PoeConstraint) {
        self.hard_constraints.push(constraint);
    }
}
