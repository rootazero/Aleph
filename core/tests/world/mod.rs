//! BDD Test World - Shared state between cucumber steps

use cucumber::World;
use tempfile::TempDir;
use rhai::{Engine, AST};

mod config_ctx;
mod daemon_ctx;

pub use config_ctx::ConfigContext;
pub use daemon_ctx::DaemonContext;

/// Scripting engine test context
#[derive(Debug, Default)]
pub struct ScriptingContext {
    pub engine: Option<Engine>,
    pub compile_result: Option<Result<AST, String>>,
    pub eval_result: Option<Result<i64, String>>,
}

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

    // ═══ Module Contexts ═══
    pub scripting: Option<ScriptingContext>,
    pub config: Option<ConfigContext>,
    pub daemon: Option<DaemonContext>,
}
