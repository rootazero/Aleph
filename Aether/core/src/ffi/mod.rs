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
//! - `cowork`: Cowork task orchestration (cowork_plan, cowork_execute, etc.)

mod config;
mod cowork;
mod mcp;
mod memory;
mod processing;
mod skills;
mod tools;

use crate::agent::{RigAgentConfig, RigAgentManager};
use crate::config::Config;
use crate::store::sqlite::MemoryEntry;
use rig::tool::server::ToolServerHandle;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

// Re-export public types
pub use self::processing::ProcessOptions;

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

impl ToolInfoFFI {
    /// Create a new tool info
    pub fn new(name: String, description: String, source: String) -> Self {
        Self { name, description, source }
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
            assistant_response: entry.assistant_response,
            timestamp: entry.timestamp,
            app_context: entry.app_context,
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
    pub(crate) memory_path: Option<String>,
    pub(crate) handler: Arc<dyn AetherEventHandler>,
    /// Tokio runtime handle for async operations
    pub(crate) runtime: tokio::runtime::Handle,
    /// Owned runtime to keep it alive (when we create our own)
    /// This MUST be stored to prevent the runtime from being dropped
    pub(crate) _owned_runtime: Option<tokio::runtime::Runtime>,
    /// Current operation's cancellation token
    /// Each new operation gets a fresh token, allowing cancellation state to be reset
    pub(crate) current_op_token: Arc<RwLock<CancellationToken>>,
    /// Shared ToolServerHandle for hot-reload support
    /// This handle is shared across all RigAgentManager instances
    pub(crate) tool_server_handle: ToolServerHandle,
    /// Names of registered tools (for tracking and display)
    pub(crate) registered_tools: Arc<RwLock<Vec<String>>>,
    /// Cowork engine for task orchestration (lazily initialized)
    pub(crate) cowork_engine: Arc<RwLock<Option<Arc<crate::cowork::CoworkEngine>>>>,
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
/// - If `config_path` is empty: Load from default path (~/.config/aether/config.toml)
/// - If `config_path` is provided and file exists: Load from that path
/// - If `config_path` is provided but file doesn't exist: Use defaults with info log
/// - If config file exists but has parse errors: Return `AetherFfiError::Config`
pub fn init_core(
    config_path: String,
    handler: Box<dyn AetherEventHandler>,
) -> Result<Arc<AetherCore>, AetherFfiError> {
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
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create Tokio runtime");
            let handle = rt.handle().clone();
            (handle, Some(rt))
        }
    };

    // Load config from file
    let full_config = if config_path.is_empty() {
        // Use default path (~/.config/aether/config.toml)
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
    let (provider, model, api_key, base_url, system_prompt, temperature, max_tokens) = {
        let default_provider = full_config.get_default_provider();
        if let Some(ref name) = default_provider {
            if let Some(provider_config) = full_config.providers.get(name) {
                let provider_type = provider_config.infer_provider_type(name);
                (
                    provider_type,
                    provider_config.model.clone(),
                    provider_config.api_key.clone(),
                    provider_config.base_url.clone(),
                    None::<String>, // Provider-level system_prompt not in ProviderConfig
                    provider_config.temperature,
                    provider_config.max_tokens,
                )
            } else {
                // Default provider name exists but config not found
                info!(provider = %name, "Default provider config not found, using defaults");
                ("openai".to_string(), "gpt-4o".to_string(), None, None, None, None, None)
            }
        } else {
            // No default provider configured
            info!("No default provider configured, using openai defaults");
            ("openai".to_string(), "gpt-4o".to_string(), None, None, None, None, None)
        }
    };

    // Create RigAgentConfig with loaded values
    let rig_config = RigAgentConfig {
        provider,
        model,
        temperature: temperature.unwrap_or(0.7),
        max_tokens: max_tokens.unwrap_or(4096),
        max_turns: 10, // Default to 10 turns for tool calling loop
        system_prompt: system_prompt.unwrap_or_else(|| "You are Aether, an intelligent assistant.".to_string()),
        api_key,
        base_url,
    };

    info!(
        provider = %rig_config.provider,
        model = %rig_config.model,
        has_api_key = rig_config.api_key.is_some(),
        has_base_url = rig_config.base_url.is_some(),
        "RigAgentConfig loaded from config file"
    );

    // Wrap config holder in Arc<RwLock> for reload support
    let config_holder = Arc::new(RwLock::new(AgentConfigHolder::new(rig_config)));

    // Set up memory store path if memory is enabled
    let memory_path = if full_config.memory.enabled {
        let db_path = dirs::home_dir()
            .map(|h| h.join(".config/aether/memory.db"))
            .unwrap_or_else(|| std::path::PathBuf::from("memory.db"));
        info!(path = %db_path.display(), "Memory store enabled");
        Some(db_path.to_string_lossy().to_string())
    } else {
        info!("Memory store disabled in config");
        None
    };

    // Create initial cancellation token wrapped in Arc<RwLock> for interior mutability
    // Each operation will get a fresh token via reset_cancel_token()
    let current_op_token = Arc::new(RwLock::new(CancellationToken::new()));

    // Set up search tool environment variables from config BEFORE creating ToolServer
    // This bridges the config file settings to the rig tools which read from env vars
    if let Some(ref search_config) = full_config.search {
        if search_config.enabled {
            // Set Tavily API key if configured
            if let Some(tavily_backend) = search_config.backends.get("tavily") {
                if let Some(ref api_key) = tavily_backend.api_key {
                    if !api_key.is_empty() {
                        std::env::set_var("TAVILY_API_KEY", api_key);
                        info!("Set TAVILY_API_KEY from config file");
                    }
                }
            }
        }
    }

    // Create shared ToolServerHandle with built-in tools for hot-reload support
    // NOTE: ToolServer::run() requires a tokio runtime context (uses tokio::spawn)
    // We use runtime.enter() to set the current runtime context before creating the handle
    let (tool_server_handle, registered_tools) = {
        let _guard = runtime.enter();  // Enter runtime context for tokio::spawn
        RigAgentManager::create_shared_handle()
    };
    info!(
        tools = ?registered_tools.read().unwrap(),
        "Created shared ToolServerHandle with built-in tools"
    );

    Ok(Arc::new(AetherCore {
        config_holder,
        full_config: Arc::new(Mutex::new(full_config)),
        config_path,  // Store config path for reload capability
        memory_path,
        handler,
        runtime,
        _owned_runtime: owned_runtime,  // Keep runtime alive if we created it
        current_op_token,
        tool_server_handle,
        registered_tools,
        cowork_engine: Arc::new(RwLock::new(None)),  // Lazily initialized
    }))
}
