//! BDD Test World - Shared state between cucumber steps

use cucumber::World;
use tempfile::TempDir;
use rhai::{Engine, AST};

mod agent_loop_ctx;
mod config_ctx;
mod daemon_ctx;
mod dispatcher_ctx;
mod e2e_ctx;
mod extension_ctx;
mod gateway_ctx;
mod logging_ctx;
mod memory_ctx;
mod message_builder_ctx;
mod models_ctx;
mod poe_ctx;
mod protocol_ctx;
mod scheduler_ctx;
mod security_ctx;
mod skills_ctx;
mod subagent_ctx;
mod thinker_ctx;
mod tools_ctx;

pub use agent_loop_ctx::{AgentLoopContext, MockDecision};
pub use config_ctx::ConfigContext;
pub use daemon_ctx::DaemonContext;
pub use dispatcher_ctx::DispatcherContext;
pub use e2e_ctx::{E2eContext, BatchLoadResult};
pub use extension_ctx::ExtensionContext;
pub use gateway_ctx::{GatewayContext, TrackingExecutionAdapter, TestEmitter, make_test_message};
pub use logging_ctx::LoggingContext;
pub use memory_ctx::MemoryContext;
pub use message_builder_ctx::MessageBuilderContext;
pub use models_ctx::ModelsContext;
pub use poe_ctx::{PoeContext, PoeConstraint, PoeOutcomeType};
pub use protocol_ctx::ProtocolContext;
pub use scheduler_ctx::SchedulerContext;
pub use security_ctx::{SecurityContext, SkillExecutionResult};
pub use skills_ctx::SkillsContext;
pub use subagent_ctx::SubagentContext;
pub use thinker_ctx::ThinkerContext;
pub use tools_ctx::ToolsContext;

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
    pub e2e: Option<E2eContext>,
    pub gateway: Option<GatewayContext>,
    pub logging: Option<LoggingContext>,
    pub memory: Option<MemoryContext>,
    pub message_builder: Option<MessageBuilderContext>,
    pub agent_loop: Option<AgentLoopContext>,
    pub extension: Option<ExtensionContext>,
    pub poe: Option<PoeContext>,
    pub scheduler: Option<SchedulerContext>,
    pub security: Option<SecurityContext>,
    pub thinker: Option<ThinkerContext>,
    pub tools: Option<ToolsContext>,
    pub models: Option<ModelsContext>,
    pub protocol: Option<ProtocolContext>,
    pub skills: Option<SkillsContext>,
    pub subagent: Option<SubagentContext>,
}
