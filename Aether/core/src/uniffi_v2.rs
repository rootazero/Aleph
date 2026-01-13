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
//! let handler = Arc::new(MyHandler::new());
//! let core = init_v2("~/.config/aether/config.toml", handler)?;
//!
//! core.process("Hello, world!".to_string(), None)?;
//! ```

use crate::agent::{RigAgentConfig, RigAgentManager};
use crate::store::sqlite::MemoryEntry;
use std::sync::Arc;
use tracing::{error, info};

/// Error type for UniFFI v2
///
/// This error type is designed to be FFI-friendly with simple string messages
/// for each error variant.
#[derive(Debug, thiserror::Error)]
pub enum AetherV2Error {
    #[error("Configuration error: {message}")]
    Config { message: String },
    #[error("Provider error: {message}")]
    Provider { message: String },
    #[error("Tool error: {message}")]
    Tool { message: String },
    #[error("Memory error: {message}")]
    Memory { message: String },
    #[error("Operation cancelled")]
    Cancelled,
}

impl From<crate::error::AetherError> for AetherV2Error {
    fn from(e: crate::error::AetherError) -> Self {
        AetherV2Error::Config { message: e.to_string() }
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
    config_holder: AgentConfigHolder,
    memory_path: Option<MemoryStorePath>,
    handler: Arc<dyn AetherV2EventHandler>,
    runtime: tokio::runtime::Handle,
}

impl AetherV2Core {
    /// Process user input asynchronously
    ///
    /// This method processes the input on a background thread and calls
    /// the appropriate handler callbacks during processing.
    pub fn process(
        &self,
        input: String,
        options: Option<ProcessOptionsV2>,
    ) -> Result<(), AetherV2Error> {
        let _options = options.unwrap_or_default();
        let handler = Arc::clone(&self.handler);
        let config = self.config_holder.config().clone();
        let runtime = self.runtime.clone();

        // Spawn a background thread to handle processing
        std::thread::spawn(move || {
            handler.on_thinking();

            // Create a fresh manager in the new thread
            let manager = RigAgentManager::new(config);

            let result = runtime.block_on(async {
                manager.process(&input).await
            });

            match result {
                Ok(response) => {
                    handler.on_complete(response.content);
                }
                Err(e) => {
                    error!(error = %e, "Processing failed");
                    handler.on_error(e.to_string());
                }
            }
        });

        Ok(())
    }

    /// Cancel current operation
    ///
    /// Note: Cancellation is not yet implemented.
    pub fn cancel(&self) {
        // TODO: Implement cancellation via CancellationToken
        info!("Cancel requested");
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
            AetherV2Error::Memory { message: "Memory store not initialized".to_string() }
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
            Err(e) => Err(AetherV2Error::Memory { message: e.to_string() }),
        }
    }

    /// Clear all memory entries
    pub fn clear_memory(&self) -> Result<(), AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory { message: "Memory store not initialized".to_string() }
        })?;

        let db_path = memory_path.path.clone();

        let result = self.runtime.block_on(async move {
            use crate::store::MemoryStore;
            let store = MemoryStore::new(&db_path).await?;
            store.clear().await
        });

        result.map_err(|e| AetherV2Error::Memory { message: e.to_string() })
    }

    /// Reload configuration from file
    ///
    /// Note: Configuration reload is not yet implemented.
    pub fn reload_config(&self) -> Result<(), AetherV2Error> {
        // TODO: Implement config reload
        info!("Config reload requested");
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
/// * `config_path` - Path to the configuration file
/// * `handler` - Event handler for callbacks
///
/// # Returns
///
/// Returns an Arc-wrapped AetherV2Core on success, or an error if
/// initialization fails.
pub fn init_v2(
    config_path: String,
    handler: Arc<dyn AetherV2EventHandler>,
) -> Result<Arc<AetherV2Core>, AetherV2Error> {
    info!(config_path = %config_path, "Initializing AetherV2Core");

    // Create runtime if not in async context
    let runtime = tokio::runtime::Handle::try_current()
        .unwrap_or_else(|_| {
            tokio::runtime::Runtime::new()
                .expect("Failed to create Tokio runtime")
                .handle()
                .clone()
        });

    // TODO: Load config from file
    let config = RigAgentConfig::default();
    let config_holder = AgentConfigHolder::new(config);

    Ok(Arc::new(AetherV2Core {
        config_holder,
        memory_path: None,
        handler,
        runtime,
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
        let err = AetherV2Error::Config { message: "test error".to_string() };
        assert_eq!(format!("{}", err), "Configuration error: test error");

        let err = AetherV2Error::Provider { message: "provider failed".to_string() };
        assert_eq!(format!("{}", err), "Provider error: provider failed");

        let err = AetherV2Error::Tool { message: "tool error".to_string() };
        assert_eq!(format!("{}", err), "Tool error: tool error");

        let err = AetherV2Error::Memory { message: "memory error".to_string() };
        assert_eq!(format!("{}", err), "Memory error: memory error");

        let err = AetherV2Error::Cancelled;
        assert_eq!(format!("{}", err), "Operation cancelled");
    }
}
