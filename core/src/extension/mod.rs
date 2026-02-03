//! Extension System - Plugin and Component Management
//!
//! This module provides a complete extension system for Aether, compatible with
//! Claude Code plugins while supporting enhanced features like TypeScript plugins
//! and npm package installation.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        ExtensionManager                                  │
//! │  - Orchestrates discovery, loading, registration, integration           │
//! └────────────────────────────┬────────────────────────────────────────────┘
//!                              │
//!          ┌───────────────────┼───────────────────┐
//!          ▼                   ▼                   ▼
//!     ConfigManager      ComponentLoader      ComponentRegistry
//!     (aether.jsonc)     (skills, agents)    (state management)
//!          │                   │                   │
//!          └───────────────────┼───────────────────┘
//!                              │
//!          ┌───────────────────┴───────────────────┐
//!          ▼                                       ▼
//!     PluginRuntime                           HookExecutor
//!     (Node.js, npm)                          (event hooks)
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::extension::{ExtensionManager, ExtensionConfig};
//!
//! // Create manager with default configuration
//! let manager = ExtensionManager::new(ExtensionConfig::default()).await?;
//!
//! // Load all extensions
//! manager.load_all().await?;
//!
//! // Get skills for LLM prompt injection
//! let skills = manager.get_auto_invocable_skills();
//!
//! // Execute a skill
//! let result = manager.execute_skill("my-plugin:hello", "World").await?;
//! ```

pub mod config;
pub mod discovery;
pub mod hooks;
mod plugin_loader;
pub mod runtime;
pub mod sync_api;

mod error;
mod loader;
pub mod manifest;
pub mod registry;
mod service_manager;
mod skill_tool;
mod template;
mod types;
pub mod watcher;

pub use error::*;
pub use loader::*;
pub use manifest::*;
pub use plugin_loader::PluginLoader;
pub use registry::*;
pub use service_manager::ServiceManager;
pub use skill_tool::{build_skill_tool_description, check_skill_permission, request_skill_permission_async};
pub use template::SkillTemplate;
pub use types::*;

// Re-export config types
pub use config::{AetherConfig, ConfigManager};

// Re-export sync API
pub use sync_api::SyncExtensionManager;

// Re-export new plugin system types (Phase 1)
pub use discovery::{discover_all, DiscoveryConfig as PluginDiscoveryConfig, PluginCandidate};
pub use manifest::PluginManifest;
pub use registry::{HookRegistration, PluginHookEvent, PluginRegistry, ToolRegistration};
pub use types::{PluginKind, PluginOrigin, PluginRecord, PluginStatus};

use crate::discovery::{DiscoveryConfig, DiscoveryManager};
use hooks::{HookContext, HookExecutor, HookResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

// =============================================================================
// Cache State
// =============================================================================

/// Cache state for lazy-loading
#[derive(Debug, Default)]
struct CacheState {
    /// Whether components have been loaded
    loaded: bool,
    /// When components were loaded (for potential expiration)
    loaded_at: Option<Instant>,
}

/// Extension system configuration
#[derive(Debug, Clone)]
pub struct ExtensionConfig {
    /// Discovery configuration
    pub discovery: DiscoveryConfig,

    /// Whether to enable Node.js plugin runtime
    pub enable_node_runtime: bool,

    /// Whether to auto-load extensions on startup
    pub auto_load: bool,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        Self {
            discovery: DiscoveryConfig::default(),
            enable_node_runtime: true,
            auto_load: true,
        }
    }
}

/// Extension Manager - main entry point for the extension system
pub struct ExtensionManager {
    /// Configuration
    #[allow(dead_code)]
    config: ExtensionConfig,

    /// Discovery manager
    discovery: DiscoveryManager,

    /// Config manager (aether.jsonc)
    config_manager: ConfigManager,

    /// Component registry
    registry: Arc<RwLock<ComponentRegistry>>,

    /// Component loader
    loader: ComponentLoader,

    /// Hook executor
    hook_executor: Arc<RwLock<HookExecutor>>,

    /// Cache state for lazy-loading
    cache_state: Arc<RwLock<CacheState>>,

    /// Plugin loader for runtime plugins (Node.js, WASM)
    plugin_loader: Arc<RwLock<PluginLoader>>,

    /// Plugin registry for runtime registrations
    plugin_registry: Arc<RwLock<PluginRegistry>>,

    /// Service lifecycle manager
    service_manager: Arc<RwLock<ServiceManager>>,
}

impl ExtensionManager {
    /// Create a new extension manager
    pub async fn new(config: ExtensionConfig) -> ExtensionResult<Self> {
        let discovery = DiscoveryManager::new(config.discovery.clone())?;
        let config_manager = ConfigManager::new(&discovery).await?;
        let registry = Arc::new(RwLock::new(ComponentRegistry::new()));
        let loader = ComponentLoader::new();
        let hook_executor = Arc::new(RwLock::new(HookExecutor::empty()));
        let cache_state = Arc::new(RwLock::new(CacheState::default()));
        let plugin_loader = Arc::new(RwLock::new(PluginLoader::new()));
        let plugin_registry = Arc::new(RwLock::new(PluginRegistry::new()));
        let service_manager = Arc::new(RwLock::new(ServiceManager::new()));

        Ok(Self {
            config,
            discovery,
            config_manager,
            registry,
            loader,
            hook_executor,
            cache_state,
            plugin_loader,
            plugin_registry,
            service_manager,
        })
    }

    /// Create with default configuration
    pub async fn with_defaults() -> ExtensionResult<Self> {
        Self::new(ExtensionConfig::default()).await
    }

    /// Load all extensions
    pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
        let mut summary = LoadSummary::default();

        // 1. Load skills
        let skill_dirs = self.discovery.discover_skill_dirs()?;
        for dir in skill_dirs {
            match self.loader.load_skill(&dir.path).await {
                Ok(skill) => {
                    self.registry.write().await.register_skill(skill);
                    summary.skills_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load skill from {:?}: {}", dir.path, e);
                    summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 2. Load commands
        let command_dirs = self.discovery.discover_command_dirs()?;
        for dir in command_dirs {
            match self.loader.load_command(&dir.path).await {
                Ok(cmd) => {
                    self.registry.write().await.register_command(cmd);
                    summary.commands_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load command from {:?}: {}", dir.path, e);
                    summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 3. Load agents
        let agent_dirs = self.discovery.discover_agent_dirs()?;
        for dir in agent_dirs {
            match self.loader.load_agent(&dir.path).await {
                Ok(agent) => {
                    self.registry.write().await.register_agent(agent);
                    summary.agents_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load agent from {:?}: {}", dir.path, e);
                    summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 4. Load plugins
        let plugin_dirs = self.discovery.discover_plugin_dirs()?;
        for dir in plugin_dirs {
            match self.loader.load_plugin(&dir.path).await {
                Ok(plugin) => {
                    // Register plugin hooks
                    if !plugin.hooks.is_empty() {
                        let mut executor = self.hook_executor.write().await;
                        for hook in plugin.hooks.clone() {
                            executor.add_hook(hook);
                            summary.hooks_loaded += 1;
                        }
                    }

                    // Register plugin components
                    let reg = &mut *self.registry.write().await;
                    for skill in plugin.skills.clone() {
                        reg.register_skill(skill);
                        summary.skills_loaded += 1;
                    }
                    for cmd in plugin.commands.clone() {
                        reg.register_command(cmd);
                        summary.commands_loaded += 1;
                    }
                    for agent in plugin.agents.clone() {
                        reg.register_agent(agent);
                        summary.agents_loaded += 1;
                    }
                    reg.register_plugin(plugin);
                    summary.plugins_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load plugin from {:?}: {}", dir.path, e);
                    summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 5. Load runtime plugins (Node.js, WASM)
        // Note: Runtime plugins are discovered separately and loaded via PluginLoader.
        // They register tools and hooks with the PluginRegistry (not ComponentRegistry).
        // For now, runtime plugin discovery is handled via separate API calls.
        // TODO: Add automatic discovery and loading of runtime plugins here.

        tracing::info!(
            "Extension loading complete: {} skills, {} commands, {} agents, {} plugins, {} hooks",
            summary.skills_loaded,
            summary.commands_loaded,
            summary.agents_loaded,
            summary.plugins_loaded,
            summary.hooks_loaded
        );

        // Update cache state
        {
            let mut state = self.cache_state.write().await;
            state.loaded = true;
            state.loaded_at = Some(Instant::now());
        }

        Ok(summary)
    }

    /// Ensure extensions are loaded (lazy-loading entry point)
    ///
    /// This method is idempotent - calling it multiple times only loads once.
    /// Use `reload()` to force a fresh load.
    pub async fn ensure_loaded(&self) -> ExtensionResult<()> {
        // Fast path: check if already loaded
        {
            let state = self.cache_state.read().await;
            if state.loaded {
                return Ok(());
            }
        }

        // Slow path: acquire write lock and load
        // Double-check after acquiring write lock to avoid race
        let state = self.cache_state.write().await;
        if state.loaded {
            return Ok(());
        }

        // Release lock before loading (load_all will acquire its own locks)
        drop(state);

        // Load all extensions
        self.load_all().await?;

        Ok(())
    }

    /// Force reload all extensions
    ///
    /// Clears the cache and reloads everything from disk.
    /// Useful for hot-reloading during development.
    pub async fn reload(&self) -> ExtensionResult<LoadSummary> {
        // Clear cache state
        {
            let mut state = self.cache_state.write().await;
            state.loaded = false;
            state.loaded_at = None;
        }

        // Clear registry
        self.registry.write().await.clear();

        // Clear hooks
        *self.hook_executor.write().await = HookExecutor::empty();

        // Reload everything
        self.load_all().await
    }

    /// Check if extensions have been loaded
    pub async fn is_loaded(&self) -> bool {
        self.cache_state.read().await.loaded
    }

    /// Get all skills (from enabled sources)
    pub async fn get_all_skills(&self) -> Vec<ExtensionSkill> {
        self.registry.read().await.get_all_skills()
    }

    /// Get auto-invocable skills (for LLM prompt injection)
    pub async fn get_auto_invocable_skills(&self) -> Vec<ExtensionSkill> {
        self.registry.read().await.get_auto_invocable_skills()
    }

    /// Get all commands
    pub async fn get_all_commands(&self) -> Vec<ExtensionCommand> {
        self.registry.read().await.get_all_commands()
    }

    /// Get all agents
    pub async fn get_all_agents(&self) -> Vec<ExtensionAgent> {
        self.registry.read().await.get_all_agents()
    }

    /// Get a specific skill by qualified name
    pub async fn get_skill(&self, qualified_name: &str) -> Option<ExtensionSkill> {
        self.registry.read().await.get_skill(qualified_name)
    }

    /// Get a specific command by name
    pub async fn get_command(&self, name: &str) -> Option<ExtensionCommand> {
        self.registry.read().await.get_command(name)
    }

    /// Get a specific agent by name
    pub async fn get_agent(&self, name: &str) -> Option<ExtensionAgent> {
        self.registry.read().await.get_agent(name)
    }

    /// Get the merged configuration
    pub fn get_config(&self) -> &AetherConfig {
        self.config_manager.get_config()
    }

    /// Get the discovery manager
    pub fn discovery(&self) -> &DiscoveryManager {
        &self.discovery
    }

    /// Get the Aether home directory
    pub fn aether_home(&self) -> ExtensionResult<PathBuf> {
        Ok(self.discovery.aether_home()?)
    }

    // =========================================================================
    // Hook Execution
    // =========================================================================

    /// Execute hooks for an event
    pub async fn execute_hooks(
        &self,
        event: HookEvent,
        context: &HookContext,
    ) -> ExtensionResult<HookResult> {
        self.hook_executor.read().await.execute(event, context).await
    }

    /// Get the number of registered hooks
    pub async fn hook_count(&self) -> usize {
        self.hook_executor.read().await.hook_count()
    }

    /// Convert V2 TOML hook declarations to HookConfig
    ///
    /// This method transforms hook definitions from `aether.plugin.toml` manifests
    /// into `HookConfig` objects that can be executed by the hook system.
    ///
    /// # Event Mapping
    ///
    /// TOML event strings are mapped to `HookEvent` variants:
    /// - `"before_agent_start"` / `"session_start"` -> `SessionStart`
    /// - `"before_tool_call"` -> `PreToolUse`
    /// - `"after_tool_call"` -> `PostToolUse`
    /// - `"before_message_send"` -> `ChatMessage`
    /// - `"after_message_send"` -> `ChatResponse`
    /// - `"on_error"` -> `PostToolUseFailure`
    /// - `"session_end"` -> `SessionEnd`
    ///
    /// # Arguments
    ///
    /// * `hooks` - Slice of TOML hook sections from the plugin manifest
    /// * `plugin_name` - Name of the plugin (for logging and identification)
    /// * `plugin_root` - Root directory of the plugin (for variable substitution)
    ///
    /// # Returns
    ///
    /// A vector of `HookConfig` objects ready for registration with the hook executor.
    fn convert_v2_hooks(
        &self,
        hooks: &[crate::extension::manifest::HookSection],
        plugin_name: &str,
        plugin_root: &Path,
    ) -> Vec<HookConfig> {
        hooks
            .iter()
            .map(|h| {
                let event = match h.event.as_str() {
                    "before_agent_start" => HookEvent::SessionStart,
                    "before_tool_call" => HookEvent::PreToolUse,
                    "after_tool_call" => HookEvent::PostToolUse,
                    "before_message_send" => HookEvent::ChatMessage,
                    "after_message_send" => HookEvent::ChatResponse,
                    "on_error" => HookEvent::PostToolUseFailure,
                    "session_start" => HookEvent::SessionStart,
                    "session_end" => HookEvent::SessionEnd,
                    _ => HookEvent::PreToolUse, // Default fallback
                };

                HookConfig {
                    event,
                    kind: HookKind::from_str(&h.kind),
                    priority: HookPriority::from_str(&h.priority),
                    matcher: h.filter.clone(),
                    actions: vec![], // Runtime hooks use handler, not actions
                    plugin_name: plugin_name.to_string(),
                    plugin_root: plugin_root.to_path_buf(),
                    handler: h.handler.clone(),
                }
            })
            .collect()
    }

    // =========================================================================
    // Runtime Plugin Operations (Node.js, WASM)
    // =========================================================================

    /// Call a tool on a runtime plugin.
    ///
    /// This method calls a tool handler on a loaded runtime plugin (Node.js or WASM).
    /// The plugin must have been loaded via the PluginLoader.
    ///
    /// # Lock Behavior
    ///
    /// This method acquires a **write lock** on the plugin loader because the underlying
    /// IPC call requires mutable access to the Node.js process stdin/stdout streams.
    /// Node.js IPC is inherently sequential - multiple concurrent writes to stdin would
    /// corrupt the message framing.
    ///
    /// For high-throughput scenarios with many concurrent tool calls, consider:
    /// - Running multiple instances of the same plugin (load-balanced)
    /// - Using a bounded queue to throttle concurrent calls
    /// - Implementing request batching to reduce the number of separate tool calls
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin containing the tool
    /// * `handler` - The handler function name to call
    /// * `args` - The arguments to pass to the handler
    ///
    /// # Returns
    ///
    /// * `Ok(serde_json::Value)` - The result from the tool handler
    /// * `Err(ExtensionError::PluginNotFound)` - If the plugin is not loaded
    pub async fn call_plugin_tool(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        self.plugin_loader
            .write()
            .await
            .call_tool(plugin_id, handler, args)
    }

    /// Execute a hook handler on a runtime plugin.
    ///
    /// This method executes a hook handler on a loaded runtime plugin (Node.js or WASM).
    /// The plugin must have been loaded via the PluginLoader.
    ///
    /// # Lock Behavior
    ///
    /// This method acquires a **write lock** on the plugin loader because the underlying
    /// IPC call requires mutable access to the Node.js process stdin/stdout streams.
    /// Node.js IPC is inherently sequential - multiple concurrent writes to stdin would
    /// corrupt the message framing.
    ///
    /// For high-throughput scenarios with many concurrent hook executions, consider:
    /// - Running multiple instances of the same plugin (load-balanced)
    /// - Using a bounded queue to throttle concurrent calls
    /// - Implementing request batching for bulk event handling
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin containing the hook
    /// * `handler` - The handler function name to call
    /// * `event_data` - The event data to pass to the handler
    ///
    /// # Returns
    ///
    /// * `Ok(serde_json::Value)` - The result from the hook handler
    /// * `Err(ExtensionError::PluginNotFound)` - If the plugin is not loaded
    pub async fn execute_plugin_hook(
        &self,
        plugin_id: &str,
        handler: &str,
        event_data: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        self.plugin_loader
            .write()
            .await
            .execute_hook(plugin_id, handler, event_data)
    }

    /// Execute a direct command on a runtime plugin.
    ///
    /// This method executes a direct command handler on a loaded runtime plugin
    /// (Node.js or WASM). Direct commands are user-triggered commands that execute
    /// immediately without LLM involvement (e.g., `/status`, `/clear`, `/version`).
    ///
    /// # Lock Behavior
    ///
    /// This method acquires a **write lock** on the plugin loader because the underlying
    /// IPC call requires mutable access to the Node.js process stdin/stdout streams.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin containing the command handler
    /// * `handler` - The handler function name to call
    /// * `args` - The arguments to pass to the handler
    ///
    /// # Returns
    ///
    /// * `Ok(DirectCommandResult)` - The result from the command handler
    /// * `Err(ExtensionError::PluginNotFound)` - If the plugin is not loaded
    pub async fn execute_plugin_command(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<DirectCommandResult> {
        self.plugin_loader
            .write()
            .await
            .execute_command(plugin_id, handler, args)
    }

    /// Get the plugin registry for runtime plugins.
    ///
    /// This provides read access to the registry containing tools, hooks,
    /// and other registrations from runtime plugins (Node.js, WASM).
    pub async fn get_plugin_registry(&self) -> tokio::sync::RwLockReadGuard<'_, PluginRegistry> {
        self.plugin_registry.read().await
    }

    /// Get the plugin loader for runtime plugins.
    ///
    /// This provides read access to the plugin loader for checking plugin status.
    pub async fn get_plugin_loader(&self) -> tokio::sync::RwLockReadGuard<'_, PluginLoader> {
        self.plugin_loader.read().await
    }

    /// Load a runtime plugin from a manifest.
    ///
    /// This method loads a plugin into the appropriate runtime (Node.js or WASM)
    /// based on its kind, and registers its tools and hooks with the plugin registry.
    ///
    /// # Arguments
    ///
    /// * `manifest` - The plugin manifest containing metadata and entry point
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the plugin was loaded successfully
    /// * `Err(ExtensionError)` if loading failed
    pub async fn load_runtime_plugin(&self, manifest: &PluginManifest) -> ExtensionResult<()> {
        let mut loader = self.plugin_loader.write().await;
        let mut registry = self.plugin_registry.write().await;
        loader.load_plugin(manifest, &mut registry)
    }

    /// Unload a runtime plugin.
    ///
    /// This method unloads a plugin from its runtime and removes it from tracking.
    /// Note: This does not automatically unregister tools/hooks from the registry.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin to unload
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the plugin was unloaded successfully
    /// * `Err(ExtensionError::PluginNotFound)` if the plugin is not loaded
    pub async fn unload_runtime_plugin(&self, plugin_id: &str) -> ExtensionResult<()> {
        self.plugin_loader.write().await.unload_plugin(plugin_id)
    }

    // =========================================================================
    // Service Lifecycle
    // =========================================================================

    /// Start a background service.
    ///
    /// This method starts a service registered by a plugin. The service must be
    /// registered in the plugin registry before it can be started.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin that registered the service
    /// * `service_id` - The ID of the service to start
    ///
    /// # Returns
    ///
    /// * `Ok(ServiceInfo)` - The service info after starting
    /// * `Err(ExtensionError)` - If the service was not found or failed to start
    pub async fn start_service(
        &self,
        plugin_id: &str,
        service_id: &str,
    ) -> ExtensionResult<ServiceInfo> {
        // Find the service registration in the plugin registry
        let registration = {
            let registry = self.plugin_registry.read().await;
            // Look through all services to find one matching both plugin_id and service_id
            registry
                .list_services()
                .into_iter()
                .find(|s| s.plugin_id == plugin_id && s.id == service_id)
                .cloned()
                .ok_or_else(|| {
                    ExtensionError::ServiceNotFound(format!(
                        "{}:{}", plugin_id, service_id
                    ))
                })?
        };

        // Start the service using the service manager
        let mut service_manager = self.service_manager.write().await;
        let mut loader = self.plugin_loader.write().await;
        service_manager.start_service(&registration, &mut loader)
    }

    /// Stop a background service.
    ///
    /// This method stops a running service. The service must be registered in
    /// the plugin registry.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin that registered the service
    /// * `service_id` - The ID of the service to stop
    ///
    /// # Returns
    ///
    /// * `Ok(ServiceInfo)` - The service info after stopping
    /// * `Err(ExtensionError)` - If the service was not found or failed to stop
    pub async fn stop_service(
        &self,
        plugin_id: &str,
        service_id: &str,
    ) -> ExtensionResult<ServiceInfo> {
        // Find the service registration in the plugin registry
        let registration = {
            let registry = self.plugin_registry.read().await;
            // Look through all services to find one matching both plugin_id and service_id
            registry
                .list_services()
                .into_iter()
                .find(|s| s.plugin_id == plugin_id && s.id == service_id)
                .cloned()
                .ok_or_else(|| {
                    ExtensionError::ServiceNotFound(format!(
                        "{}:{}", plugin_id, service_id
                    ))
                })?
        };

        // Stop the service using the service manager
        let mut service_manager = self.service_manager.write().await;
        let mut loader = self.plugin_loader.write().await;
        service_manager.stop_service(&registration, &mut loader)
    }

    /// Get service status.
    ///
    /// Returns the current state of a service if it has been started at least once.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin that registered the service
    /// * `service_id` - The ID of the service
    ///
    /// # Returns
    ///
    /// * `Some(ServiceInfo)` - If the service has been tracked
    /// * `None` - If the service has never been started
    pub async fn get_service_status(
        &self,
        plugin_id: &str,
        service_id: &str,
    ) -> Option<ServiceInfo> {
        self.service_manager
            .read()
            .await
            .get_service(plugin_id, service_id)
            .cloned()
    }

    /// List all services tracked by the service manager.
    ///
    /// Returns information about all services that have been started at least once,
    /// regardless of their current state.
    pub async fn list_services(&self) -> Vec<ServiceInfo> {
        self.service_manager
            .read()
            .await
            .list_services()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Get the count of running services.
    pub async fn running_service_count(&self) -> usize {
        self.service_manager.read().await.running_count()
    }

    /// Get the service manager for direct access.
    ///
    /// This provides read access to the service manager for advanced use cases.
    pub async fn get_service_manager(&self) -> tokio::sync::RwLockReadGuard<'_, ServiceManager> {
        self.service_manager.read().await
    }

    // =========================================================================
    // Skill/Command Execution
    // =========================================================================

    /// Execute a skill with arguments
    ///
    /// Returns the skill content with $ARGUMENTS replaced
    pub async fn execute_skill(
        &self,
        qualified_name: &str,
        arguments: &str,
    ) -> ExtensionResult<String> {
        let skill = self.get_skill(qualified_name).await.ok_or_else(|| {
            ExtensionError::SkillNotFound(qualified_name.to_string())
        })?;

        Ok(skill.with_arguments(arguments))
    }

    /// Execute a command with arguments
    ///
    /// Returns the command content with $ARGUMENTS replaced
    pub async fn execute_command(
        &self,
        name: &str,
        arguments: &str,
    ) -> ExtensionResult<String> {
        let cmd = self.get_command(name).await.ok_or_else(|| {
            ExtensionError::CommandNotFound(name.to_string())
        })?;

        Ok(cmd.with_arguments(arguments))
    }

    // =========================================================================
    // Skill Tool (LLM-callable)
    // =========================================================================

    /// Invoke a skill as an LLM tool
    ///
    /// This is the primary method for LLM to invoke skills. It:
    /// 1. Ensures extensions are loaded
    /// 2. Checks permissions
    /// 3. Renders templates (including file references)
    /// 4. Returns structured result with metadata
    ///
    /// # Arguments
    /// * `qualified_name` - Skill name (e.g., "my-skill" or "plugin:skill")
    /// * `arguments` - Arguments to substitute for $ARGUMENTS
    /// * `ctx` - Execution context with session and permission info
    ///
    /// # Returns
    /// * `SkillToolResult` with rendered content, base directory, and metadata
    pub async fn invoke_skill_tool(
        &self,
        qualified_name: &str,
        arguments: &str,
        ctx: &SkillContext,
    ) -> ExtensionResult<SkillToolResult> {
        // Ensure extensions are loaded
        self.ensure_loaded().await?;

        // Get the skill
        let skill = self.get_skill(qualified_name).await.ok_or_else(|| {
            ExtensionError::SkillNotFound(qualified_name.to_string())
        })?;

        // Invoke using skill_tool module
        skill_tool::invoke_skill(&skill, arguments, ctx).await
    }

    /// Get skill tool description for LLM
    ///
    /// Generates a description of available skills in XML format,
    /// suitable for inclusion in tool definitions.
    pub async fn get_skill_tool_description(&self) -> String {
        self.ensure_loaded().await.ok();
        let skills = self.get_auto_invocable_skills().await;
        skill_tool::build_skill_tool_description(&skills)
    }

    // =========================================================================
    // MCP Server Access
    // =========================================================================

    /// Get all MCP server configurations from loaded plugins
    pub async fn get_mcp_servers(&self) -> HashMap<String, McpServerConfig> {
        let mut servers = HashMap::new();

        // Get from aether.jsonc config
        if let Some(mcp) = self.config_manager.get_mcp_servers() {
            for (name, config) in mcp {
                // Convert McpConfig to McpServerConfig
                match config {
                    config::McpConfig::Local { command, environment, .. } => {
                        // command is a Vec<String> where first element is the command
                        // and the rest are arguments
                        let (cmd, args) = if command.is_empty() {
                            (String::new(), Vec::new())
                        } else {
                            (command[0].clone(), command[1..].to_vec())
                        };
                        servers.insert(name.clone(), McpServerConfig {
                            command: cmd,
                            args,
                            env: environment.clone(),
                        });
                    }
                    config::McpConfig::Remote { url, .. } => {
                        // Remote servers need a different approach
                        tracing::debug!("Remote MCP server {} at {} not yet supported", name, url);
                    }
                }
            }
        }

        // Get from loaded plugins
        for plugin in self.registry.read().await.get_all_plugins() {
            for (name, config) in &plugin.mcp_servers {
                let full_name = format!("{}:{}", plugin.name, name);
                servers.insert(full_name, config.clone());
            }
        }

        servers
    }

    // =========================================================================
    // Plugin Info
    // =========================================================================

    /// Get all loaded plugin info
    pub async fn get_plugin_info(&self) -> Vec<PluginInfo> {
        self.registry
            .read()
            .await
            .get_all_plugins()
            .into_iter()
            .map(|p| p.info())
            .collect()
    }

    /// Get a specific plugin by name
    pub async fn get_plugin(&self, name: &str) -> Option<ExtensionPlugin> {
        self.registry.read().await.get_plugin(name).cloned()
    }

    // =========================================================================
    // Primary/Sub-Agent Support
    // =========================================================================

    /// Get all primary agents (can be selected by user)
    pub async fn get_primary_agents(&self) -> Vec<ExtensionAgent> {
        self.registry
            .read()
            .await
            .get_all_agents()
            .into_iter()
            .filter(|a| a.is_primary() && !a.hidden)
            .collect()
    }

    /// Get all sub-agents (can be delegated to)
    pub async fn get_sub_agents(&self) -> Vec<ExtensionAgent> {
        self.registry
            .read()
            .await
            .get_all_agents()
            .into_iter()
            .filter(|a| a.is_subagent())
            .collect()
    }

    // =========================================================================
    // Configuration Access
    // =========================================================================

    /// Get the default model from configuration
    pub fn get_default_model(&self) -> Option<&str> {
        self.config_manager.get_config().model.as_deref()
    }

    /// Get the small/fast model from configuration
    pub fn get_small_model(&self) -> Option<&str> {
        self.config_manager.get_config().small_model.as_deref()
    }

    /// Get the default agent from configuration
    pub fn get_default_agent(&self) -> Option<&str> {
        self.config_manager.get_config().default_agent.as_deref()
    }

    /// Get all custom instructions
    pub fn get_instructions(&self) -> Vec<&str> {
        self.config_manager
            .get_config()
            .instructions
            .as_ref()
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }
}

/// Summary of extension loading
#[derive(Debug, Default)]
pub struct LoadSummary {
    /// Number of skills loaded
    pub skills_loaded: usize,
    /// Number of commands loaded
    pub commands_loaded: usize,
    /// Number of agents loaded
    pub agents_loaded: usize,
    /// Number of plugins loaded
    pub plugins_loaded: usize,
    /// Number of hooks loaded
    pub hooks_loaded: usize,
    /// Errors encountered during loading
    pub errors: Vec<String>,
}

impl LoadSummary {
    /// Check if loading was successful (no errors)
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }

    /// Total components loaded
    pub fn total_loaded(&self) -> usize {
        self.skills_loaded + self.commands_loaded + self.agents_loaded + self.plugins_loaded
    }
}

// =============================================================================
// Utility Functions
// =============================================================================

/// Build skill instructions for LLM prompt injection
///
/// Formats a list of skills into markdown instructions that can be
/// appended to the system prompt to inform the LLM about available skills.
pub fn build_skill_instructions(skills: &[ExtensionSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    output.push_str("\n\n## Available Plugin Skills\n\n");
    output.push_str("You have access to the following plugin skills. ");
    output.push_str("Use them when they match the user's intent:\n\n");

    for skill in skills {
        output.push_str(&format!(
            "### /{}\n**Description**: {}\n\n{}\n\n---\n\n",
            skill.qualified_name(),
            skill.description,
            skill.content
        ));
    }

    output
}

/// Check if a directory is a valid plugin directory
///
/// A valid plugin has `.claude-plugin/plugin.json`.
pub fn is_valid_plugin_dir(path: &std::path::Path) -> bool {
    path.join(".claude-plugin").join("plugin.json").exists()
}

/// Get the default plugins directory
///
/// Returns `~/.aether/plugins/`
pub fn default_plugins_dir() -> std::path::PathBuf {
    crate::discovery::aether_plugins_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("~/.aether/plugins"))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_extension_manager_has_plugin_loader() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Calling with nonexistent plugin should return PluginNotFound error
        let result = manager
            .call_plugin_tool("nonexistent", "handler", serde_json::json!({}))
            .await;
        assert!(result.is_err());

        // Verify the error is PluginNotFound
        match result {
            Err(ExtensionError::PluginNotFound(id)) => {
                assert_eq!(id, "nonexistent");
            }
            other => {
                panic!("Expected PluginNotFound error, got: {:?}", other);
            }
        }
    }

    #[tokio::test]
    async fn test_extension_manager_has_plugin_registry() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Should be able to access the plugin registry
        let registry = manager.get_plugin_registry().await;

        // Registry should be empty initially
        assert!(registry.list_plugins().is_empty());
        assert!(registry.list_tools().is_empty());
    }

    #[tokio::test]
    async fn test_extension_manager_execute_plugin_hook_nonexistent() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Calling with nonexistent plugin should return PluginNotFound error
        let result = manager
            .execute_plugin_hook("nonexistent", "onEvent", serde_json::json!({"test": true}))
            .await;
        assert!(result.is_err());

        // Verify the error is PluginNotFound
        match result {
            Err(ExtensionError::PluginNotFound(id)) => {
                assert_eq!(id, "nonexistent");
            }
            other => {
                panic!("Expected PluginNotFound error, got: {:?}", other);
            }
        }
    }

    #[tokio::test]
    async fn test_extension_manager_get_plugin_loader() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Should be able to access the plugin loader
        let loader = manager.get_plugin_loader().await;

        // No runtime should be active initially
        assert!(!loader.is_any_runtime_active());
        assert!(loader.loaded_plugin_ids().is_empty());
    }

    #[tokio::test]
    async fn test_extension_manager_has_service_manager() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Should be able to access the service manager
        let service_manager = manager.get_service_manager().await;

        // No services should be running initially
        assert_eq!(service_manager.running_count(), 0);
        assert_eq!(service_manager.total_count(), 0);
    }

    #[tokio::test]
    async fn test_extension_manager_list_services_empty() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Should return empty list when no services have been started
        let services = manager.list_services().await;
        assert!(services.is_empty());
    }

    #[tokio::test]
    async fn test_extension_manager_running_service_count() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Should be 0 when no services are running
        assert_eq!(manager.running_service_count().await, 0);
    }

    #[tokio::test]
    async fn test_extension_manager_get_service_status_not_found() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Should return None for nonexistent service
        let status = manager.get_service_status("nonexistent-plugin", "nonexistent-service").await;
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_extension_manager_start_service_not_registered() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Starting a service that is not registered should return ServiceNotFound
        let result = manager.start_service("nonexistent-plugin", "nonexistent-service").await;
        assert!(result.is_err());

        match result {
            Err(ExtensionError::ServiceNotFound(id)) => {
                assert_eq!(id, "nonexistent-plugin:nonexistent-service");
            }
            other => {
                panic!("Expected ServiceNotFound error, got: {:?}", other);
            }
        }
    }

    #[tokio::test]
    async fn test_extension_manager_stop_service_not_registered() {
        let manager = ExtensionManager::with_defaults().await.unwrap();

        // Stopping a service that is not registered should return ServiceNotFound
        let result = manager.stop_service("nonexistent-plugin", "nonexistent-service").await;
        assert!(result.is_err());

        match result {
            Err(ExtensionError::ServiceNotFound(id)) => {
                assert_eq!(id, "nonexistent-plugin:nonexistent-service");
            }
            other => {
                panic!("Expected ServiceNotFound error, got: {:?}", other);
            }
        }
    }
}
