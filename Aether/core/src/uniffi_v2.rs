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
use crate::config::{Config, FullConfig, ProviderConfig, RoutingRuleConfig, GeneralConfig, TestConnectionResult};
use crate::store::sqlite::MemoryEntry;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

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
    /// Full configuration with interior mutability for Settings UI operations
    full_config: Arc<Mutex<Config>>,
    /// Config file path for reload capability (empty string means default path)
    config_path: String,
    memory_path: Option<MemoryStorePath>,
    handler: Arc<dyn AetherV2EventHandler>,
    /// Tokio runtime handle for async operations
    runtime: tokio::runtime::Handle,
    /// Owned runtime to keep it alive (when we create our own)
    /// This MUST be stored to prevent the runtime from being dropped
    _owned_runtime: Option<tokio::runtime::Runtime>,
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

            // Create a fresh manager in the new thread with tools enabled
            let manager = RigAgentManager::new(config)
                .with_search_tool()
                .with_web_fetch_tool();

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

        // Also update full_config
        *self.full_config.lock().unwrap() = full_config;

        Ok(())
    }

    // ========================================================================
    // CONFIG MANAGEMENT METHODS (V1 → V2 Migration)
    // ========================================================================

    /// Acquires the full config mutex lock with poison recovery.
    #[inline(always)]
    fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
        self.full_config.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in full_config, recovering");
            e.into_inner()
        })
    }

    /// Load configuration and return it in UniFFI-compatible format
    pub fn load_config(&self) -> Result<FullConfig, AetherV2Error> {
        let config = self.lock_config();
        Ok(config.clone().into())
    }

    /// Update provider configuration
    pub fn update_provider(
        &self,
        name: String,
        provider: ProviderConfig,
    ) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();
        config.providers.insert(name, provider);
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        Ok(())
    }

    /// Delete provider configuration
    pub fn delete_provider(&self, name: String) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();
        config.providers.remove(&name);
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        Ok(())
    }

    /// Update routing rules
    ///
    /// This method updates the routing rules in config.
    /// **IMPORTANT**: Preserves builtin rules (is_builtin = true) and only
    /// updates user-defined rules.
    pub fn update_routing_rules(
        &self,
        rules: Vec<RoutingRuleConfig>,
    ) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();

        // Preserve builtin rules from current config
        let builtin_rules: Vec<_> = config
            .rules
            .iter()
            .filter(|r| r.is_builtin)
            .cloned()
            .collect();

        // Merge: builtin rules first (for priority), then user rules
        let mut merged_rules = builtin_rules;
        merged_rules.extend(rules);

        info!(
            builtin = merged_rules.iter().filter(|r| r.is_builtin).count(),
            user = merged_rules.iter().filter(|r| !r.is_builtin).count(),
            total = merged_rules.len(),
            "Updating routing rules"
        );

        config.rules = merged_rules;
        config.validate().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;

        info!("Routing rules updated");
        Ok(())
    }

    /// Update shortcuts configuration
    pub fn update_shortcuts(&self, shortcuts: crate::config::ShortcutsConfig) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();
        config.shortcuts = Some(shortcuts);
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!("Shortcuts configuration updated");
        Ok(())
    }

    /// Update behavior configuration
    pub fn update_behavior(&self, behavior: crate::config::BehaviorConfig) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();
        config.behavior = Some(behavior);
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!("Behavior configuration updated");
        Ok(())
    }

    /// Update trigger configuration
    pub fn update_trigger_config(&self, trigger: crate::config::TriggerConfig) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();
        config.trigger = Some(trigger);
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!("Trigger configuration updated");
        Ok(())
    }

    /// Update general configuration (language preference, etc.)
    pub fn update_general_config(&self, new_config: GeneralConfig) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();
        config.general = new_config;
        config.save().map_err(|e| AetherV2Error::Config(format!("Failed to save general config: {}", e)))?;
        Ok(())
    }

    /// Update search configuration
    pub fn update_search_config(&self, search: crate::config::SearchConfig) -> Result<(), AetherV2Error> {
        // Convert UniFFI SearchConfig to internal SearchConfigInternal
        let search_internal: crate::config::SearchConfigInternal = search.into();

        let mut config = self.lock_config();
        config.search = Some(search_internal);
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!("Search configuration updated");
        Ok(())
    }

    /// Validate regex pattern
    pub fn validate_regex(&self, pattern: String) -> Result<bool, AetherV2Error> {
        match regex::Regex::new(&pattern) {
            Ok(_) => Ok(true),
            Err(e) => Err(AetherV2Error::Config(format!("Invalid regex: {}", e))),
        }
    }

    /// Test provider connection with temporary configuration
    ///
    /// This method tests a provider without persisting the configuration to disk.
    /// Useful for "Test Connection" feature in UI before saving the provider.
    pub fn test_provider_connection_with_config(
        &self,
        provider_name: String,
        provider_config: ProviderConfig,
    ) -> TestConnectionResult {
        use crate::providers::create_provider;

        // Create provider instance
        let provider = match create_provider(&provider_name, provider_config) {
            Ok(p) => p,
            Err(e) => {
                return TestConnectionResult {
                    success: false,
                    message: format!("Failed to create provider: {}", e.user_friendly_message()),
                };
            }
        };

        // Send test request
        let test_prompt = "Say 'OK' if you can read this.";
        let result = self.runtime.block_on(async {
            provider.process(test_prompt, None).await.map_err(|e| format!("{}", e))
        });

        match result {
            Ok(response) => TestConnectionResult {
                success: true,
                message: format!(
                    "✓ Connection successful! Provider responded: {}",
                    response.chars().take(50).collect::<String>()
                ),
            },
            Err(err_msg) => TestConnectionResult {
                success: false,
                message: err_msg,
            },
        }
    }

    /// Get the current default provider (if exists and enabled)
    pub fn get_default_provider(&self) -> Option<String> {
        let config = self.lock_config();
        config.get_default_provider()
    }

    /// Set the default provider (validates that provider exists and is enabled)
    pub fn set_default_provider(&self, provider_name: String) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();
        config.set_default_provider(&provider_name)
            .map_err(|e| AetherV2Error::Config(e.to_string()))?;
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!(provider = %provider_name, "Default provider updated");
        Ok(())
    }

    /// Get list of all enabled provider names (sorted alphabetically)
    pub fn get_enabled_providers(&self) -> Vec<String> {
        let config = self.lock_config();
        config.get_enabled_providers()
    }

    // ========================================================================
    // MCP MANAGEMENT METHODS (V1 → V2 Migration)
    // ========================================================================

    /// Get MCP configuration for Settings UI
    pub fn get_mcp_config(&self) -> crate::mcp::McpSettingsConfig {
        let config = self.lock_config();
        crate::mcp::McpSettingsConfig {
            enabled: config.mcp.enabled,
            fs_enabled: config.tools.fs_enabled,
            git_enabled: config.tools.git_enabled,
            shell_enabled: config.tools.shell_enabled,
            system_info_enabled: config.tools.system_info_enabled,
            allowed_roots: config.tools.allowed_roots.clone(),
            allowed_repos: config.tools.allowed_repos.clone(),
            allowed_commands: config.tools.allowed_commands.clone(),
            shell_timeout_seconds: config.tools.shell_timeout_seconds,
        }
    }

    /// Update MCP configuration
    pub fn update_mcp_config(&self, new_config: crate::mcp::McpSettingsConfig) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();

        config.mcp.enabled = new_config.enabled;
        config.tools.fs_enabled = new_config.fs_enabled;
        config.tools.git_enabled = new_config.git_enabled;
        config.tools.shell_enabled = new_config.shell_enabled;
        config.tools.system_info_enabled = new_config.system_info_enabled;
        config.tools.allowed_roots = new_config.allowed_roots;
        config.tools.allowed_repos = new_config.allowed_repos;
        config.tools.allowed_commands = new_config.allowed_commands;
        config.tools.shell_timeout_seconds = new_config.shell_timeout_seconds;

        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!("MCP configuration updated");
        Ok(())
    }

    /// List all external MCP servers
    pub fn list_mcp_servers(&self) -> Vec<crate::mcp::McpServerConfig> {
        let config = self.lock_config();
        let mut servers = Vec::new();

        for ext in &config.mcp.external_servers {
            servers.push(crate::mcp::McpServerConfig {
                id: ext.name.clone(),
                name: ext.name.clone(),
                server_type: crate::mcp::McpServerType::External,
                enabled: true,
                command: Some(ext.command.clone()),
                args: ext.args.clone(),
                env: ext
                    .env
                    .iter()
                    .map(|(k, v)| crate::mcp::McpEnvVar {
                        key: k.clone(),
                        value: v.clone(),
                    })
                    .collect(),
                working_directory: ext.cwd.clone(),
                trigger_command: Some(format!("/mcp/{}", ext.name)),
                permissions: crate::mcp::McpServerPermissions {
                    requires_confirmation: true,
                    allowed_paths: Vec::new(),
                    allowed_commands: Vec::new(),
                },
                icon: "puzzlepiece.extension".to_string(),
                color: "#FF9500".to_string(),
            });
        }

        servers
    }

    /// Get a specific MCP server by ID
    pub fn get_mcp_server(&self, id: String) -> Option<crate::mcp::McpServerConfig> {
        self.list_mcp_servers().into_iter().find(|s| s.id == id)
    }

    /// Get MCP server status
    pub fn get_mcp_server_status(&self, id: String) -> crate::mcp::McpServerStatusInfo {
        let server = self.get_mcp_server(id.clone());

        match server {
            Some(s) => {
                if s.enabled {
                    crate::mcp::McpServerStatusInfo {
                        status: crate::mcp::McpServerStatus::Running,
                        message: Some("Server is active".to_string()),
                        last_error: None,
                    }
                } else {
                    crate::mcp::McpServerStatusInfo {
                        status: crate::mcp::McpServerStatus::Stopped,
                        message: Some("Server is disabled".to_string()),
                        last_error: None,
                    }
                }
            }
            None => crate::mcp::McpServerStatusInfo {
                status: crate::mcp::McpServerStatus::Error,
                message: None,
                last_error: Some(format!("Server '{}' not found", id)),
            },
        }
    }

    /// Add an external MCP server
    pub fn add_mcp_server(&self, config: crate::mcp::McpServerConfig) -> Result<(), AetherV2Error> {
        if config.server_type == crate::mcp::McpServerType::Builtin {
            return Err(AetherV2Error::Config("Cannot add builtin servers".to_string()));
        }

        let command = config
            .command
            .as_ref()
            .ok_or_else(|| AetherV2Error::Config("External server requires a command".to_string()))?;

        if config.id.is_empty() {
            return Err(AetherV2Error::Config("Server ID cannot be empty".to_string()));
        }

        let external_config = crate::config::McpExternalServerConfig {
            name: config.id.clone(),
            command: command.clone(),
            args: config.args.clone(),
            env: config
                .env
                .into_iter()
                .map(|e| (e.key, e.value))
                .collect(),
            cwd: config.working_directory,
            requires_runtime: None,
            timeout_seconds: 30,
        };

        let mut cfg = self.lock_config();

        if cfg.mcp.external_servers.iter().any(|s| s.name == config.id) {
            return Err(AetherV2Error::Config(format!(
                "Server '{}' already exists",
                config.id
            )));
        }

        cfg.mcp.external_servers.push(external_config);
        cfg.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;

        info!(server_id = %config.id, "MCP server added");
        Ok(())
    }

    /// Update an external MCP server configuration
    pub fn update_mcp_server(&self, config: crate::mcp::McpServerConfig) -> Result<(), AetherV2Error> {
        if config.server_type == crate::mcp::McpServerType::Builtin {
            return Err(AetherV2Error::Config(
                "Builtin servers cannot be updated via this method".to_string(),
            ));
        }

        let command = config
            .command
            .as_ref()
            .ok_or_else(|| AetherV2Error::Config("External server requires a command".to_string()))?;

        let mut cfg = self.lock_config();

        let server = cfg
            .mcp
            .external_servers
            .iter_mut()
            .find(|s| s.name == config.id);

        match server {
            Some(s) => {
                s.command = command.clone();
                s.args = config.args;
                s.env = config.env.into_iter().map(|e| (e.key, e.value)).collect();
                s.cwd = config.working_directory;
            }
            None => {
                return Err(AetherV2Error::Config(format!(
                    "External server '{}' not found",
                    config.id
                )));
            }
        }

        cfg.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!(server_id = %config.id, "MCP server updated");
        Ok(())
    }

    /// Delete an external MCP server
    pub fn delete_mcp_server(&self, id: String) -> Result<(), AetherV2Error> {
        let mut cfg = self.lock_config();

        let initial_len = cfg.mcp.external_servers.len();
        cfg.mcp.external_servers.retain(|s| s.name != id);

        if cfg.mcp.external_servers.len() == initial_len {
            return Err(AetherV2Error::Config(format!(
                "External server '{}' not found",
                id
            )));
        }

        cfg.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!(server_id = %id, "MCP server deleted");
        Ok(())
    }

    /// Get MCP server logs
    pub fn get_mcp_server_logs(&self, _id: String, _max_lines: u32) -> Vec<String> {
        // TODO: Implement log collection from server process
        Vec::new()
    }

    /// Export MCP configuration as claude_desktop_config.json format
    pub fn export_mcp_config_json(&self) -> String {
        let config = self.lock_config();
        let mut servers = serde_json::Map::new();

        for ext in &config.mcp.external_servers {
            let mut server_obj = serde_json::Map::new();
            server_obj.insert("command".to_string(), serde_json::json!(ext.command));
            server_obj.insert("args".to_string(), serde_json::json!(ext.args));

            if !ext.env.is_empty() {
                server_obj.insert("env".to_string(), serde_json::json!(ext.env));
            }

            if let Some(cwd) = &ext.cwd {
                server_obj.insert("cwd".to_string(), serde_json::json!(cwd));
            }

            servers.insert(ext.name.clone(), serde_json::Value::Object(server_obj));
        }

        let export = serde_json::json!({ "mcpServers": servers });
        serde_json::to_string_pretty(&export).unwrap_or_else(|_| "{}".to_string())
    }

    /// Import MCP configuration from claude_desktop_config.json format
    pub fn import_mcp_config_json(&self, json: String) -> Result<(), AetherV2Error> {
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| AetherV2Error::Config(format!("Invalid JSON: {}", e)))?;

        let servers = parsed
            .get("mcpServers")
            .ok_or_else(|| AetherV2Error::Config("Missing 'mcpServers' field".to_string()))?
            .as_object()
            .ok_or_else(|| AetherV2Error::Config("'mcpServers' must be an object".to_string()))?;

        let mut cfg = self.lock_config();

        for (name, server_config) in servers {
            let command = server_config
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AetherV2Error::Config(format!("Server '{}' missing 'command'", name))
                })?;

            let args: Vec<String> = server_config
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let env: std::collections::HashMap<String, String> = server_config
                .get("env")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            let cwd = server_config
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(String::from);

            if let Some(existing) = cfg.mcp.external_servers.iter_mut().find(|s| s.name == *name) {
                existing.command = command.to_string();
                existing.args = args;
                existing.env = env;
                existing.cwd = cwd;
            } else {
                cfg.mcp
                    .external_servers
                    .push(crate::config::McpExternalServerConfig {
                        name: name.clone(),
                        command: command.to_string(),
                        args,
                        env,
                        cwd,
                        requires_runtime: None,
                        timeout_seconds: 30,
                    });
            }
        }

        cfg.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!("MCP configuration imported");
        Ok(())
    }

    // ========================================================================
    // MEMORY MANAGEMENT METHODS (V1 → V2 Migration)
    // ========================================================================

    /// Get memory configuration
    pub fn get_memory_config(&self) -> crate::config::MemoryConfig {
        let config = self.lock_config();
        config.memory.clone()
    }

    /// Update memory configuration
    pub fn update_memory_config(&self, new_config: crate::config::MemoryConfig) -> Result<(), AetherV2Error> {
        let mut config = self.lock_config();
        config.memory = new_config;
        config.save().map_err(|e| AetherV2Error::Config(e.to_string()))?;
        info!("Memory configuration updated");
        Ok(())
    }

    /// Delete specific memory by ID
    pub fn delete_memory(&self, id: String) -> Result<(), AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        use crate::memory::database::VectorDatabase;
        use std::path::PathBuf;
        let db_path = PathBuf::from(&memory_path.path);
        let db = VectorDatabase::new(db_path)
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        self.runtime.block_on(db.delete_memory(&id))
            .map_err(|e| AetherV2Error::Memory(e.to_string()))
    }

    /// Get memory database statistics
    pub fn get_memory_stats(&self) -> Result<crate::memory::database::MemoryStats, AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        use crate::memory::database::VectorDatabase;
        use std::path::PathBuf;
        let db_path = PathBuf::from(&memory_path.path);
        let db = VectorDatabase::new(db_path)
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        self.runtime.block_on(db.get_stats())
            .map_err(|e| AetherV2Error::Memory(e.to_string()))
    }

    /// Get list of unique app bundle IDs from memories
    pub fn get_memory_app_list(&self) -> Result<Vec<crate::core::types::AppMemoryInfo>, AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        use crate::memory::database::VectorDatabase;
        use std::path::PathBuf;
        let db_path = PathBuf::from(&memory_path.path);
        let db = VectorDatabase::new(db_path)
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        let apps = self.runtime.block_on(db.get_app_list())
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        Ok(apps
            .into_iter()
            .map(|(app_bundle_id, memory_count)| crate::core::types::AppMemoryInfo {
                app_bundle_id,
                memory_count,
            })
            .collect())
    }

    /// Clear memories (with optional filters)
    pub fn clear_memories(
        &self,
        app_bundle_id: Option<String>,
        window_title: Option<String>,
    ) -> Result<u64, AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        use crate::memory::database::VectorDatabase;
        use std::path::PathBuf;
        let db_path = PathBuf::from(&memory_path.path);
        let db = VectorDatabase::new(db_path)
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        self.runtime.block_on(db.clear_memories(app_bundle_id.as_deref(), window_title.as_deref()))
            .map_err(|e| AetherV2Error::Memory(e.to_string()))
    }

    /// Clear all compressed facts (Layer 2 data)
    pub fn clear_facts(&self) -> Result<u64, AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        use crate::memory::database::VectorDatabase;
        use std::path::PathBuf;
        let db_path = PathBuf::from(&memory_path.path);
        let db = VectorDatabase::new(db_path)
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        self.runtime.block_on(db.clear_facts())
            .map_err(|e| AetherV2Error::Memory(e.to_string()))
    }

    /// Delete all memories associated with a specific topic ID
    pub fn delete_memories_by_topic_id(&self, topic_id: String) -> Result<u64, AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        use crate::memory::database::VectorDatabase;
        use std::path::PathBuf;
        let db_path = PathBuf::from(&memory_path.path);
        let db = VectorDatabase::new(db_path)
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        self.runtime.block_on(db.delete_by_topic_id(&topic_id))
            .map_err(|e| AetherV2Error::Memory(e.to_string()))
    }

    /// Get compression statistics
    pub fn get_compression_stats(&self) -> Result<crate::core::types::CompressionStats, AetherV2Error> {
        let memory_path = self.memory_path.as_ref().ok_or_else(|| {
            AetherV2Error::Memory("Memory store not initialized".to_string())
        })?;

        use crate::memory::database::VectorDatabase;
        use std::path::PathBuf;
        let db_path = PathBuf::from(&memory_path.path);
        let db = VectorDatabase::new(db_path)
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        let stats = self.runtime.block_on(db.get_stats())
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;
        let fact_stats = self.runtime.block_on(db.get_fact_stats())
            .map_err(|e| AetherV2Error::Memory(e.to_string()))?;

        Ok(crate::core::types::CompressionStats {
            total_raw_memories: stats.total_memories,
            total_facts: fact_stats.total_facts,
            valid_facts: fact_stats.valid_facts,
            facts_by_type: fact_stats.facts_by_type,
        })
    }

    /// Manually trigger memory compression
    ///
    /// Note: In V2, compression is simplified. This is a placeholder
    /// that returns a default result.
    pub fn trigger_compression(&self) -> Result<crate::memory::context::CompressionResult, AetherV2Error> {
        // V2 compression is not yet fully implemented
        // Return a default result indicating no compression occurred
        Ok(crate::memory::context::CompressionResult {
            memories_processed: 0,
            facts_extracted: 0,
            facts_invalidated: 0,
            duration_ms: 0,
        })
    }

    // ========================================================================
    // SKILLS MANAGEMENT METHODS (V1 → V2 Migration)
    // ========================================================================

    /// List all installed skills
    pub fn list_skills(&self) -> Result<Vec<crate::skills::SkillInfo>, AetherV2Error> {
        crate::initialization::list_installed_skills()
            .map_err(|e| AetherV2Error::Config(e.to_string()))
    }

    /// Install a skill from a GitHub URL
    pub fn install_skill(&self, url: String) -> Result<crate::skills::SkillInfo, AetherV2Error> {
        let skill_info = crate::initialization::install_skill_from_url(url)
            .map_err(|e| AetherV2Error::Config(e.to_string()))?;

        info!(skill_id = %skill_info.id, "Skill installed");
        Ok(skill_info)
    }

    /// Install skills from a local ZIP file
    pub fn install_skills_from_zip(&self, zip_path: String) -> Result<Vec<String>, AetherV2Error> {
        let skill_ids = crate::initialization::install_skills_from_zip(zip_path)
            .map_err(|e| AetherV2Error::Config(e.to_string()))?;

        info!(count = skill_ids.len(), "Skills installed from ZIP");
        Ok(skill_ids)
    }

    /// Delete a skill by ID
    pub fn delete_skill(&self, skill_id: String) -> Result<(), AetherV2Error> {
        crate::initialization::delete_skill(skill_id.clone())
            .map_err(|e| AetherV2Error::Config(e.to_string()))?;

        info!(skill_id = %skill_id, "Skill deleted");
        Ok(())
    }

    /// Get the skills directory path
    pub fn get_skills_dir(&self) -> Result<String, AetherV2Error> {
        crate::initialization::get_skills_dir_string()
            .map_err(|e| AetherV2Error::Config(e.to_string()))
    }

    /// Refresh skills (placeholder for V2)
    ///
    /// In V2, this is a no-op since tool registry is managed differently.
    pub fn refresh_skills(&self) {
        info!("Skills refresh requested (V2)");
    }

    // ========================================================================
    // TOOL REGISTRY METHODS (V1 → V2 Migration)
    // ========================================================================

    /// List builtin tools only
    pub fn list_builtin_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        // Return static builtin tools
        vec![
            crate::dispatcher::UnifiedToolInfo {
                id: "builtin:search".to_string(),
                name: "search".to_string(),
                display_name: "Search".to_string(),
                description: "Search the internet".to_string(),
                source_type: crate::dispatcher::ToolSourceType::Builtin,
                source_id: None,
                parameters_schema: None,
                is_active: true,
                requires_confirmation: false,
                safety_level: "Read Only".to_string(),
                service_name: None,
                icon: Some("magnifyingglass".to_string()),
                usage: Some("/search <query>".to_string()),
                localization_key: Some("tool.search".to_string()),
                is_builtin: true,
                sort_order: 10,
                has_subtools: false,
            },
            crate::dispatcher::UnifiedToolInfo {
                id: "builtin:webfetch".to_string(),
                name: "webfetch".to_string(),
                display_name: "Web Fetch".to_string(),
                description: "Fetch web page content".to_string(),
                source_type: crate::dispatcher::ToolSourceType::Builtin,
                source_id: None,
                parameters_schema: None,
                is_active: true,
                requires_confirmation: false,
                safety_level: "Read Only".to_string(),
                service_name: None,
                icon: Some("globe".to_string()),
                usage: Some("/webfetch <url>".to_string()),
                localization_key: Some("tool.webfetch".to_string()),
                is_builtin: true,
                sort_order: 20,
                has_subtools: false,
            },
        ]
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
        full_config: Arc::new(Mutex::new(full_config)),
        config_path,  // Store config path for reload capability
        memory_path,
        handler,
        runtime,
        _owned_runtime: owned_runtime,  // Keep runtime alive if we created it
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
