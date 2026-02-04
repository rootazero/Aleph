//! BDD Test World - Shared state between cucumber steps

use cucumber::World;
use tempfile::TempDir;

/// Main World struct for all BDD tests
/// Each module context is lazily initialized via Option<T>
#[derive(Debug, Default, World)]
pub struct AlephWorld {
    // ═══ Common State ═══
    /// Temporary directory for test isolation
    pub temp_dir: Option<TempDir>,
    /// Last operation result (for Then assertions)
    pub last_result: Option<Result<(), String>>,
    /// Last error message captured
    pub last_error: Option<String>,

    // ═══ Module Contexts (added as batches are implemented) ═══
    // pub config: Option<ConfigContext>,
    // pub scripting: Option<ScriptingContext>,
}
