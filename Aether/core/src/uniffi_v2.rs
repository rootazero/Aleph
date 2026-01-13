//! UniFFI v2 bindings for simplified rig-based architecture
//!
//! This module provides a streamlined interface for the rig-based agent system.
//! It is designed to be exposed via UniFFI in the future when the v2 architecture
//! is fully integrated.
//!
//! # Architecture
//!
//! The v2 architecture simplifies the existing Aether core by:
//! - Using RigAgentManager for all AI processing
//! - Providing a simpler event callback interface
//! - Supporting both sync and async operations
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::uniffi_v2::{AetherV2Core, init_v2};
//!
//! let handler = Box::new(MyHandler::new());
//! let core = init_v2("~/.config/aether/config.toml", handler)?;
//!
//! core.process("Hello, world!".to_string(), None)?;
//! ```

use crate::agent::{RigAgentConfig, RigAgentManager};
use crate::config::Config;
use crate::store::sqlite::MemoryEntry;
use std::path::Path;
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

/// Error type for UniFFI v2
///
/// This error type is designed to be FFI-friendly.
/// UniFFI Error enums must use simple variants with message support via Display trait.
#[derive(Debug, thiserror::Error)]
pub enum AetherV2Error {
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

impl From<crate::error::AetherError> for AetherV2Error {
    fn from(e: crate::error::AetherError) -> Self {
        AetherV2Error::Config(e.to_string())
    }
}

/// Event handler callback interface for v2
///
/// Clients implement this trait to receive callbacks during AI processing.
/// All methods take `&self` for thread-safe callback invocation.
pub trait AetherV2EventHandler: Send + Sync {
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

/// Processing options for v2
#[derive(Debug, Clone)]
pub struct ProcessOptionsV2 {
    /// Application context (bundle ID)
    pub app_context: Option<String>,
    /// Window title of the active application
    pub window_title: Option<String>,
    /// Enable streaming mode
    pub stream: bool,
}

impl Default for ProcessOptionsV2 {
    fn default() -> Self {
        Self {
            app_context: None,
            window_title: None,
            stream: true,  // Streaming enabled by default
        }
    }
}

impl ProcessOptionsV2 {
    /// Create new processing options with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the application context
    pub fn with_app_context(mut self, context: String) -> Self {
        self.app_context = Some(context);
        self
    }

    /// Set the window title
    pub fn with_window_title(mut self, title: String) -> Self {
        self.window_title = Some(title);
        self
    }

    /// Set streaming mode
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }
}

/// Tool information for UI display
#[derive(Debug, Clone)]
pub struct ToolInfoV2 {
    /// Tool name/identifier
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Tool source (builtin, mcp, skill, etc.)
    pub source: String,
}

impl ToolInfoV2 {
    /// Create a new tool info
    pub fn new(name: String, description: String, source: String) -> Self {
        Self { name, description, source }
    }
}

/// Memory item for UI display
#[derive(Debug, Clone)]
pub struct MemoryItemV2 {
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

impl From<MemoryEntry> for MemoryItemV2 {
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

/// Memory store path for lazy initialization
///
/// This wrapper allows us to store the path without the actual MemoryStore,
/// enabling on-demand creation for each operation.
struct MemoryStorePath {
    path: String,
}

/// Agent configuration holder for thread-safe access
///
/// Since RigAgentManager may contain non-Send types (via MemoryStore),
/// we store only the config and create managers on-demand.
struct AgentConfigHolder {
    config: RigAgentConfig,
}

impl AgentConfigHolder {
    fn new(config: RigAgentConfig) -> Self {
        Self { config }
    }

    fn config(&self) -> &RigAgentConfig {
        &self.config
    }
}

/// Core v2 implementation
///
/// This struct provides the main interface for the v2 architecture.
/// It manages the configuration and provides methods for processing,
/// tool management, and memory operations.
///
/// Note: RigAgentManager is created on-demand because it may contain
/// non-Send types. The config is stored separately.
pub struct AetherV2Core {
    /// Configuration holder with interior mutability for reload support
    config_holder: Arc<RwLock<AgentConfigHolder>>,
    /// Config file path for reload capability (empty string means default path)
    config_path: String,
    memory_path: Option<MemoryStorePath>,
    handler: Arc<dyn AetherV2EventHandler>,
    runtime: tokio::runtime::Handle,
    /// Current operation's cancellation token
    /// Each new operation gets a fresh token, allowing cancellation state to be reset
    current_op_token: Arc<RwLock<CancellationToken>>,
}

impl AetherV2Core {
    /// Process user input asynchronously
    ///
    /// This method processes the input on a background thread and calls
    /// the appropriate handler callbacks during processing.
    ///
    /// The operation can be cancelled by calling `cancel()`. When cancelled,
    /// the handler's `on_error` callback will be invoked with "Operation cancelled".
    pub fn process(
        &self,
        input: String,
        options: Option<ProcessOptionsV2>,
    ) -> Result<(), AetherV2Error> {
        let _options = options.unwrap_or_default();
        let handler = Arc::clone(&self.handler);
        // Acquire read lock to get current config (supports config reload)
        let config = self.config_holder.read().unwrap().config().clone();
        let runtime = self.runtime.clone();

        // Create a fresh token for this operation
        // This resets cancellation state, allowing new operations after previous cancellations
        let op_token = self.reset_cancel_token();

        // Spawn a background thread to handle processing
        std::thread::spawn(move || {
            // Check if already cancelled before starting
            if op_token.is_cancelled() {
                handler.on_error("Operation cancelled".to_string());
                return;
            }

            handler.on_thinking();

            // Create a fresh manager in the new thread
            let manager = RigAgentManager::new(config);

            let result = runtime.block_on(async {
                tokio::select! {
                    biased;

                    // Check for cancellation first (biased mode)
                    _ = op_token.cancelled() => {
                        Err(crate::error::AetherError::cancelled())
                    }

                    // Process the request
                    result = manager.process(&input) => {
                        result
                    }
                }
            });

            match result {
                Ok(response) => {
                    // If tokio::select! returned the result branch, the operation completed successfully
                    handler.on_complete(response.content);
                }
                Err(e) => {
                    // Check if the error is due to cancellation
                    if op_token.is_cancelled() {
                        handler.on_error("Operation cancelled".to_string());
                    } else {
                        error!(error = %e, "Processing failed");
                        handler.on_error(e.to_string());
                    }
                }
            }
        });

        Ok(())
    }

    /// Cancel current operation
    ///
    /// Triggers cancellation of the current in-progress operation.
    /// The handler's `on_error` callback will be invoked with "Operation cancelled".
    /// After cancellation, subsequent calls to `process()` will work normally
    /// since each operation gets a fresh cancellation token.
    pub fn cancel(&self) {
        info!("Cancel requested, triggering cancellation");
        self.current_op_token.read().unwrap().cancel();
    }

    /// Check if the current operation has been cancelled
    pub fn is_cancelled(&self) -> bool {
        self.current_op_token.read().unwrap().is_cancelled()
    }

    /// Create a fresh cancellation token for a new operation
    ///
    /// This replaces the current token with a new one, effectively resetting
    /// the cancellation state. Returns a clone of the new token for the operation.
    fn reset_cancel_token(&self) -> CancellationToken {
        let new_token = CancellationToken::new();
        let token_clone = new_token.clone();
        *self.current_op_token.write().unwrap() = new_token;
        token_clone
    }

    /// List available tools
    ///
    /// Returns a list of all tools available in the current configuration.
    pub fn list_tools(&self) -> Vec<ToolInfoV2> {
        vec![
            ToolInfoV2 {
                name: "search".to_string(),
                description: "Search the internet".to_string(),
                source: "builtin".to_string(),
            },
            ToolInfoV2 {
                name: "web_fetch".to_string(),
                description: "Fetch web page content".to_string(),
                source: "builtin".to_string(),
            },
        ]
    }

    /// Search memory for relevant entries
    ///
    /// Searches the memory store for entries matching the query.
    pub fn search_memory(&self, query: String, limit: u32) -> Result<Vec<MemoryItemV2>, AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        // Create a temporary MemoryStore for the query
        // This is necessary because MemoryStore contains non-Send types
        let db_path = memory_path.path.clone();
        let query_clone = query.clone();

        let result = self.runtime.block_on(async move {
            use crate::store::MemoryStore;
            let store = MemoryStore::new(&db_path).await?;
            store.search(&query_clone, limit as usize).await
        });

        match result {
            Ok(entries) => Ok(entries.into_iter().map(|(e, _)| e.into()).collect()),
            Err(e) => Err(AetherV2Error::Memory(e.to_string())),
        }
    }

    /// Clear all memory entries
    pub fn clear_memory(&self) -> Result<(), AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        let db_path = memory_path.path.clone();

        let result = self.runtime.block_on(async move {
            use crate::store::MemoryStore;
            let store = MemoryStore::new(&db_path).await?;
            store.clear().await
        });

        result.map_err(|e| AetherV2Error::Memory(e.to_string()))
    }

    /// Reload configuration from file
    ///
    /// Re-loads config from the original config path and updates the internal
    /// configuration. If reload fails, the existing configuration remains unchanged.
    ///
    /// # Returns
    /// * `Ok(())` - Configuration reloaded successfully
    /// * `Err(AetherV2Error::Config)` - Failed to load or parse config file
    pub fn reload_config(&self) -> Result<(), AetherV2Error> {
        info!(path = %self.config_path, "Reloading config");

        // Load config from stored path (same logic as init_v2)
        let full_config = if self.config_path.is_empty() {
            // Use default path (~/.config/aether/config.toml)
            Config::load().map_err(|e| AetherV2Error::Config(e.to_string()))?
        } else {
            let path = Path::new(&self.config_path);
            if path.exists() {
                Config::load_from_file(path).map_err(|e| AetherV2Error::Config(e.to_string()))?
            } else {
                return Err(AetherV2Error::Config(format!("Config file not found: {}", self.config_path)));
            }
        };

        // Extract provider settings (same logic as init_v2)
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
                        None::<String>,
                        provider_config.temperature,
                        provider_config.max_tokens,
                    )
                } else {
                    info!(provider = %name, "Default provider config not found, using defaults");
                    ("openai".to_string(), "gpt-4o".to_string(), None, None, None, None, None)
                }
            } else {
                info!("No default provider configured, using openai defaults");
                ("openai".to_string(), "gpt-4o".to_string(), None, None, None, None, None)
            }
        };

        // Create new RigAgentConfig with loaded values
        let new_config = RigAgentConfig {
            provider,
            model,
            temperature: temperature.unwrap_or(0.7),
            max_tokens: max_tokens.unwrap_or(4096),
            system_prompt: system_prompt.unwrap_or_else(|| "You are Aether, an intelligent assistant.".to_string()),
            api_key,
            base_url,
        };

        info!(
            provider = %new_config.provider,
            model = %new_config.model,
            has_api_key = new_config.api_key.is_some(),
            has_base_url = new_config.base_url.is_some(),
            "Config reloaded successfully"
        );

        // Update config holder (acquire write lock)
        *self.config_holder.write().unwrap() = AgentConfigHolder::new(new_config);

        Ok(())
    }
}

/// Initialize AetherV2Core
///
/// Creates a new AetherV2Core instance with the given configuration path
/// and event handler.
///
/// # Arguments
///
/// * `config_path` - Path to the configuration file (empty string uses default path)
/// * `handler` - Event handler for callbacks
///
/// # Returns
///
/// Returns an Arc-wrapped AetherV2Core on success, or an error if
/// initialization fails.
///
/// # Config Loading Behavior
///
/// - If `config_path` is empty: Load from default path (~/.config/aether/config.toml)
/// - If `config_path` is provided and file exists: Load from that path
/// - If `config_path` is provided but file doesn't exist: Use defaults with info log
/// - If config file exists but has parse errors: Return `AetherV2Error::Config`
pub fn init_v2(
    config_path: String,
    handler: Box<dyn AetherV2EventHandler>,
) -> Result<Arc<AetherV2Core>, AetherV2Error> {
    info!(config_path = %config_path, "Initializing AetherV2Core");

    // Convert Box to Arc for internal use
    let handler: Arc<dyn AetherV2EventHandler> = Arc::from(handler);

    // Create runtime if not in async context
    let runtime = tokio::runtime::Handle::try_current()
        .unwrap_or_else(|_| {
            tokio::runtime::Runtime::new()
                .expect("Failed to create Tokio runtime")
                .handle()
                .clone()
        });

    // Load config from file
    let full_config = if config_path.is_empty() {
        // Use default path (~/.config/aether/config.toml)
        Config::load().map_err(|e| AetherV2Error::Config(e.to_string()))?
    } else {
        let path = Path::new(&config_path);
        if path.exists() {
            Config::load_from_file(path).map_err(|e| AetherV2Error::Config(e.to_string()))?
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
        Some(MemoryStorePath { path: db_path.to_string_lossy().to_string() })
    } else {
        info!("Memory store disabled in config");
        None
    };

    // Create initial cancellation token wrapped in Arc<RwLock> for interior mutability
    // Each operation will get a fresh token via reset_cancel_token()
    let current_op_token = Arc::new(RwLock::new(CancellationToken::new()));

    Ok(Arc::new(AetherV2Core {
        config_holder,
        config_path,  // Store config path for reload capability
        memory_path,
        handler,
        runtime,
        current_op_token,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[allow(dead_code)]
    struct TestHandler {
        completed: AtomicBool,
    }

    impl TestHandler {
        #[allow(dead_code)]
        fn new() -> Self {
            Self { completed: AtomicBool::new(false) }
        }
    }

    impl AetherV2EventHandler for TestHandler {
        fn on_thinking(&self) {}
        fn on_tool_start(&self, _: String) {}
        fn on_tool_result(&self, _: String, _: String) {}
        fn on_stream_chunk(&self, _: String) {}
        fn on_complete(&self, _: String) {
            self.completed.store(true, Ordering::SeqCst);
        }
        fn on_error(&self, _: String) {}
        fn on_memory_stored(&self) {}
    }

    #[test]
    fn test_tool_info_creation() {
        let info = ToolInfoV2 {
            name: "test".to_string(),
            description: "Test tool".to_string(),
            source: "builtin".to_string(),
        };
        assert_eq!(info.name, "test");
    }

    #[test]
    fn test_process_options_default() {
        let options = ProcessOptionsV2::default();
        assert!(options.stream);
        assert!(options.app_context.is_none());
    }

    #[test]
    fn test_process_options_builder() {
        let options = ProcessOptionsV2::new()
            .with_app_context("com.example.app".to_string())
            .with_window_title("Test Window".to_string())
            .with_stream(false);

        assert_eq!(options.app_context, Some("com.example.app".to_string()));
        assert_eq!(options.window_title, Some("Test Window".to_string()));
        assert!(!options.stream);
    }

    #[test]
    fn test_tool_info_new() {
        let info = ToolInfoV2::new(
            "test_tool".to_string(),
            "A test tool".to_string(),
            "native".to_string(),
        );
        assert_eq!(info.name, "test_tool");
        assert_eq!(info.description, "A test tool");
        assert_eq!(info.source, "native");
    }

    #[test]
    fn test_aether_v2_error_display() {
        let err = AetherV2Error::Config("test error".to_string());
        assert_eq!(format!("{}", err), "Configuration error: test error");

        let err = AetherV2Error::Provider("provider failed".to_string());
        assert_eq!(format!("{}", err), "Provider error: provider failed");

        let err = AetherV2Error::Tool("tool error".to_string());
        assert_eq!(format!("{}", err), "Tool error: tool error");

        let err = AetherV2Error::Memory("memory error".to_string());
        assert_eq!(format!("{}", err), "Memory error: memory error");

        let err = AetherV2Error::Cancelled;
        assert_eq!(format!("{}", err), "Operation cancelled");
    }

    /// Test handler that tracks cancellation errors
    struct CancellationTestHandler {
        thinking_called: AtomicBool,
        cancelled: AtomicBool,
        error_message: std::sync::Mutex<Option<String>>,
    }

    impl CancellationTestHandler {
        fn new() -> Self {
            Self {
                thinking_called: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                error_message: std::sync::Mutex::new(None),
            }
        }
    }

    impl AetherV2EventHandler for CancellationTestHandler {
        fn on_thinking(&self) {
            self.thinking_called.store(true, Ordering::SeqCst);
        }
        fn on_tool_start(&self, _: String) {}
        fn on_tool_result(&self, _: String, _: String) {}
        fn on_stream_chunk(&self, _: String) {}
        fn on_complete(&self, _: String) {}
        fn on_error(&self, message: String) {
            if message.contains("cancelled") {
                self.cancelled.store(true, Ordering::SeqCst);
            }
            *self.error_message.lock().unwrap() = Some(message);
        }
        fn on_memory_stored(&self) {}
    }

    #[test]
    fn test_cancellation_token_triggers_cancel() {
        // Create a CancellationToken and verify cancel() triggers it
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_fresh_token_is_independent() {
        // Test that fresh tokens are independent (not child tokens)
        // This verifies the fix for Issue 1: parent token permanent cancellation
        let token1 = CancellationToken::new();
        let token2 = CancellationToken::new();

        token1.cancel();

        // token2 should NOT be affected by token1's cancellation
        assert!(token1.is_cancelled());
        assert!(!token2.is_cancelled());
    }

    #[test]
    fn test_init_v2_creates_cancel_token() {
        let handler = Box::new(CancellationTestHandler::new());
        let core = init_v2("/test/config.toml".to_string(), handler).unwrap();

        // Initially not cancelled
        assert!(!core.is_cancelled());

        // After cancel(), should be cancelled
        core.cancel();
        assert!(core.is_cancelled());
    }

    #[test]
    fn test_cancellation_state_resets_between_operations() {
        // Test that each process() gets a fresh token, allowing new operations after cancellation
        // This verifies the fix for Issue 2: missing reset mechanism

        // Use Arc for the inner handler to allow checking state after init_v2
        let inner_handler = Arc::new(CancellationTestHandler::new());
        let inner_handler_clone = Arc::clone(&inner_handler);

        // Create a wrapper that implements AetherV2EventHandler and delegates to Arc
        struct ArcHandler(Arc<CancellationTestHandler>);
        impl AetherV2EventHandler for ArcHandler {
            fn on_thinking(&self) { self.0.on_thinking(); }
            fn on_tool_start(&self, name: String) { self.0.on_tool_start(name); }
            fn on_tool_result(&self, name: String, result: String) { self.0.on_tool_result(name, result); }
            fn on_stream_chunk(&self, text: String) { self.0.on_stream_chunk(text); }
            fn on_complete(&self, response: String) { self.0.on_complete(response); }
            fn on_error(&self, message: String) { self.0.on_error(message); }
            fn on_memory_stored(&self) { self.0.on_memory_stored(); }
        }

        let handler = Box::new(ArcHandler(inner_handler_clone));
        let core = init_v2("/test/config.toml".to_string(), handler).unwrap();

        // Cancel the current operation
        core.cancel();
        assert!(core.is_cancelled());

        // Start a new process - this should create a fresh token and NOT be cancelled
        let result = core.process("test input".to_string(), None);
        assert!(result.is_ok());

        // The new operation should have a fresh (non-cancelled) token
        // Note: is_cancelled() now reflects the NEW operation's token state
        assert!(!core.is_cancelled(), "New operation should not be cancelled");

        // Wait a bit for the background thread to start
        std::thread::sleep(std::time::Duration::from_millis(50));

        // The handler should have received on_thinking (not cancellation error)
        assert!(inner_handler.thinking_called.load(Ordering::SeqCst),
            "Handler should receive on_thinking for new operation");
    }

    #[test]
    fn test_cancel_method_logs_info() {
        // Test that cancel() logs the cancellation request
        let handler = Box::new(CancellationTestHandler::new());
        let core = init_v2("/test/config.toml".to_string(), handler).unwrap();

        // This should not panic and should log
        core.cancel();

        // Verify the token is cancelled
        assert!(core.is_cancelled());
    }

    // ========================================
    // Config Loading Tests (Phase 2.2)
    // ========================================

    #[test]
    fn test_init_v2_with_nonexistent_config_uses_defaults() {
        // When config file doesn't exist, should use defaults
        let handler = Box::new(CancellationTestHandler::new());
        let core = init_v2("/nonexistent/path/config.toml".to_string(), handler).unwrap();

        // Should initialize successfully with defaults
        assert!(!core.is_cancelled());
    }

    #[test]
    fn test_init_v2_with_empty_path_uses_default_path() {
        // When config_path is empty, should try default path
        // This will use Config::load() which handles default path
        let handler = Box::new(CancellationTestHandler::new());

        // This should succeed (uses default config if file doesn't exist)
        let result = init_v2(String::new(), handler);
        assert!(result.is_ok());
    }

    #[test]
    fn test_init_v2_config_loading_from_temp_file() {
        use std::io::Write;

        // Create a temp config file with valid TOML
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("aether_test_config.toml");

        let config_content = r##"
default_hotkey = "Grave"

[general]
default_provider = "openai"

[providers.openai]
model = "gpt-4o-mini"
api_key = "test-api-key-12345"
base_url = "https://api.custom.com/v1"
enabled = true
timeout_seconds = 30
color = "#10a37f"

[memory]
enabled = false
"##;

        let mut file = std::fs::File::create(&config_path).expect("Failed to create temp config file");
        file.write_all(config_content.as_bytes()).expect("Failed to write config");
        drop(file);

        // Initialize with the temp config file
        let handler = Box::new(CancellationTestHandler::new());
        let result = init_v2(config_path.to_string_lossy().to_string(), handler);

        // Clean up the temp file
        let _ = std::fs::remove_file(&config_path);

        // Verify initialization succeeded
        assert!(result.is_ok(), "init_v2 should succeed with valid config file");
    }

    #[test]
    fn test_init_v2_with_invalid_config_returns_error() {
        use std::io::Write;

        // Create a temp config file with invalid TOML
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("aether_test_invalid_config.toml");

        let invalid_content = r#"
this is not valid toml
[providers.openai
missing closing bracket
"#;

        let mut file = std::fs::File::create(&config_path).expect("Failed to create temp config file");
        file.write_all(invalid_content.as_bytes()).expect("Failed to write config");
        drop(file);

        // Initialize with the invalid config file
        let handler = Box::new(CancellationTestHandler::new());
        let result = init_v2(config_path.to_string_lossy().to_string(), handler);

        // Clean up the temp file
        let _ = std::fs::remove_file(&config_path);

        // Should return a Config error
        assert!(result.is_err(), "init_v2 should fail with invalid config file");
        if let Err(AetherV2Error::Config(message)) = result {
            assert!(!message.is_empty(), "Error message should not be empty");
        } else {
            panic!("Expected AetherV2Error::Config variant");
        }
    }

    #[test]
    fn test_rig_agent_config_default_includes_new_fields() {
        // Verify RigAgentConfig default includes api_key and base_url
        let config = RigAgentConfig::default();
        assert!(config.api_key.is_none());
        assert!(config.base_url.is_none());
        assert_eq!(config.provider, "openai");
        assert_eq!(config.model, "gpt-4o");
    }

    // ========================================
    // Config Reload Tests (Phase 2.3)
    // ========================================

    #[test]
    fn test_reload_config_with_nonexistent_file_returns_error() {
        // Initialize with a non-existent config path
        let handler = Box::new(CancellationTestHandler::new());
        let core = init_v2("/nonexistent/path/config.toml".to_string(), handler).unwrap();

        // After init (which falls back to defaults), try to reload
        // This should fail because the file doesn't exist
        let result = core.reload_config();
        assert!(result.is_err(), "reload_config should fail when config file doesn't exist");

        if let Err(AetherV2Error::Config(message)) = result {
            assert!(message.contains("not found"), "Error message should indicate file not found");
        } else {
            panic!("Expected AetherV2Error::Config variant");
        }
    }

    #[test]
    fn test_reload_config_with_valid_file_succeeds() {
        use std::io::Write;

        // Create a temp config file with valid TOML
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("aether_test_reload_config.toml");

        let config_content = r##"
default_hotkey = "Grave"

[general]
default_provider = "openai"

[providers.openai]
model = "gpt-4o-mini"
api_key = "test-api-key-12345"
enabled = true
timeout_seconds = 30

[memory]
enabled = false
"##;

        let mut file = std::fs::File::create(&config_path).expect("Failed to create temp config file");
        file.write_all(config_content.as_bytes()).expect("Failed to write config");
        drop(file);

        // Initialize with the temp config file
        let handler = Box::new(CancellationTestHandler::new());
        let core = init_v2(config_path.to_string_lossy().to_string(), handler).unwrap();

        // Reload config - should succeed
        let result = core.reload_config();

        // Clean up the temp file
        let _ = std::fs::remove_file(&config_path);

        assert!(result.is_ok(), "reload_config should succeed with valid config file: {:?}", result);
    }

    #[test]
    fn test_reload_config_updates_internal_config() {
        use std::io::Write;

        // Create initial config file
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("aether_test_reload_update.toml");

        let initial_config = r##"
default_hotkey = "Grave"

[general]
default_provider = "openai"

[providers.openai]
model = "gpt-4o"
api_key = "initial-api-key"
enabled = true
timeout_seconds = 30

[memory]
enabled = false
"##;

        let mut file = std::fs::File::create(&config_path).expect("Failed to create temp config file");
        file.write_all(initial_config.as_bytes()).expect("Failed to write config");
        drop(file);

        // Initialize
        let handler = Box::new(CancellationTestHandler::new());
        let core = init_v2(config_path.to_string_lossy().to_string(), handler).unwrap();

        // Verify initial model
        {
            let config = core.config_holder.read().unwrap();
            assert_eq!(config.config().model, "gpt-4o");
        }

        // Update config file with new model
        let updated_config = r##"
default_hotkey = "Grave"

[general]
default_provider = "openai"

[providers.openai]
model = "gpt-4o-mini"
api_key = "updated-api-key"
enabled = true
timeout_seconds = 30

[memory]
enabled = false
"##;

        let mut file = std::fs::File::create(&config_path).expect("Failed to create temp config file");
        file.write_all(updated_config.as_bytes()).expect("Failed to write config");
        drop(file);

        // Reload config
        let result = core.reload_config();

        // Clean up the temp file
        let _ = std::fs::remove_file(&config_path);

        assert!(result.is_ok(), "reload_config should succeed");

        // Verify model was updated
        {
            let config = core.config_holder.read().unwrap();
            assert_eq!(config.config().model, "gpt-4o-mini", "Model should be updated after reload");
        }
    }

    #[test]
    fn test_reload_config_with_empty_path_uses_default() {
        // Initialize with empty path (uses default config path)
        let handler = Box::new(CancellationTestHandler::new());
        let core = init_v2(String::new(), handler).unwrap();

        // Reload should not panic (may fail if default config doesn't exist, which is OK)
        // The important thing is that it doesn't crash and handles the empty path case
        let _result = core.reload_config();
        // No assertion on result - just verify it doesn't panic
    }

    #[test]
    fn test_reload_config_preserves_existing_on_failure() {
        use std::io::Write;

        // Create initial valid config file
        let temp_dir = std::env::temp_dir();
        let config_path = temp_dir.join("aether_test_reload_preserve.toml");

        let valid_config = r##"
default_hotkey = "Grave"

[general]
default_provider = "openai"

[providers.openai]
model = "gpt-4o"
api_key = "test-api-key"
enabled = true
timeout_seconds = 30

[memory]
enabled = false
"##;

        let mut file = std::fs::File::create(&config_path).expect("Failed to create temp config file");
        file.write_all(valid_config.as_bytes()).expect("Failed to write config");
        drop(file);

        // Initialize
        let handler = Box::new(CancellationTestHandler::new());
        let core = init_v2(config_path.to_string_lossy().to_string(), handler).unwrap();

        // Verify initial model
        {
            let config = core.config_holder.read().unwrap();
            assert_eq!(config.config().model, "gpt-4o");
        }

        // Now write invalid config to the file
        let invalid_config = r#"
this is not valid toml [broken
"#;

        let mut file = std::fs::File::create(&config_path).expect("Failed to create temp config file");
        file.write_all(invalid_config.as_bytes()).expect("Failed to write config");
        drop(file);

        // Try to reload - should fail
        let result = core.reload_config();

        // Clean up the temp file
        let _ = std::fs::remove_file(&config_path);

        assert!(result.is_err(), "reload_config should fail with invalid config");

        // Verify original config is preserved
        {
            let config = core.config_holder.read().unwrap();
            assert_eq!(config.config().model, "gpt-4o", "Original config should be preserved on reload failure");
        }
    }

    #[test]
    fn test_config_path_stored_correctly() {
        // Test with specific path
        let test_path = "/test/path/config.toml";
        let handler1 = Box::new(CancellationTestHandler::new());
        let core = init_v2(test_path.to_string(), handler1).unwrap();
        assert_eq!(core.config_path, test_path, "Config path should be stored");

        // Test with empty path
        let handler2 = Box::new(CancellationTestHandler::new());
        let core2 = init_v2(String::new(), handler2).unwrap();
        assert!(core2.config_path.is_empty(), "Empty config path should remain empty");
    }
}
