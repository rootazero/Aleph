//! BDD Test World - Shared state between cucumber steps

use cucumber::World;
use tempfile::TempDir;
use rhai::{Engine, AST};

mod agent_loop_ctx;
mod config_ctx;
mod daemon_ctx;
mod dispatcher_ctx;
mod gateway_ctx;
mod memory_ctx;
mod message_builder_ctx;
mod perception_ctx;
mod poe_ctx;
mod thinker_ctx;

pub use agent_loop_ctx::{AgentLoopContext, MockDecision};
pub use config_ctx::ConfigContext;
pub use daemon_ctx::DaemonContext;
pub use dispatcher_ctx::DispatcherContext;
pub use gateway_ctx::{GatewayContext, TrackingExecutionAdapter, TestEmitter, make_test_message};
pub use memory_ctx::MemoryContext;
pub use message_builder_ctx::MessageBuilderContext;
pub use perception_ctx::PerceptionContext;
pub use poe_ctx::{PoeContext, PoeConstraint, PoeOutcomeType};
pub use thinker_ctx::ThinkerContext;

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
    pub dispatcher: Option<DispatcherContext>,
    pub gateway: Option<GatewayContext>,
    pub memory: Option<MemoryContext>,
    pub message_builder: Option<MessageBuilderContext>,
    pub perception: Option<PerceptionContext>,
    pub agent_loop: Option<AgentLoopContext>,
    pub poe: Option<PoeContext>,
    pub thinker: Option<ThinkerContext>,
}
