//! Extension System - Plugin and Skill Management
//!
//! This module provides a unified extension system for Aleph.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        ExtensionManager                                │
//! │  - Orchestrates discovery, loading, registration, integration          │
//! └────────────────────────────┬───────────────────────────────────────────┘
//!                              │
//!          ┌───────────────────┼───────────────────┐
//!          ▼                   ▼                   ▼
//!     PluginRegistry      PluginLoader        SkillSystem
//!   (unified registry)  (Node.js, WASM)    (skills, agents)
//!          │                   │                   │
//!          └───────────────────┼───────────────────┘
//!                              │
//!                              │
//!                              ▼
//!                        HookExecutor
//!                       (unified hooks)
//! ```

pub mod component_id;
pub mod config;
pub mod discovery;
pub mod hooks;
mod loader;
pub mod marketplace;
pub mod runtime;
pub mod scope;
pub mod sync_api;
pub mod validation;

pub mod capability;
pub mod registrar;

mod channel_manager;
mod error;
mod http_handler;
pub mod manifest;
pub mod mcp_config;
mod plugin_ops;
mod provider_adapter;
pub mod registry;
mod service_manager;
mod service_ops;
mod skill_ops;
mod skill_tool;
mod template;
mod types;
pub mod watcher;

pub use channel_manager::{ChannelHandle, ChannelManager};
pub use component_id::ComponentId;
pub use error::*;
pub use http_handler::{match_path, PluginHttpHandler};
pub use loader::PluginLoader;
pub use manifest::*;
pub use provider_adapter::PluginProviderAdapter;
pub use registry::*;
pub use service_manager::ServiceManager;
pub use skill_tool::{check_skill_permission, request_skill_permission_async};
pub use template::SkillTemplate;
pub use types::*;

// Re-export config types
pub use config::{AlephConfig, ConfigManager};

// Re-export sync API
pub use sync_api::SyncExtensionManager;

// Re-export new plugin system types (Phase 1)
pub use capability::{CapabilityDeclaration, CapabilitySource, SourceFormat, Tier};
pub use discovery::{discover_all, DiscoveryConfig as PluginDiscoveryConfig, PluginCandidate};
pub use manifest::PluginManifest;
pub use registrar::CapabilityApi;
pub use registry::{HookRegistration, PluginRegistry, ToolRegistration};
pub use types::{PluginKind, PluginOrigin, PluginRecord, PluginStatus};

use crate::discovery::{DiscoveryConfig, DiscoveryManager};
use crate::sync_primitives::Arc;
use hooks::HookExecutor;
use manifest::adapter::AdapterRegistry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock as StdRwLock;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

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

    /// Whether to auto-load extensions on startup
    pub auto_load: bool,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        Self {
            discovery: DiscoveryConfig::default(),
            auto_load: true,
        }
    }
}

/// Extension Manager - main entry point for the extension system
pub struct ExtensionManager {
    /// Discovery manager
    discovery: DiscoveryManager,

    /// Config manager (aleph.jsonc)
    config_manager: ConfigManager,

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

    /// Adapter registry for capability-driven manifest parsing
    adapter_registry: AdapterRegistry,

    /// Skill System v2 (independent bounded context)
    skill_system: crate::skill::SkillSystem,

    /// Active plugin tools keyed by short tool name.
    ///
    /// This is a sync snapshot so runtime systems like the agent loop can
    /// cheaply read tool metadata and revision state without awaiting locks.
    active_plugin_tools: Arc<StdRwLock<HashMap<String, ToolRegistration>>>,

    /// Monotonic revision for active plugin tool snapshot changes.
    plugin_tool_revision: Arc<AtomicU64>,

    /// Guard to serialize concurrent load_all() calls
    load_guard: Mutex<()>,
}

impl ExtensionManager {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Create a new extension manager
    pub async fn new(config: ExtensionConfig) -> ExtensionResult<Self> {
        let discovery = DiscoveryManager::new(config.discovery.clone())?;
        let config_manager = ConfigManager::new(&discovery).await?;
        let hook_executor = Arc::new(RwLock::new(HookExecutor::empty()));
        let cache_state = Arc::new(RwLock::new(CacheState::default()));
        let plugin_loader = Arc::new(RwLock::new(PluginLoader::new()));
        let plugin_registry = Arc::new(RwLock::new(PluginRegistry::new()));
        let service_manager = Arc::new(RwLock::new(ServiceManager::new()));
        let adapter_registry = AdapterRegistry::with_defaults();

        Ok(Self {
            discovery,
            config_manager,
            hook_executor,
            cache_state,
            plugin_loader,
            plugin_registry,
            service_manager,
            adapter_registry,
            skill_system: crate::skill::SkillSystem::new(),
            active_plugin_tools: Arc::new(StdRwLock::new(HashMap::new())),
            plugin_tool_revision: Arc::new(AtomicU64::new(0)),
            load_guard: Mutex::new(()),
        })
    }

    /// Create with default configuration
    pub async fn with_defaults() -> ExtensionResult<Self> {
        Self::new(ExtensionConfig::default()).await
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Load all extensions.
    pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
        let mut summary = LoadSummary::default();

        // Collect all plugin directories from discovery
        let plugin_dirs = self.collect_plugin_dirs()?;

        // Clear and rebuild registry
        let mut mcp_configs: Vec<(String, McpServerConfig)> = Vec::new();
        {
            let mut registry = self.plugin_registry.write().await;
            registry.clear();

            for dir_path in &plugin_dirs {
                match self.adapter_registry.parse_dir(dir_path) {
                    Ok(output) => {
                        // Collect MCP configs (no-op in CapabilityApi dispatch)
                        for cap in &output.capabilities {
                            if let crate::extension::capability::CapabilityDeclaration::McpServer(
                                mcp,
                            ) = cap
                            {
                                let server_name = output.plugin_id.clone();
                                mcp_configs.push((server_name, mcp.clone()));
                            }
                        }

                        // Build plugin record from adapter output
                        let record = PluginRecord::from_adapter_output(&output, dir_path.clone());
                        let plugin_id = output.plugin_id.clone();
                        registry.register_plugin(record);

                        // Register all capabilities via CapabilityApi
                        let mut api = registrar::CapabilityApi::new(
                            &mut registry,
                            plugin_id.clone(),
                            output.permissions,
                        );
                        for cap in output.capabilities {
                            if let Err(e) = api.register_capability(cap) {
                                tracing::debug!(
                                    "Failed to register capability for plugin {}: {}",
                                    plugin_id,
                                    e
                                );
                            }
                        }

                        summary.plugins_loaded += 1;
                    }
                    Err(e) => {
                        tracing::debug!("Failed to parse plugin dir {:?}: {}", dir_path, e);
                        summary
                            .errors
                            .push(format!("{}: {}", dir_path.display(), e));
                    }
                }
            }

            // Count loaded skills/commands/agents from registry
            summary.skills_loaded = registry.list_skills().len();
            summary.agents_loaded = registry.list_agents().len();
            summary.hooks_loaded = registry.list_hooks().len();
        }

        // Sync hooks from registry to HookExecutor
        self.sync_hooks_from_registry().await;

        // Initialize SkillSystem with discovered skill directories
        let mut skill_dirs: Vec<PathBuf> = Vec::new();
        // Skill dirs from discovery (includes ~/.aleph/skills/ where bundled + user skills live)
        for d in self.discovery.discover_skill_dirs().unwrap_or_default() {
            skill_dirs.push(d.path);
        }
        if let Err(e) = self.skill_system.init(skill_dirs).await {
            tracing::warn!("Failed to init skill system: {}", e);
        }

        self.refresh_active_plugin_tools().await;

        let mut cache = self.cache_state.write().await;
        cache.loaded = true;
        cache.loaded_at = Some(Instant::now());

        tracing::info!(
            "Extension loading complete: {} skills, {} agents, {} plugins, {} hooks",
            summary.skills_loaded,
            summary.agents_loaded,
            summary.plugins_loaded,
            summary.hooks_loaded,
        );

        Ok(summary)
    }

    /// Ensure extensions are loaded (lazy-loading entry point).
    ///
    /// Uses a Mutex guard to serialize concurrent calls — concurrent callers
    /// wait for the first load to complete rather than seeing partial data.
    pub async fn ensure_loaded(&self) -> ExtensionResult<()> {
        // Fast path: already loaded (no lock contention)
        if self.cache_state.read().await.loaded {
            return Ok(());
        }

        // Serialize concurrent loads — other callers block here until first load completes
        let _guard = self.load_guard.lock().await;

        // Re-check after acquiring guard (another task may have loaded while we waited)
        if self.cache_state.read().await.loaded {
            return Ok(());
        }

        // load_all() sets cache_state.loaded = true on success
        self.load_all().await?;
        Ok(())
    }

    /// Force reload all extensions
    pub async fn reload(&self) -> ExtensionResult<LoadSummary> {
        {
            let mut state = self.cache_state.write().await;
            state.loaded = false;
            state.loaded_at = None;
        }

        // Reset hook executor (registry.clear() is called inside load_all)
        *self.hook_executor.write().await = HookExecutor::empty();

        self.load_all().await
    }

    /// Collect all plugin directories from discovery, deduplicating by canonical path.
    ///
    /// Origin is not tracked here — `PluginRecord::from_adapter_output` derives it
    /// from the `AdapterOutput::source` field set during manifest parsing.
    fn collect_plugin_dirs(&self) -> ExtensionResult<Vec<PathBuf>> {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        let mut result = Vec::new();

        // Collect from all discovery sources: skills, commands, agents, plugins
        let all_dirs = [
            self.discovery.discover_skill_dirs(),
            self.discovery.discover_command_dirs(),
            self.discovery.discover_agent_dirs(),
            self.discovery.discover_plugins(),
        ];

        for dirs in all_dirs.into_iter().flatten() {
            for d in dirs {
                let canonical = d.path.canonicalize().unwrap_or_else(|_| d.path.clone());
                if seen.insert(canonical) {
                    result.push(d.path);
                }
            }
        }

        Ok(result)
    }

    /// Sync hooks from PluginRegistry to HookExecutor.
    ///
    /// Reads HookRegistration entries from the registry and converts them
    /// to HookConfig entries that HookExecutor understands.
    async fn sync_hooks_from_registry(&self) {
        let registry = self.plugin_registry.read().await;
        let hook_regs = registry.list_hooks();

        let mut executor = self.hook_executor.write().await;
        // HookExecutor was already reset (via load_all's clear path or reload).
        // Convert HookRegistration → HookConfig for the executor.
        for hr in hook_regs {
            let hook_config = HookConfig {
                event: hr.event,
                kind: HookKind::default(),
                priority: match hr.priority {
                    i if i <= HookPriority::System.as_i32() => HookPriority::System,
                    i if i <= HookPriority::High.as_i32() => HookPriority::High,
                    i if i >= HookPriority::Low.as_i32() => HookPriority::Low,
                    _ => HookPriority::Normal,
                },
                matcher: None,
                actions: vec![],
                plugin_name: hr.plugin_id.clone(),
                plugin_root: PathBuf::new(),
                handler: Some(hr.handler.clone()),
            };
            executor.add_hook(hook_config);
        }
    }

    fn build_active_plugin_tool_index(
        registry: &PluginRegistry,
    ) -> HashMap<String, ToolRegistration> {
        let mut active_plugins: Vec<String> = registry
            .list_active_plugins()
            .into_iter()
            .map(|plugin| plugin.id.clone())
            .collect();
        active_plugins.sort();

        let mut active_tools = HashMap::new();
        for plugin_id in active_plugins {
            let mut plugin_tools: Vec<ToolRegistration> = registry
                .list_tools_for_plugin(&plugin_id)
                .into_iter()
                .cloned()
                .collect();
            plugin_tools.sort_by(|a, b| a.name.cmp(&b.name));

            for tool in plugin_tools {
                active_tools.entry(tool.name.clone()).or_insert(tool);
            }
        }

        active_tools
    }

    async fn refresh_active_plugin_tools(&self) {
        let active_tools = {
            let registry = self.plugin_registry.read().await;
            Self::build_active_plugin_tool_index(&registry)
        };

        *self
            .active_plugin_tools
            .write()
            .unwrap_or_else(|e| e.into_inner()) = active_tools;
        self.plugin_tool_revision.fetch_add(1, Ordering::SeqCst);
    }

    /// Refresh sync runtime caches derived from the async plugin registry.
    pub async fn sync_runtime_snapshots(&self) {
        self.refresh_active_plugin_tools().await;
    }

    /// Monotonic revision for active plugin tool metadata.
    pub fn plugin_tool_revision(&self) -> u64 {
        self.plugin_tool_revision.load(Ordering::SeqCst)
    }

    /// Snapshot of active plugin tools keyed by short tool name.
    pub fn active_plugin_tools_snapshot(&self) -> Vec<ToolRegistration> {
        let tools = self
            .active_plugin_tools
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let mut snapshot: Vec<ToolRegistration> = tools.values().cloned().collect();
        snapshot.sort_by(|a, b| a.name.cmp(&b.name).then(a.plugin_id.cmp(&b.plugin_id)));
        snapshot
    }

    /// Resolve an active plugin tool by short name or `plugin_id:name`.
    pub fn resolve_active_plugin_tool(&self, name: &str) -> Option<ToolRegistration> {
        let tools = self
            .active_plugin_tools
            .read()
            .unwrap_or_else(|e| e.into_inner());

        if let Some(tool) = tools.get(name) {
            return Some(tool.clone());
        }

        let (plugin_id, short_name) = name.split_once(':')?;
        tools
            .get(short_name)
            .filter(|tool| tool.plugin_id == plugin_id)
            .cloned()
    }

    /// Check if extensions have been loaded
    pub async fn is_loaded(&self) -> bool {
        self.cache_state.read().await.loaded
    }

    /// Get a reference to the adapter registry for capability-driven parsing.
    pub fn adapter_registry(&self) -> &AdapterRegistry {
        &self.adapter_registry
    }

    /// Hot-reload a single plugin by ID.
    ///
    /// Unregisters all existing capabilities, re-parses the manifest from disk,
    /// and re-registers all capabilities declared in the updated manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin is not found, the manifest cannot be parsed,
    /// or capability registration fails (e.g. missing permissions).
    pub async fn reload_plugin(&self, plugin_id: &str) -> anyhow::Result<()> {
        // Find root dir from existing record
        let root_dir = {
            let registry = self.plugin_registry.read().await;
            registry
                .get_plugin(plugin_id)
                .map(|p| p.root_dir.clone())
                .ok_or_else(|| anyhow::anyhow!("Plugin not found: {}", plugin_id))?
        };

        // Re-parse manifest via adapter registry
        let output = self.adapter_registry.parse_dir(&root_dir)?;

        // Use permissions from adapter output (consistent with load_all path)
        let permissions = output.permissions.clone();

        // Build updated plugin record
        let record = PluginRecord::from_adapter_output(&output, root_dir);

        // Atomically unregister old capabilities and register new ones
        let mut registry = self.plugin_registry.write().await;
        let mut api =
            registrar::CapabilityApi::new(&mut registry, output.plugin_id.clone(), permissions);
        api.reload(record, output.capabilities)?;
        drop(registry);

        self.refresh_active_plugin_tools().await;

        tracing::info!(plugin = plugin_id, "Plugin hot-reloaded successfully");
        Ok(())
    }
}

// =============================================================================
// Utility Functions
// =============================================================================

/// Get the default plugins directory (user scope: ~/.aleph/plugins/installed/)
pub fn default_plugins_dir() -> std::path::PathBuf {
    crate::discovery::aleph_plugins_dir()
        .map(|p| p.join("installed"))
        .unwrap_or_else(|_| std::path::PathBuf::from("~/.aleph/plugins/installed"))
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
        let result = manager
            .call_plugin_tool("nonexistent", "handler", serde_json::json!({}))
            .await;
        assert!(result.is_err());
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
        let registry = manager.get_plugin_registry().await;
        assert!(registry.list_plugins().is_empty());
        assert!(registry.list_tools().is_empty());
    }

    #[tokio::test]
    async fn test_extension_manager_execute_plugin_hook_nonexistent() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let result = manager
            .execute_plugin_hook("nonexistent", "onEvent", serde_json::json!({"test": true}))
            .await;
        assert!(result.is_err());
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
        let loader = manager.get_plugin_loader().await;
        assert!(!loader.is_any_runtime_active());
        assert!(loader.loaded_plugin_ids().is_empty());
    }

    #[tokio::test]
    async fn test_extension_manager_has_service_manager() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let service_manager = manager.get_service_manager().await;
        assert_eq!(service_manager.running_count(), 0);
        assert_eq!(service_manager.total_count(), 0);
    }

    #[tokio::test]
    async fn test_extension_manager_list_services_empty() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let services = manager.list_services().await;
        assert!(services.is_empty());
    }

    #[tokio::test]
    async fn test_extension_manager_running_service_count() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        assert_eq!(manager.running_service_count().await, 0);
    }

    #[tokio::test]
    async fn test_extension_manager_get_service_status_not_found() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let status = manager
            .get_service_status("nonexistent-plugin", "nonexistent-service")
            .await;
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_extension_manager_start_service_not_registered() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let result = manager
            .start_service("nonexistent-plugin", "nonexistent-service")
            .await;
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
        let result = manager
            .stop_service("nonexistent-plugin", "nonexistent-service")
            .await;
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
