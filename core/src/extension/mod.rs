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
//!          ┌───────────────────┴───────────────────┐
//!          ▼                                       ▼
//!     HookExecutor                          ContentLoader
//!     (unified hooks)                    (Markdown parsing)
//! ```

pub mod component_id;
pub mod config;
pub mod discovery;
pub mod hooks;
pub mod marketplace;
mod plugin_loader;
pub mod runtime;
pub mod scope;
pub mod sync_api;
pub mod validation;

mod channel_manager;
mod error;
mod http_handler;
mod content_loader;
pub mod mcp_config;
pub mod manifest;
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

pub use component_id::ComponentId;
pub use channel_manager::{ChannelHandle, ChannelManager};
pub use error::*;
pub use http_handler::{match_path, PluginHttpHandler};
pub use content_loader::*;
pub use manifest::*;
pub use plugin_loader::PluginLoader;
pub use provider_adapter::PluginProviderAdapter;
pub use registry::*;
pub use service_manager::ServiceManager;
pub use skill_tool::{build_skill_tool_description, check_skill_permission, request_skill_permission_async};
pub use template::SkillTemplate;
pub use types::*;

// Re-export config types
pub use config::{AlephConfig, ConfigManager};

// Re-export sync API
pub use sync_api::SyncExtensionManager;

// Re-export new plugin system types (Phase 1)
pub use discovery::{discover_all, DiscoveryConfig as PluginDiscoveryConfig, PluginCandidate};
pub use manifest::PluginManifest;
pub use registry::{HookRegistration, PluginRegistry, ToolRegistration};
pub use types::{PluginKind, PluginOrigin, PluginRecord, PluginStatus};

use crate::discovery::{DiscoveryConfig, DiscoveryManager};
use hooks::{HookExecutor};
use std::collections::HashMap;
use std::path::PathBuf;
use crate::sync_primitives::Arc;
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
    /// Discovery manager
    discovery: DiscoveryManager,

    /// Config manager (aleph.jsonc)
    config_manager: ConfigManager,

    /// Loaded skills by qualified name
    skills: Arc<RwLock<HashMap<String, ExtensionSkill>>>,

    /// Loaded commands by name
    commands: Arc<RwLock<HashMap<String, ExtensionCommand>>>,

    /// Loaded agents by qualified name
    agents: Arc<RwLock<HashMap<String, ExtensionAgent>>>,

    /// Component loader
    loader: ContentLoader,

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

    /// Skill System v2 (independent bounded context)
    skill_system: crate::skill::SkillSystem,
}

impl ExtensionManager {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Create a new extension manager
    pub async fn new(config: ExtensionConfig) -> ExtensionResult<Self> {
        let discovery = DiscoveryManager::new(config.discovery.clone())?;
        let config_manager = ConfigManager::new(&discovery).await?;
        let loader = ContentLoader::new();
        let hook_executor = Arc::new(RwLock::new(HookExecutor::empty()));
        let cache_state = Arc::new(RwLock::new(CacheState::default()));
        let plugin_loader = Arc::new(RwLock::new(PluginLoader::new()));
        let plugin_registry = Arc::new(RwLock::new(PluginRegistry::new()));
        let service_manager = Arc::new(RwLock::new(ServiceManager::new()));

        Ok(Self {
            discovery,
            config_manager,
            skills: Arc::new(RwLock::new(HashMap::new())),
            commands: Arc::new(RwLock::new(HashMap::new())),
            agents: Arc::new(RwLock::new(HashMap::new())),
            loader,
            hook_executor,
            cache_state,
            plugin_loader,
            plugin_registry,
            service_manager,
            skill_system: crate::skill::SkillSystem::new(),
        })
    }

    /// Create with default configuration
    pub async fn with_defaults() -> ExtensionResult<Self> {
        Self::new(ExtensionConfig::default()).await
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Load all extensions.
    pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
        let load_result = self.loader.load_all(&self.discovery).await?;

        // Store loaded skills
        {
            let mut skills = self.skills.write().await;
            for skill in load_result.skills {
                let name = skill.qualified_name();
                skills.insert(name, skill);
            }
        }

        // Store loaded commands
        {
            let mut commands = self.commands.write().await;
            for cmd in load_result.commands {
                let name = cmd.qualified_name();
                commands.insert(name, cmd);
            }
        }

        // Store loaded agents
        {
            let mut agents = self.agents.write().await;
            for agent in load_result.agents {
                let name = agent.qualified_name();
                agents.insert(name, agent);
            }
        }

        // Register hooks
        {
            let mut executor = self.hook_executor.write().await;
            for hook in load_result.hooks {
                executor.add_hook(hook);
            }
        }

        // Discover and register plugins from ~/.aleph/plugins/
        {
            let scanner = &self.discovery;
            if let Ok(discovered) = scanner.discover_plugins() {
                let mut registry = self.plugin_registry.write().await;
                for dp in discovered {
                    if registry.get_plugin(&dp.name).is_some() {
                        continue;
                    }
                    match crate::extension::manifest::parse_manifest_from_dir_sync(&dp.path) {
                        Ok(manifest) => {
                            let mut record = PluginRecord::new(
                                manifest.id.clone(),
                                manifest.name.clone(),
                                manifest.kind,
                                PluginOrigin::Global,
                            );
                            record.version = manifest.version.clone();
                            record.description = manifest.description.clone();
                            record.root_dir = dp.path.clone();
                            record.hook_count = manifest.hooks_v2.as_ref().map_or(0, |h| h.len());
                            registry.register_plugin(record);
                        }
                        Err(e) => {
                            tracing::debug!("Skipping plugin at {:?}: {}", dp.path, e);
                        }
                    }
                }
            }
        }

        // Initialize SkillSystem with discovered skill directories
        let skill_dirs: Vec<PathBuf> = self.discovery.discover_skill_dirs()
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.path)
            .collect();
        if let Err(e) = self.skill_system.init(skill_dirs).await {
            tracing::warn!("Failed to init skill system: {}", e);
        }

        let mut cache = self.cache_state.write().await;
        cache.loaded = true;
        cache.loaded_at = Some(Instant::now());

        Ok(load_result.summary)
    }

    /// Ensure extensions are loaded (lazy-loading entry point).
    pub async fn ensure_loaded(&self) -> ExtensionResult<()> {
        {
            let state = self.cache_state.read().await;
            if state.loaded {
                return Ok(());
            }
        }

        let mut state = self.cache_state.write().await;
        if state.loaded {
            return Ok(());
        }

        state.loaded = true;
        drop(state);

        if let Err(e) = self.load_all().await {
            let mut state = self.cache_state.write().await;
            state.loaded = false;
            return Err(e);
        }

        Ok(())
    }

    /// Force reload all extensions
    pub async fn reload(&self) -> ExtensionResult<LoadSummary> {
        {
            let mut state = self.cache_state.write().await;
            state.loaded = false;
            state.loaded_at = None;
        }

        self.skills.write().await.clear();
        self.commands.write().await.clear();
        self.agents.write().await.clear();
        *self.hook_executor.write().await = HookExecutor::empty();

        self.load_all().await
    }

    /// Check if extensions have been loaded
    pub async fn is_loaded(&self) -> bool {
        self.cache_state.read().await.loaded
    }
}

// =============================================================================
// Utility Functions
// =============================================================================

/// Build skill instructions for LLM prompt injection
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
pub fn is_valid_plugin_dir(path: &std::path::Path) -> bool {
    path.join(".claude-plugin").join("plugin.json").exists()
}

/// Get the default plugins directory (user scope: ~/.aleph/plugins/installed/)
pub fn default_plugins_dir() -> std::path::PathBuf {
    crate::discovery::aleph_plugins_dir()
        .map(|p| p.join("installed"))
        .unwrap_or_else(|_| std::path::PathBuf::from("~/.aleph/plugins/installed"))
}

/// Get the legacy plugins directory (~/.aleph/plugins/ — for backward compat scanning)
pub fn legacy_plugins_dir() -> std::path::PathBuf {
    crate::discovery::aleph_plugins_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("~/.aleph/plugins"))
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
        let status = manager.get_service_status("nonexistent-plugin", "nonexistent-service").await;
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_extension_manager_start_service_not_registered() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
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
