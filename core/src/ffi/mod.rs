//! FFI module - Unified FFI interface for AetherCore
//!
//! This module provides the FFI interface for native clients (Swift, Kotlin, etc.)
//! It is split into submodules for better maintainability:
//!
//! - `processing`: AI processing methods (process, cancel, etc.)
//! - `tools`: Tool management (list_tools, add_mcp_tool, etc.)
//! - `memory`: Memory operations (search_memory, clear_memory, etc.)
//! - `config`: Configuration management (reload_config, update_provider, etc.)
//! - `skills`: Skills management (list_skills, install_skill, etc.)
//! - `mcp`: MCP server management (list_mcp_servers, add_mcp_server, etc.)
//! - `dispatcher`: Dispatcher FFI methods (task orchestration, model routing, etc.)
//! - `dispatcher_types`: Dispatcher FFI types (enums, structs for UniFFI)
//! - `generation`: Media generation (generate_image, generate_speech, etc.)
//! - `session`: Session lifecycle management (resume, cancel, list)
//! - `typo_correction`: Quick typo correction (correct_typo)

mod agent_loop_adapter;
#[cfg(feature = "uniffi")]
mod async_extension;
mod config;
mod dag_executor;
mod dispatcher;
pub mod dispatcher_types;
mod generation;
#[cfg(feature = "uniffi")]
mod init;
mod mcp;
mod memory;
pub mod plan_confirmation;
mod plugins;
mod processing;
// Note: processing is now a directory module (processing/mod.rs)
mod prompt_helpers;
mod provider_factory;
mod runtime;
mod session;
mod skills;
pub mod tool_discovery;
mod tools;
mod typo_correction;
mod user_input;

// Agent Loop FFI adapter for new architecture
pub use agent_loop_adapter::FfiLoopCallback;

use crate::agents::rig::tools::{create_builtin_tool_server, create_builtin_tools_list, BuiltinToolConfig};
use crate::agents::rig::ChatMessage;
use crate::agents::RigAgentConfig;
use crate::config::Config;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::memory::MemoryEntry;
use crate::tools::AetherToolServerHandle;
use crate::utils::paths::get_memory_db_path;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

// Re-export public types
pub use self::generation::{
    GenerationDataFFI, GenerationDataTypeFFI, GenerationMetadataFFI, GenerationOutputFFI,
    GenerationParamsFFI, GenerationProgressFFI, GenerationProviderConfigFFI,
    GenerationProviderInfoFFI, GenerationTypeFFI,
};
#[cfg(feature = "uniffi")]
pub use self::init::{
    needs_first_time_init, run_initialization, InitProgressHandlerFFI, InitResultFFI,
};
pub use self::processing::ProcessOptions;
pub use self::plugins::{PluginInfoFFI, PluginSkillFFI};
pub use self::runtime::{RuntimeInfo, RuntimeUpdateInfo};
pub use self::session::SessionSummary;
pub use self::typo_correction::TypoCorrectionResult;

/// Error type for FFI boundary
///
/// This error type is designed to be FFI-friendly.
/// UniFFI Error enums must use simple variants with message support via Display trait.
#[derive(Debug, thiserror::Error)]
pub enum AetherFfiError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Provider error: {0}")]
    Provider(String),
    #[error("Tool error: {0}")]
    Tool(String),
    #[error("Memory error: {0}")]
    Memory(String),
    #[error("Operation cancelled")]
    Cancelled,
}

impl From<crate::error::AetherError> for AetherFfiError {
    fn from(e: crate::error::AetherError) -> Self {
        AetherFfiError::Config(e.to_string())
    }
}

/// Event handler callback interface
///
/// Clients implement this trait to receive callbacks during AI processing.
/// All methods take `&self` for thread-safe callback invocation.
pub trait AetherEventHandler: Send + Sync {
    /// Called when AI starts processing (thinking)
    fn on_thinking(&self);

    /// Called when a tool execution starts
    fn on_tool_start(&self, tool_name: String);

    /// Called when a tool execution completes
    fn on_tool_result(&self, tool_name: String, result: String);

    /// Called for each streaming chunk of the response
    fn on_stream_chunk(&self, text: String);

    /// Called when processing completes with the full response
    fn on_complete(&self, response: String);

    /// Called when an error occurs
    fn on_error(&self, message: String);

    /// Called when a memory entry is stored
    fn on_memory_stored(&self);

    /// Called when agent execution mode is detected
    ///
    /// This callback notifies the UI that the input has been classified
    /// as an executable task and agent mode will be activated.
    fn on_agent_mode_detected(&self, task: crate::intent::ExecutableTaskFFI);

    // ========================================================================
    // HOT-RELOAD CALLBACKS
    // ========================================================================

    /// Called when tool registry is updated (MCP server added/removed, skill installed/deleted)
    ///
    /// This callback notifies the UI that the tool list has changed and may need refreshing.
    /// The tool_count parameter indicates the new total number of registered tools.
    fn on_tools_changed(&self, tool_count: u32);

    /// Called when MCP servers have finished starting
    ///
    /// This callback provides a report of which servers started successfully and which failed.
    fn on_mcp_startup_complete(&self, report: crate::event_handler::McpStartupReportFFI);

    /// Called when runtime updates are available (Phase 7)
    ///
    /// This callback notifies the UI that one or more runtimes have updates available.
    /// Called once at startup after background update check completes.
    fn on_runtime_updates_available(&self, updates: Vec<runtime::RuntimeUpdateInfo>);

    // ========================================================================
    // AGENTIC LOOP CALLBACKS (Phase 5)
    // ========================================================================

    /// Called when a new session is created
    fn on_session_started(&self, session_id: String);

    /// Called when tool execution starts (with call_id for tracking)
    fn on_tool_call_started(&self, call_id: String, tool_name: String);

    /// Called when tool execution completes
    fn on_tool_call_completed(&self, call_id: String, output: String);

    /// Called when tool execution fails
    fn on_tool_call_failed(&self, call_id: String, error: String, is_retryable: bool);

    /// Called on each loop iteration with progress update
    fn on_loop_progress(&self, session_id: String, iteration: u32, status: String);

    /// Called when a plan is created for multi-step task
    fn on_plan_created(&self, session_id: String, steps: Vec<String>);

    /// Called when session completes
    fn on_session_completed(&self, session_id: String, summary: String);

    /// Called when sub-agent is started
    fn on_subagent_started(
        &self,
        parent_session_id: String,
        child_session_id: String,
        agent_id: String,
    );

    /// Called when sub-agent completes
    fn on_subagent_completed(&self, child_session_id: String, success: bool, summary: String);

    /// Called when session is compacted (context reduced via summarization)
    ///
    /// This callback notifies the UI that a session's context has been compacted
    /// to prevent token overflow. The UI may display this information to the user
    /// or update any token usage displays.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session that was compacted
    /// * `tokens_before` - Token count before compaction
    /// * `tokens_after` - Token count after compaction
    fn on_session_compacted(&self, session_id: String, tokens_before: u64, tokens_after: u64) {
        // Default: no-op
        // UI implementations should override to display compaction notification
        let _ = (session_id, tokens_before, tokens_after);
    }

    // ========================================================================
    // UNIFIED PLANNER CALLBACKS (Phase 10)
    // ========================================================================

    /// Called when user confirmation is required before executing a task
    ///
    /// This callback allows the UI to prompt the user for confirmation before
    /// executing potentially destructive or multi-step operations.
    ///
    /// # Arguments
    ///
    /// * `message` - Description of what will be executed
    ///
    /// # Default Implementation
    ///
    /// Default implementation does nothing (auto-confirm). Override to implement
    /// confirmation UI.
    fn on_confirmation_required(&self, _message: String) {
        // Default: auto-confirm (no-op)
        // UI implementations should override to show confirmation dialog
    }

    // ========================================================================
    // DAG PLAN CONFIRMATION CALLBACKS
    // ========================================================================

    /// Called when a DAG task plan requires user confirmation
    ///
    /// This callback is triggered when the DAG scheduler detects high-risk
    /// tasks in the execution plan. The UI should display the plan and
    /// prompt the user for confirmation.
    ///
    /// After receiving this callback, Swift should:
    /// 1. Display the DagTaskPlan to the user
    /// 2. Wait for user to click "Confirm" or "Cancel"
    /// 3. Call `AetherCore.confirm_task_plan(plan_id, decision)` with the decision
    ///
    /// # Arguments
    ///
    /// * `plan_id` - Unique identifier for this confirmation request
    /// * `plan` - The task plan that needs confirmation
    fn on_plan_confirmation_required(&self, plan_id: String, plan: crate::dispatcher::DagTaskPlan);

    // ========================================================================
    // USER INPUT CALLBACKS (Agent Loop Interactive Input)
    // ========================================================================

    /// Called when the agent loop needs user input
    ///
    /// This callback is triggered when the LLM requests user input during
    /// agent execution (e.g., ask_user action). The UI should display the
    /// question and optionally present choices to the user.
    ///
    /// After receiving this callback, Swift should:
    /// 1. Display the question to the user
    /// 2. If options are provided, show them as choices
    /// 3. Wait for user to type response or select option
    /// 4. Call `AetherCore.respond_to_user_input(request_id, response)` with the response
    ///
    /// # Arguments
    ///
    /// * `request_id` - Unique identifier for this input request
    /// * `question` - The question to ask the user
    /// * `options` - Optional list of choices (empty if free-form input)
    fn on_user_input_request(&self, request_id: String, question: String, options: Vec<String>) {
        // Default: no-op (auto-respond with empty string)
        // UI implementations should override to show input dialog
        let _ = (request_id, question, options);
    }

    // ========================================================================
    // MESSAGE FLOW CALLBACKS (Part Update Events)
    // ========================================================================

    /// Called when a session part is added, updated, or removed
    ///
    /// This callback enables real-time message flow rendering in the UI:
    /// - Tool calls with status transitions (Pending → Running → Completed/Failed)
    /// - Streaming AI responses via delta field
    /// - Sub-agent progress display
    ///
    /// # Arguments
    ///
    /// * `event` - Part update event containing all rendering information
    fn on_part_update(&self, event: PartUpdateEventFFI) {
        // Default: no-op
        // UI implementations should override to render message flow
        let _ = event;
    }
}

/// Tool information for UI display
#[derive(Debug, Clone)]
pub struct ToolInfoFFI {
    /// Tool name/identifier
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Tool source (builtin, mcp, skill, etc.)
    pub source: String,
}

// =============================================================================
// Part Update Event Types (for message flow rendering)
// =============================================================================

/// Part event type for FFI (matches PartEventTypeFFI in aether.udl)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartEventTypeFFI {
    /// Part was added to the session
    Added,
    /// Part was updated (e.g., tool call status changed, streaming text)
    Updated,
    /// Part was removed (e.g., compaction)
    Removed,
}

impl From<crate::components::PartEventType> for PartEventTypeFFI {
    fn from(event_type: crate::components::PartEventType) -> Self {
        match event_type {
            crate::components::PartEventType::Added => Self::Added,
            crate::components::PartEventType::Updated => Self::Updated,
            crate::components::PartEventType::Removed => Self::Removed,
        }
    }
}

/// Part update event for FFI (matches PartUpdateEventFFI in aether.udl)
///
/// Contains all information needed for the UI to render message flow updates.
#[derive(Debug, Clone)]
pub struct PartUpdateEventFFI {
    /// Session ID this part belongs to
    pub session_id: String,
    /// Unique part identifier
    pub part_id: String,
    /// Part type name (e.g., "tool_call", "ai_response")
    pub part_type: String,
    /// Event type (Added, Updated, Removed)
    pub event_type: PartEventTypeFFI,
    /// Serialized part data as JSON
    pub part_json: String,
    /// Delta content for streaming updates (text chunks)
    pub delta: Option<String>,
    /// Timestamp when the event occurred
    pub timestamp: i64,
}

impl From<&crate::components::PartUpdateData> for PartUpdateEventFFI {
    fn from(data: &crate::components::PartUpdateData) -> Self {
        Self {
            session_id: data.session_id.clone(),
            part_id: data.part_id.clone(),
            part_type: data.part_type.clone(),
            event_type: data.event_type.into(),
            part_json: data.part_json.clone(),
            delta: data.delta.clone(),
            timestamp: data.timestamp,
        }
    }
}

impl ToolInfoFFI {
    /// Create a new tool info
    pub fn new(name: String, description: String, source: String) -> Self {
        Self {
            name,
            description,
            source,
        }
    }
}

/// Memory item for UI display
#[derive(Debug, Clone)]
pub struct MemoryItem {
    /// Unique identifier
    pub id: String,
    /// User's input text
    pub user_input: String,
    /// AI's response text
    pub assistant_response: String,
    /// Unix timestamp
    pub timestamp: i64,
    /// Application context when memory was created
    pub app_context: Option<String>,
}

impl From<MemoryEntry> for MemoryItem {
    fn from(entry: MemoryEntry) -> Self {
        Self {
            id: entry.id,
            user_input: entry.user_input,
            assistant_response: entry.ai_output,
            timestamp: entry.context.timestamp,
            app_context: Some(entry.context.app_bundle_id),
        }
    }
}

/// Agent configuration holder for thread-safe access
///
/// Since RigAgentManager may contain non-Send types (via MemoryStore),
/// we store only the config and create managers on-demand.
pub(crate) struct AgentConfigHolder {
    config: RigAgentConfig,
}

impl AgentConfigHolder {
    pub(crate) fn new(config: RigAgentConfig) -> Self {
        Self { config }
    }

    pub(crate) fn config(&self) -> &RigAgentConfig {
        &self.config
    }
}

/// Core implementation
///
/// This struct provides the main interface for the architecture.
/// It manages the configuration and provides methods for processing,
/// tool management, and memory operations.
///
/// # Hot-Reload Support
///
/// Tools are managed through a shared `ToolServerHandle`, enabling:
/// - Runtime addition of MCP tools when servers connect
/// - Runtime removal of tools when servers disconnect
/// - All tools persist across `process()` calls
pub struct AetherCore {
    /// Configuration holder with interior mutability for reload support
    pub(crate) config_holder: Arc<RwLock<AgentConfigHolder>>,
    /// Full configuration with interior mutability for Settings UI operations
    pub(crate) full_config: Arc<Mutex<Config>>,
    /// Config file path for reload capability (empty string means default path)
    pub(crate) config_path: String,
    /// Memory database path for lazy initialization (enables on-demand creation)
    /// Wrapped in RwLock to support dynamic enable/disable of memory feature
    pub(crate) memory_path: Arc<RwLock<Option<String>>>,
    pub(crate) handler: Arc<dyn AetherEventHandler>,
    /// Tokio runtime handle for async operations
    pub(crate) runtime: tokio::runtime::Handle,
    /// Owned runtime to keep it alive (when we create our own)
    /// This MUST be stored to prevent the runtime from being dropped
    pub(crate) _owned_runtime: Option<tokio::runtime::Runtime>,
    /// Current operation's cancellation token
    /// Each new operation gets a fresh token, allowing cancellation state to be reset
    pub(crate) current_op_token: Arc<RwLock<CancellationToken>>,
    /// Shared AetherToolServerHandle for hot-reload support
    /// This handle is shared across all tool server instances
    pub(crate) tool_server_handle: AetherToolServerHandle,
    /// Names of registered tools (for tracking and display)
    pub(crate) registered_tools: Arc<RwLock<Vec<String>>>,
    /// Agent engine for task orchestration (lazily initialized)
    pub(crate) agent_engine: Arc<RwLock<Option<Arc<crate::dispatcher::AgentEngine>>>>,
    /// Conversation histories keyed by topic_id for multi-turn support
    pub(crate) conversation_histories: Arc<RwLock<HashMap<String, Vec<ChatMessage>>>>,
    /// Generation provider registry for media generation (image, speech, etc.)
    pub(crate) generation_registry: Arc<RwLock<crate::generation::GenerationProviderRegistry>>,
}

impl AetherCore {
    /// Acquires the full config mutex lock with poison recovery.
    #[inline(always)]
    pub(crate) fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
        self.full_config.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in full_config, recovering");
            e.into_inner()
        })
    }

    // ========================================================================
    // HOT-RELOAD SUPPORT
    // ========================================================================

    /// Notify UI that tool registry has changed
    ///
    /// This should be called after any operation that modifies the tool registry:
    /// - MCP server add/update/delete
    /// - Skill install/delete
    /// - Custom command changes
    pub(crate) fn notify_tools_changed(&self) {
        let tool_count = self
            .registered_tools
            .read()
            .map(|tools| tools.len() as u32)
            .unwrap_or(0);

        info!(
            tool_count = tool_count,
            "Notifying UI of tool registry change"
        );
        self.handler.on_tools_changed(tool_count);
    }

    /// Get current tool count
    pub fn get_tool_count(&self) -> u32 {
        self.registered_tools
            .read()
            .map(|tools| tools.len() as u32)
            .unwrap_or(0)
    }
}

/// Initialize AetherCore
///
/// Creates a new AetherCore instance with the given configuration path
/// and event handler.
///
/// # Arguments
///
/// * `config_path` - Path to the configuration file (empty string uses default path)
/// * `handler` - Event handler for callbacks
///
/// # Returns
///
/// Returns an Arc-wrapped AetherCore on success, or an error if
/// initialization fails.
///
/// # Config Loading Behavior
///
/// - If `config_path` is empty: Load from default path (~/.aether/config.toml)
/// - If `config_path` is provided and file exists: Load from that path
/// - If `config_path` is provided but file doesn't exist: Use defaults with info log
/// - If config file exists but has parse errors: Return `AetherFfiError::Config`
pub fn init_core(
    config_path: String,
    handler: Box<dyn AetherEventHandler>,
) -> Result<Arc<AetherCore>, AetherFfiError> {
    // Initialize logging system first (file + console with PII scrubbing)
    // Must be called before any tracing macros to ensure logs are captured
    crate::init_logging();

    info!(config_path = %config_path, "Initializing AetherCore");

    // Convert Box to Arc for internal use
    let handler: Arc<dyn AetherEventHandler> = Arc::from(handler);

    // Get or create runtime
    // IMPORTANT: If we create our own runtime, we MUST store it to keep it alive
    let (runtime, owned_runtime) = match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // Already in async context, use existing runtime
            (handle, None)
        }
        Err(_) => {
            // Not in async context, create our own runtime
            let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
            let handle = rt.handle().clone();
            (handle, Some(rt))
        }
    };

    // Load config from file
    let full_config = if config_path.is_empty() {
        // Use default path (~/.aether/config.toml)
        Config::load().map_err(|e| AetherFfiError::Config(e.to_string()))?
    } else {
        let path = Path::new(&config_path);
        if path.exists() {
            Config::load_from_file(path).map_err(|e| AetherFfiError::Config(e.to_string()))?
        } else {
            info!(path = %config_path, "Config file not found, using defaults");
            Config::default()
        }
    };

    // Extract provider settings from loaded config
    // provider_name is the config key (e.g., "t8star"), provider_type is the protocol (e.g., "openai")
    let (provider_name_for_log, provider, model, api_key, base_url, system_prompt, temperature, max_tokens, timeout_seconds) = {
        let default_provider = full_config.get_default_provider();
        if let Some(ref name) = default_provider {
            if let Some(provider_config) = full_config.providers.get(name) {
                let provider_type = provider_config.infer_provider_type(name);
                (
                    Some(name.clone()),
                    provider_type,
                    provider_config.model.clone(),
                    provider_config.api_key.clone(),
                    provider_config.base_url.clone(),
                    None::<String>, // Provider-level system_prompt not in ProviderConfig
                    provider_config.temperature,
                    provider_config.max_tokens,
                    provider_config.timeout_seconds,
                )
            } else {
                // Default provider name exists but config not found
                info!(provider = %name, "Default provider config not found, using defaults");
                (
                    None,
                    "openai".to_string(),
                    "gpt-4o".to_string(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    30u64, // Default timeout
                )
            }
        } else {
            // No default provider configured
            info!("No default provider configured, using openai defaults");
            (
                None,
                "openai".to_string(),
                "gpt-4o".to_string(),
                None,
                None,
                None,
                None,
                None,
                30u64, // Default timeout
            )
        }
    };

    // Create RigAgentConfig with loaded values
    let rig_config = RigAgentConfig {
        provider_name: provider_name_for_log.clone(),
        provider,
        model,
        temperature: temperature.unwrap_or(0.7),
        max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        max_turns: 50, // Default to 50 turns for complex multi-step tasks
        timeout_seconds, // Use timeout from provider config
        system_prompt: system_prompt
            .unwrap_or_else(|| "You are Aether, an intelligent assistant.".to_string()),
        api_key,
        base_url,
    };

    // Log with both provider_name (config key) and provider_type (protocol) for clarity
    info!(
        provider_name = provider_name_for_log.as_deref().unwrap_or("(default)"),
        provider_type = %rig_config.provider,
        model = %rig_config.model,
        has_api_key = rig_config.api_key.is_some(),
        base_url = rig_config.base_url.as_deref().unwrap_or("(default)"),
        "RigAgentConfig loaded from config file"
    );

    // Wrap config holder in Arc<RwLock> for reload support
    let config_holder = Arc::new(RwLock::new(AgentConfigHolder::new(rig_config)));

    // Set up memory store path if memory is enabled
    // Wrapped in Arc<RwLock> to support dynamic enable/disable of memory feature
    // Uses unified path: ~/.aether/memory.db (cross-platform)
    let memory_path = Arc::new(RwLock::new(if full_config.memory.enabled {
        match get_memory_db_path() {
            Ok(db_path) => {
                info!(path = %db_path.display(), "Memory store enabled");
                Some(db_path.to_string_lossy().to_string())
            }
            Err(e) => {
                warn!(error = %e, "Failed to get memory db path, memory disabled");
                None
            }
        }
    } else {
        info!("Memory store disabled in config");
        None
    }));

    // Create initial cancellation token wrapped in Arc<RwLock> for interior mutability
    // Each operation will get a fresh token via reset_cancel_token()
    let current_op_token = Arc::new(RwLock::new(CancellationToken::new()));

    // Initialize generation provider registry from config FIRST
    // (needed for BuiltinToolConfig to include ImageGenerateTool)
    let generation_registry = generation::init_generation_providers(&full_config);

    // Extract search tool configuration from config file
    let builtin_tool_config = {
        let tavily_api_key = if let Some(ref search_config) = full_config.search {
            if search_config.enabled {
                // Get Tavily API key if configured
                let key = search_config
                    .backends
                    .get("tavily")
                    .and_then(|backend| backend.api_key.clone())
                    .filter(|key| !key.is_empty());

                if key.is_some() {
                    info!("Tavily API key found in config file");
                }
                key
            } else {
                None
            }
        } else {
            None
        };

        BuiltinToolConfig {
            tavily_api_key,
            generation_registry: Some(generation_registry.clone()),
        }
    };

    // Create shared AetherToolServerHandle with built-in tools for hot-reload support
    let tool_server_handle = create_builtin_tool_server(Some(&builtin_tool_config));
    let registered_tools = Arc::new(RwLock::new(create_builtin_tools_list()));
    info!(
        tools = ?registered_tools.read().unwrap(),
        "Created shared AetherToolServerHandle with built-in tools"
    );

    let core = Arc::new(AetherCore {
        config_holder,
        full_config: Arc::new(Mutex::new(full_config)),
        config_path, // Store config path for reload capability
        memory_path,
        handler,
        runtime,
        _owned_runtime: owned_runtime, // Keep runtime alive if we created it
        current_op_token,
        tool_server_handle,
        registered_tools,
        agent_engine: Arc::new(RwLock::new(None)), // Lazily initialized
        conversation_histories: Arc::new(RwLock::new(HashMap::new())), // Multi-turn support
        generation_registry,
    });

    // Start background runtime update check (Phase 7)
    // This checks for updates asynchronously and notifies UI if available
    core.start_runtime_update_check();

    Ok(core)
}
