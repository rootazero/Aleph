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

pub mod hooks;
mod loader;
pub mod marketplace;
pub mod runtime;
pub mod scope;
pub mod validation;

pub mod capability;
pub mod registrar;

mod error;
mod manager_global;
pub mod manifest;
pub mod mcp_config;
mod plugin_ops;
pub mod plugin_state;
pub mod plugin_trust;
mod projection;
pub mod registry;
mod service_manager;
mod service_ops;
mod skill_ops;
mod skill_tool;
mod template;
mod types;
pub mod watcher;

pub use error::*;
pub use loader::PluginLoader;
pub use manager_global::{
    init_extension_manager, is_extension_manager_initialized, try_extension_manager,
};
pub use manifest::*;
pub use registry::*;
pub use service_manager::ServiceManager;
pub use template::SkillTemplate;
pub use types::*;

// Re-export marketplace types
pub use marketplace::types::{MarketplaceConfig, MarketplaceSourceType};

// Re-export new plugin system types (Phase 1)
pub use capability::{CapabilityDeclaration, CapabilitySource, SourceFormat, Tier};
pub use manifest::PluginManifest;
pub use registrar::CapabilityApi;
pub use registry::{HookRegistration, PluginRegistry, ToolRegistration};
pub use types::{PluginKind, PluginOrigin, PluginRecord, PluginStatus};

use crate::discovery::{DiscoveryConfig, DiscoveryManager};
use crate::sync_primitives::Arc;
use crate::sync_primitives::Mutex as StdMutex;
use crate::sync_primitives::RwLock as StdRwLock;
use crate::sync_primitives::{AtomicU64, Ordering};
use hooks::{HookExecutor, ShellHookConsent};
use manifest::adapter::AdapterRegistry;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize}; // for OwnerTrustPolicyConfig
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::{Mutex, RwLock};
use watcher::{ExtensionChangeEvent, ExtensionChangeType, ExtensionWatcher, InternalWriteTracker};

// =============================================================================
// Cache State
// =============================================================================

/// Cache state for lazy-loading
#[derive(Debug, Default)]
struct CacheState {
    /// Whether components have been loaded
    loaded: bool,
}

/// Extension system configuration
#[derive(Debug, Clone, Default)]
pub struct ExtensionConfig {
    /// Discovery configuration
    pub discovery: DiscoveryConfig,

    /// Override for the durable plugin activation document
    /// (`<data_dir>/plugins.toml`).
    ///
    /// `None` resolves the standard location. Tests set this to a temp path:
    /// the alternative — pointing `ALEPH_HOME` somewhere else — is a
    /// **process-global** switch, and libtest runs in parallel, so two tests
    /// would silently fight over it.
    pub plugins_config_path: Option<PathBuf>,

    /// Extra plugin-parent directories to scan, **test-only**.
    ///
    /// Production plugin parents come from `ALEPH_HOME` and the project
    /// registry, both process-global — so without this a test cannot give a
    /// manager its own plugin tree without fighting every sibling test for the
    /// same environment variable. Gated on `cfg(test)` so it cannot become an
    /// undocumented production knob with no consumers (R10).
    #[cfg(test)]
    pub extra_plugin_parents: Vec<PathBuf>,

    /// Owner trust policy for plugin loading. Default is `permissive()`
    /// (legacy behaviour: every plugin loads). Setting this to `restrictive`
    /// causes `Workspace`/`Global` plugins to be gated by the supplied
    /// allowlist — see [`crate::extension::plugin_trust::OwnerTrustPolicy`].
    /// Wrapped in `Option` because `Default` cannot construct an
    /// `OwnerTrustPolicy` value from this module without an import seam.
    pub owner_trust: Option<OwnerTrustPolicyConfig>,
}

/// Wire-friendly form of [`crate::extension::plugin_trust::OwnerTrustPolicy`].
///
/// We don't deserialize `OwnerTrustPolicy` directly because it is a runtime
/// stateful object with a method-rich API (`trust`, `untrust`, `allowlist()`);
/// operators only need the static allowlist + mode. The manager converts this
/// DTO into the runtime policy in [`ExtensionManager::new`].
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerTrustPolicyConfig {
    /// `true` → enforce the allowlist for `Workspace`/`Global` plugins.
    /// `false` → permissive (legacy).
    #[serde(default)]
    pub restrictive: bool,
    /// Plugin ids that may load from non-trusted origins when restrictive.
    #[serde(default)]
    pub allowlist: Vec<String>,
}

impl OwnerTrustPolicyConfig {
    /// Materialise the runtime policy. `restrictive=false` ⇒ `permissive()`
    /// (matches historical default); `restrictive=true` ⇒ `restrictive(allowlist)`.
    #[must_use]
    pub fn to_policy(self) -> crate::extension::plugin_trust::OwnerTrustPolicy {
        if self.restrictive {
            crate::extension::plugin_trust::OwnerTrustPolicy::restrictive(self.allowlist)
        } else {
            crate::extension::plugin_trust::OwnerTrustPolicy::permissive()
        }
    }
}

/// Extension Manager - main entry point for the extension system
pub struct ExtensionManager {
    /// Discovery manager
    discovery: DiscoveryManager,

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

    /// Guard to serialize concurrent `load_all()` calls
    load_guard: Mutex<()>,

    /// Memory extension registry (Spec 4 Task 11).
    /// When set, `load_runtime_plugin` / `ensure_plugin_loaded` call
    /// `load_plugin_with_memory` so that plugins declaring a `[memory]`
    /// section are auto-registered as `McpMemoryExtension` entries.
    /// Wrapped in `RwLock` so it can be injected after construction (the manager
    /// is typically behind an Arc by the time Task 11 calls `set_memory_registry`).
    memory_registry: crate::sync_primitives::RwLock<
        Option<crate::sync_primitives::Arc<crate::memory::extensions::MemoryExtensionRegistry>>,
    >,

    /// Owner trust policy (P3.5 — openclaw parity). When set to
    /// `OwnerTrustPolicy::restrictive(allowlist)`, plugins from
    /// `Workspace` / `Global` origins are only loaded when their id is
    /// in the allowlist. Default is `permissive()` (legacy behaviour:
    /// every plugin loads). Updated by [`Self::set_owner_trust_policy`].
    owner_trust_policy:
        Arc<crate::sync_primitives::RwLock<crate::extension::plugin_trust::OwnerTrustPolicy>>,

    /// Durable per-plugin activation state (`<data_dir>/plugins.toml`).
    ///
    /// The single source for "did the operator disable this plugin?". Read in
    /// `load_all` so the answer survives a restart, written by
    /// `set_plugin_enabled` — which every toggle face already funnels through.
    /// See `plugin_state.rs` for why this replaced the `.disabled` marker.
    plugins_config: Arc<RwLock<crate::extension::plugin_state::PluginsConfig>>,

    /// Where [`Self::plugins_config`] is persisted. Resolved once at
    /// construction against the same root the skill twin uses.
    plugins_config_path: PathBuf,

    /// Test-only extra plugin parents; see [`ExtensionConfig::extra_plugin_parents`].
    #[cfg(test)]
    extra_plugin_parents: Vec<PathBuf>,

    /// Live MCP manager handle, used to register plugin-owned MCP servers as
    /// **transient** (runtime-only) servers. `None` until [`Self::set_mcp_handle`]
    /// is called at server boot — CLI/test paths leave it unset, so plugin MCP
    /// registration simply no-ops there. Wrapped in `RwLock<Option<_>>` because
    /// the manager is behind an `Arc` by the time the MCP actor materialises,
    /// mirroring the `memory_registry` injection pattern.
    mcp_handle: crate::sync_primitives::RwLock<Option<crate::mcp::McpManagerHandle>>,

    /// File watcher for hot-reloading commands/agents/plugins/hooks.json.
    /// `None` until [`Self::start_watcher`] is called (test/CLI paths skip
    /// the watcher entirely).
    watcher: StdMutex<Option<Arc<ExtensionWatcher>>>,

    /// Tracks paths Aleph itself just wrote so the watcher can skip them
    /// and avoid a write→reload→write feedback loop. Shared with the watcher
    /// callback via `Arc`.
    internal_writes: Arc<InternalWriteTracker>,

    /// Counts how many times `reload()` has executed since construction.
    /// Exposed via [`Self::reload_count`] — used by integration tests to
    /// assert at-most-once reload behaviour on adjacent watcher events,
    /// and available for future runtime observability.
    reload_count: AtomicU64,
}

/// Convert a plugin-shipped agent registration into an [`crate::agents::AgentDef`]
/// for the agent registry's delegation + `<available_agents>` catalog, or `None`
/// when it is not a delegatable sub-agent.
///
/// Mirrors the disk-agent loader (`crate::agents::loader`): the markdown
/// `content` (system-prompt body) is intentionally dropped — `AgentDef` is
/// frontmatter / section-key based and disk agents discard their body the same
/// way, so a plugin sub-agent runs through the standard sub-agent prompt flow.
/// The declared `tools` map (name → allowed) becomes allow / deny lists; absent,
/// the constructor default (`["*"]`) is kept, matching a disk agent with no tool
/// frontmatter (a subagent is spawned by an already-privileged primary and the
/// recursion guard blocks re-spawning, so wildcard-by-default is safe here too).
fn plugin_agent_to_def(
    reg: &crate::extension::AgentRegistration,
) -> Option<crate::agents::AgentDef> {
    if !reg.is_subagent() {
        return None;
    }
    let id = reg.name.trim();
    if id.is_empty() {
        return None;
    }
    let mut def = crate::agents::AgentDef::new(id, crate::agents::AgentMode::SubAgent);
    if let Some(desc) = &reg.description {
        def = def.with_description(desc.clone());
    }
    if let Some(tools) = &reg.tools {
        let mut allowed: Vec<String> = tools
            .iter()
            .filter_map(|(t, &ok)| ok.then_some(t.clone()))
            .collect();
        let mut denied: Vec<String> = tools
            .iter()
            .filter_map(|(t, &ok)| (!ok).then_some(t.clone()))
            .collect();
        allowed.sort();
        denied.sort();
        if !allowed.is_empty() {
            def = def.with_allowed_tools(allowed);
        }
        if !denied.is_empty() {
            def = def.with_denied_tools(denied);
        }
    }
    if let Some(steps) = reg.steps {
        def = def.with_max_iterations(steps);
    }
    if let Some(model) = &reg.model {
        def = def.with_model_hint(model.clone());
    }
    def.source = crate::agents::AgentSource::Plugin;
    Some(def)
}

impl ExtensionManager {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Create a new extension manager
    pub async fn new(config: ExtensionConfig) -> ExtensionResult<Self> {
        let discovery = DiscoveryManager::new(config.discovery.clone())?;
        let hook_executor = Arc::new(RwLock::new(
            HookExecutor::empty().with_consent(ShellHookConsent::shared()),
        ));
        let cache_state = Arc::new(RwLock::new(CacheState::default()));
        let plugin_loader = Arc::new(RwLock::new(PluginLoader::new()));
        let plugin_registry = Arc::new(RwLock::new(PluginRegistry::new()));
        let service_manager = Arc::new(RwLock::new(ServiceManager::new()));
        let adapter_registry = AdapterRegistry::with_defaults();

        // Mirrors `crate::skill::SkillSystem::new`: `get_config_dir` is a pure
        // lookup (it does not create), so constructing a manager never makes a
        // directory as a side effect.
        let plugins_config_path = config.plugins_config_path.clone().unwrap_or_else(|| {
            crate::utils::paths::get_config_dir()
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "cannot resolve Aleph config dir; falling back to ./.aleph for plugin state");
                    PathBuf::from(".aleph")
                })
                .join("data")
                .join(crate::extension::plugin_state::PLUGINS_CONFIG_FILE)
        });
        let plugins_config = Arc::new(RwLock::new(
            crate::extension::plugin_state::PluginsConfig::load(&plugins_config_path),
        ));

        Ok(Self {
            discovery,
            hook_executor,
            cache_state,
            plugin_loader,
            plugin_registry,
            service_manager,
            adapter_registry,
            // Share the process-wide instance so init() here is visible to
            // the builtin skill tools and the gateway RPC handlers.
            skill_system: crate::skill::shared_skill_system().clone(),
            active_plugin_tools: Arc::new(StdRwLock::new(HashMap::new())),
            plugin_tool_revision: Arc::new(AtomicU64::new(0)),
            load_guard: Mutex::new(()),
            memory_registry: crate::sync_primitives::RwLock::new(None),
            mcp_handle: crate::sync_primitives::RwLock::new(None),
            watcher: StdMutex::new(None),
            internal_writes: Arc::new(InternalWriteTracker::default()),
            reload_count: AtomicU64::new(0),
            owner_trust_policy: Arc::new(crate::sync_primitives::RwLock::new(
                config
                    .owner_trust
                    .map(OwnerTrustPolicyConfig::to_policy)
                    .unwrap_or_else(crate::extension::plugin_trust::OwnerTrustPolicy::permissive),
            )),
            plugins_config,
            plugins_config_path,
            #[cfg(test)]
            extra_plugin_parents: config.extra_plugin_parents.clone(),
        })
    }

    /// Create with default configuration
    pub async fn with_defaults() -> ExtensionResult<Self> {
        Self::new(ExtensionConfig::default()).await
    }

    /// Builder-style setter for the memory extension registry (Spec 4 Task 11).
    ///
    /// For use before the manager is shared behind an `Arc`. If you already have
    /// `Arc<ExtensionManager>`, use `set_memory_registry` instead.
    pub fn with_memory_registry(
        self,
        registry: crate::sync_primitives::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        *self
            .memory_registry
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(registry);
        self
    }

    /// Inject the memory extension registry after construction (Spec 4 Task 11).
    ///
    /// Safe to call on `&Arc<ExtensionManager>`. Subsequent plugin loads will
    /// use `load_plugin_with_memory` so that manifests declaring a `[memory]`
    /// section are auto-registered as `McpMemoryExtension` entries.
    pub fn set_memory_registry(
        &self,
        registry: crate::sync_primitives::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) {
        *self
            .memory_registry
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(registry);
    }

    /// Install a non-default owner trust policy. The next
    /// [`Self::load_all`] / [`Self::reload`] call uses the new policy to
    /// gate `Workspace` and `Global` plugins (Bundled/Config always pass).
    /// Pass [`OwnerTrustPolicy::permissive`] to revert to the legacy
    /// "load every plugin" behaviour.
    pub fn set_owner_trust_policy(&self, policy: crate::extension::plugin_trust::OwnerTrustPolicy) {
        *self
            .owner_trust_policy
            .write()
            .unwrap_or_else(|e| e.into_inner()) = policy;
    }

    /// Snapshot the current owner trust policy. Used by
    /// `extensions.stat` / `plugin trust <id>` so operators can read what
    /// the manager is currently enforcing.
    #[must_use]
    pub fn current_owner_trust_policy(&self) -> crate::extension::plugin_trust::OwnerTrustPolicy {
        self.owner_trust_policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Inject the live MCP manager handle after construction.
    ///
    /// Safe to call on `&Arc<ExtensionManager>`. Stores the handle so that
    /// [`Self::sync_mcp_plugin_servers`] can register plugin-owned MCP servers.
    /// Call it once at server boot — *after* the MCP tool bridge is spawned, so
    /// the `ServerStarted` events the sync triggers are observed and turned into
    /// tool registrations.
    pub fn set_mcp_handle(&self, handle: crate::mcp::McpManagerHandle) {
        *self.mcp_handle.write().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }

    /// Bind every registered MCP-backed memory extension to the live MCP
    /// manager. Idempotent: re-binding an already-bound extension just re-stores
    /// the caller. No-op unless BOTH the MCP handle and the memory registry are
    /// present (CLI/test paths leave them unset). Call once at boot after
    /// `set_mcp_handle` + `set_memory_registry` + plugin load, and again after
    /// hot-loading a plugin.
    pub async fn bind_memory_callers(&self) {
        use crate::memory::extensions::ManagerBackedMcpCaller;
        let handle = {
            let g = self.mcp_handle.read().unwrap_or_else(|e| e.into_inner());
            match g.as_ref() {
                Some(h) => h.clone(),
                None => return,
            }
        };
        let registry = {
            let g = self
                .memory_registry
                .read()
                .unwrap_or_else(|e| e.into_inner());
            match g.as_ref() {
                Some(r) => r.clone(),
                None => return,
            }
        };
        for ext in registry.mcp_bindings_snapshot() {
            if let Some(server_id) = ext.server_id() {
                let caller = crate::sync_primitives::Arc::new(ManagerBackedMcpCaller::new(
                    handle.clone(),
                    server_id.to_string(),
                ));
                ext.rebind(caller);
                tracing::info!(server_id = %server_id, "bound memory MCP caller");
            }
        }
    }

    /// Register every enabled MCP-kind plugin's `.mcp.json` servers with the
    /// attached MCP manager as **transient** (runtime-only) servers.
    ///
    /// This is the wiring that makes MCP plugins actually run: `PluginLoader`
    /// reads each plugin's `.mcp.json` into memory, and this method hands those
    /// configs to `McpManager::add_transient_server`, whose `ServerStarted`
    /// events the tool bridge converts into live tool registrations.
    ///
    /// Idempotent and non-fatal: a no-op (returns 0) when no handle is attached;
    /// servers already running are skipped by the manager. Returns the number of
    /// server configs handed to the manager this call.
    pub async fn sync_mcp_plugin_servers(&self) -> usize {
        let handle = {
            let guard = self.mcp_handle.read().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some(h) => h.clone(),
                None => return 0,
            }
        };

        if let Err(e) = self.ensure_loaded().await {
            tracing::warn!(error = %e, "sync_mcp_plugin_servers: ensure_loaded failed");
            return 0;
        }

        // Snapshot active plugins (id + root_dir). `PluginRecord.kind` is always
        // `Static` after `load_all` (the adapter doesn't carry runtime kind), so
        // we re-parse each manifest to find the true MCP-kind plugins.
        let candidates: Vec<(String, PathBuf)> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_plugins()
                .into_iter()
                .filter(|r| r.status.is_active())
                .map(|r| (r.id.clone(), r.root_dir.clone()))
                .collect()
        };

        for (id, root) in &candidates {
            match manifest::parse_manifest_from_dir_cached_global(root) {
                Ok(m) if m.kind == PluginKind::Mcp => {
                    if let Err(e) = self.ensure_plugin_loaded(id).await {
                        tracing::warn!(plugin = %id, error = %e, "sync_mcp_plugin_servers: failed to load MCP plugin");
                    }
                }
                Ok(_) => {} // non-MCP plugin: nothing to register here
                Err(e) => {
                    tracing::debug!(plugin = %id, error = %e, "sync_mcp_plugin_servers: manifest parse failed");
                }
            }
        }

        let configs = self.plugin_loader.read().await.all_mcp_configs_map();
        let mut registered = 0usize;
        for (server_id, config) in configs {
            match handle.add_transient_server(config).await {
                Ok(()) => registered += 1,
                Err(e) => {
                    tracing::warn!(server_id = %server_id, error = %e, "sync_mcp_plugin_servers: add_transient_server failed");
                }
            }
        }

        if registered > 0 {
            tracing::info!(
                count = registered,
                "registered plugin MCP servers (transient)"
            );
        }
        registered
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Load all extensions.
    pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
        let mut summary = LoadSummary::default();

        // Collect all plugin directories from discovery
        let plugin_dirs = self.collect_plugin_dirs()?;
        *self.hook_executor.write().await =
            HookExecutor::empty().with_consent(ShellHookConsent::shared());

        // Clear and rebuild registry
        {
            let mut registry = self.plugin_registry.write().await;
            registry.clear();
            let mut registered_plugin_ids = std::collections::HashSet::new();

            for dir_path in plugin_dirs.iter().rev() {
                match self.adapter_registry.parse_dir(dir_path) {
                    Ok(output) => {
                        // Shadow resolution: dirs are walked highest-priority
                        // first, so a repeat id lost. It used to be dropped
                        // silently — indistinguishable from "not installed",
                        // and the `Overridden` status written for exactly this
                        // moment had zero producers. Record the loss on the
                        // winner (the registry is keyed by id, so the loser
                        // cannot hold a row of its own).
                        if !registered_plugin_ids.insert(output.plugin_id.clone()) {
                            let winner = registry
                                .get_plugin(&output.plugin_id)
                                .map(|p| p.root_dir.display().to_string())
                                .unwrap_or_default();
                            tracing::info!(
                                plugin_id = %output.plugin_id,
                                shadowed = %dir_path.display(),
                                winner = %winner,
                                "plugin id already registered from a higher-priority scope"
                            );
                            registry.add_diagnostic(PluginDiagnostic {
                                level: DiagnosticLevel::Warn,
                                message: format!(
                                    "{} is shadowed by the copy at {winner}",
                                    dir_path.display()
                                ),
                                plugin_id: Some(output.plugin_id.clone()),
                                source: Some("discovery".to_string()),
                            });
                            summary.shadowed += 1;
                            continue;
                        }
                        // Build plugin record from adapter output
                        let mut record =
                            PluginRecord::from_adapter_output(&output, dir_path.clone());
                        if let Ok(manifest) =
                            manifest::parse_manifest_from_dir_cached_global(dir_path)
                        {
                            record.kind = manifest.kind;
                        }
                        let plugin_id = output.plugin_id.clone();
                        // P3.5 — owner trust policy gates the load. When the
                        // policy is `restrictive`, plugins from `Workspace` or
                        // `Global` origins are only registered if their id is
                        // in the policy's allowlist; `Bundled`/`Config`
                        // plugins always pass. The default is `permissive`
                        // (matches the legacy "load everything" behaviour).
                        let trust_allows = self
                            .owner_trust_policy
                            .read()
                            .unwrap_or_else(|e| e.into_inner())
                            .allows(&plugin_id, record.origin);
                        if !trust_allows {
                            tracing::info!(
                                plugin_id = %plugin_id,
                                origin = ?record.origin,
                                "plugin skipped by owner trust policy \
                                 (not in allowlist)"
                            );
                            summary.skipped_by_trust += 1;
                            // Register it as Blocked rather than dropping it:
                            // "refused by policy" and "not installed" must not
                            // render the same, and the operator needs the id to
                            // put on the allowlist.
                            let origin = record.origin;
                            registry.register_plugin(record.inactive(
                                PluginStatus::Blocked(format!("{origin:?}")),
                                format!(
                                    "refused by the owner trust policy ({origin:?} origin is not \
                                     on the allowlist); add \"{plugin_id}\" to it to load this plugin"
                                ),
                            ));
                            continue;
                        }
                        // Durable activation state. `.disabled` markers written
                        // by older builds are migrated into `plugins.toml` on
                        // first sight and then removed, so the answer converges
                        // to one source instead of two (the marker could not
                        // survive `plugin update`, which swaps the whole tree).
                        let legacy_marker = dir_path.join(".disabled");
                        if legacy_marker.exists() {
                            let mut cfg = self.plugins_config.write().await;
                            if cfg.set_enabled(&plugin_id, false) {
                                let path = self.plugins_config_path.clone();
                                if let Err(e) = cfg.save(&path).await {
                                    tracing::warn!(error = %e, "failed to persist migrated plugin disable");
                                }
                            }
                            drop(cfg);
                            match std::fs::remove_file(&legacy_marker) {
                                Ok(()) => tracing::info!(
                                    plugin_id = %plugin_id,
                                    "migrated legacy .disabled marker into plugins.toml"
                                ),
                                Err(e) => tracing::warn!(
                                    plugin_id = %plugin_id, error = %e,
                                    "legacy .disabled marker migrated but could not be removed"
                                ),
                            }
                        }
                        let operator_enabled =
                            self.plugins_config.read().await.is_enabled(&plugin_id);
                        if !operator_enabled {
                            // Registered but not active. The record and its
                            // capabilities are still registered — every
                            // downstream consumer already filters on
                            // `status.is_active()` (tool index, hook sync,
                            // `projection.rs`), so a disabled plugin is invisible
                            // to the model while staying listable and, crucially,
                            // **re-enablable without a reload**: skipping
                            // capability registration here would make
                            // `set_plugin_enabled(id, true)` flip a status with
                            // nothing behind it.
                            record.status = PluginStatus::Disabled;
                            summary.disabled_by_operator += 1;
                        }

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
                        // A plugin whose manifest will not parse used to vanish
                        // at `debug!` level: on every surface it looked exactly
                        // like a plugin that was never installed, so the
                        // operator had nothing to fix. Give it a row, an id
                        // derived from the directory, and the parse error.
                        let fallback_id = dir_path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| dir_path.display().to_string());
                        tracing::warn!(
                            plugin_dir = %dir_path.display(), error = %e,
                            "plugin manifest could not be parsed; listing it as errored"
                        );
                        summary
                            .errors
                            .push(format!("{}: {}", dir_path.display(), e));
                        if registered_plugin_ids.insert(fallback_id.clone()) {
                            let record = PluginRecord::new(
                                fallback_id,
                                dir_path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                PluginKind::Static,
                                PluginOrigin::Global,
                            )
                            .with_root_dir(dir_path.clone())
                            .with_error(e.to_string());
                            registry.register_plugin(record);
                        }
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

        // Layer in user-level hooks from ~/.aleph/hooks.json and the
        // project's .aleph/hooks.{json,local.json}. Claude Code parity:
        // users edit these files directly; no plugin packaging required.
        self.sync_user_hooks().await;

        // Publish every plugin-owned process-global projection (skill dirs,
        // sub-agents, tool index) from the ONE derivation that states the
        // activation predicate — see `projection.rs`. This used to be inlined
        // here with `list_plugins()` (every status), while `set_plugin_enabled`
        // used `list_active_plugins()`: two authors, opposite answers, and the
        // permissive one ran on every boot.
        let projection = self.republish_plugin_projections().await;
        tracing::debug!(
            plugin_skill_dirs = projection.plugin_skill_dirs.len(),
            plugin_subagents = projection.subagents.len(),
            "published plugin projections"
        );

        let mut cache = self.cache_state.write().await;
        cache.loaded = true;

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
        self.reload_count.fetch_add(1, Ordering::SeqCst);

        // Hold load_guard for the entire reload so a concurrent
        // `ensure_loaded` cannot interleave its own `load_all` between the
        // cache reset below and the `load_all` we run at the end. Without
        // this, two concurrent loads can race: each will see
        // `loaded = false`, both will execute `load_all`, and the second
        // will double-initialize `cache_state` / `active_plugin_tools` /
        // `plugin_tool_revision`.
        let _guard = self.load_guard.lock().await;

        {
            let mut state = self.cache_state.write().await;
            state.loaded = false;
        }

        // Reset hook executor (registry.clear() is called inside load_all)
        *self.hook_executor.write().await =
            HookExecutor::empty().with_consent(ShellHookConsent::shared());

        let summary = self.load_all().await?;

        // Re-register plugin MCP servers after a hot-reload so a plugin
        // installed/edited at runtime starts its servers without a restart.
        // No-op when no MCP handle is attached (CLI/test paths).
        self.sync_mcp_plugin_servers().await;

        // Service lifecycle across hot-reloads: tear down services whose
        // plugin vanished or was disabled (their registry entries are gone —
        // the ServiceManager's registration snapshots keep them stoppable),
        // then autostart services of the now-active plugin set.
        self.stop_orphaned_services().await;
        self.sync_plugin_services().await;

        Ok(summary)
    }

    /// Record that Aleph itself just wrote `path`. The hot-reload watcher
    /// will skip events for this path within the suppression TTL, preventing
    /// a write→reload→write feedback loop.
    ///
    /// Cheap to call even when no watcher is active — the tracker is always
    /// constructed.
    pub fn mark_self_write(&self, path: &Path) {
        self.internal_writes.mark(path);
    }

    /// Returns the total number of times [`Self::reload`] has executed.
    /// Used by integration tests and (in future) runtime observability.
    pub fn reload_count(&self) -> u64 {
        self.reload_count.load(Ordering::SeqCst)
    }

    /// Spawn the hot-reload watcher. Idempotent — second call returns Ok
    /// without re-spawning. Caller is expected to hold `Arc<Self>` so the
    /// watcher callback can keep the manager alive for the watcher's
    /// lifetime.
    ///
    /// The optional `notify_cb` runs on every reloadable event before the
    /// reload itself fires — boot wiring uses it to publish an
    /// `extension.reloaded` topic event on the gateway event bus so Panel UI
    /// can show a toast.
    ///
    /// Routing by [`ExtensionChangeType`]:
    /// - `Skill` → no-op (delegated to the already-wired `SkillWatcher`
    ///   which does per-skill targeted reloads — full `reload()` here would
    ///   double-fire).
    /// - `HooksConfig` → [`Self::sync_user_hooks`] only (cheap, no plugin
    ///   re-discovery).
    /// - everything else → full [`Self::reload`].
    pub async fn start_watcher(
        self: &Arc<Self>,
        notify_cb: Option<Box<dyn Fn(ExtensionChangeEvent) + Send + Sync>>,
    ) -> ExtensionResult<()> {
        self.start_watcher_with_dirs(None, notify_cb).await
    }

    /// Same as [`Self::start_watcher`] but lets the caller override the
    /// watched directory list. `None` uses the defaults (`~/.claude/`,
    /// `~/.aleph/`). Primarily used by integration tests that need to
    /// substitute a `TempDir` for the home directory.
    pub async fn start_watcher_with_dirs(
        self: &Arc<Self>,
        watch_dirs: Option<Vec<PathBuf>>,
        notify_cb: Option<Box<dyn Fn(ExtensionChangeEvent) + Send + Sync>>,
    ) -> ExtensionResult<()> {
        let mut watcher_guard = self.watcher.lock().unwrap_or_else(|e| e.into_inner());
        if watcher_guard.is_some() {
            return Ok(());
        }

        let manager = Arc::clone(self);
        let internal_writes = Arc::clone(&self.internal_writes);
        let cb_arc: Arc<Option<Box<dyn Fn(ExtensionChangeEvent) + Send + Sync>>> =
            Arc::new(notify_cb);
        // The notify-debouncer-full callback runs on a dedicated OS thread
        // outside the Tokio runtime, so we must hand it a Handle captured
        // here in async context — bare `tokio::spawn` would panic.
        let runtime = tokio::runtime::Handle::current();

        let on_event = move |event: ExtensionChangeEvent| {
            // Filter out paths Aleph itself just wrote (5s suppression).
            let filtered: Vec<PathBuf> = event
                .changed_paths
                .iter()
                .filter(|p| !internal_writes.was_recent(p))
                .cloned()
                .collect();
            if filtered.is_empty() {
                tracing::trace!(
                    paths = ?event.changed_paths,
                    "Extension watcher: suppressed (internal-write window)"
                );
                return;
            }

            let mut effective = event.clone();
            effective.changed_paths = filtered;

            // Notify outer callback (e.g. gateway event publisher).
            if let Some(cb) = cb_arc.as_ref() {
                cb(effective.clone());
            }

            // Route to the cheapest correct reload path.
            match effective.change_type {
                ExtensionChangeType::Skill => {
                    // SkillWatcher already handles SKILL.md targeted reloads;
                    // running full reload() here would double-fire.
                    tracing::trace!(
                        paths = ?effective.changed_paths,
                        "Extension watcher: skill change deferred to SkillWatcher"
                    );
                }
                ExtensionChangeType::HooksConfig => {
                    let mgr = Arc::clone(&manager);
                    runtime.spawn(async move {
                        tracing::info!(
                            paths = ?effective.changed_paths,
                            "Extension watcher: hooks config changed, reloading user hooks"
                        );
                        mgr.sync_user_hooks().await;
                    });
                }
                _ => {
                    let mgr = Arc::clone(&manager);
                    runtime.spawn(async move {
                        tracing::info!(
                            kind = ?effective.change_type,
                            paths = ?effective.changed_paths,
                            "Extension watcher: triggering full reload"
                        );
                        if let Err(e) = mgr.reload().await {
                            tracing::warn!(error = %e, "Extension hot reload failed");
                        }
                    });
                }
            }
        };

        let watcher = match watch_dirs {
            Some(dirs) => ExtensionWatcher::new_with_dirs(dirs, on_event),
            None => ExtensionWatcher::new(None, on_event),
        };

        watcher
            .start()
            .map_err(|e| ExtensionError::Runtime(e.to_string()))?;

        *watcher_guard = Some(Arc::new(watcher));
        Ok(())
    }

    /// Stop the hot-reload watcher (no-op if not started). Tests use this
    /// to release watch handles before `TempDir` drops.
    pub fn stop_watcher(&self) -> ExtensionResult<()> {
        let mut guard = self.watcher.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = guard.take() {
            w.stop()
                .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
        }
        Ok(())
    }

    /// Collect all plugin directories from discovery, deduplicating by canonical path.
    ///
    /// Origin is not tracked here — `PluginRecord::from_adapter_output` derives it
    /// from the `AdapterOutput::source` field set during manifest parsing.
    fn collect_plugin_dirs(&self) -> ExtensionResult<Vec<PathBuf>> {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        let mut result = Vec::new();

        // Project-local plugin parents: every registered project's
        // `.aleph/plugins` (+ `.aleph/plugins.local`), so project-local installs
        // are discovered alongside the global `~/.aleph/plugins` (Claude-Code
        // style). The daemon serves all registered projects from one process,
        // so discovery is union-of-all; isolating which project sees which
        // plugin at runtime is a separate, deferred concern. A failure to read
        // the registry degrades to global-only discovery.
        let project_plugin_parents: Vec<PathBuf> = crate::projects::ProjectStore::shared()
            .list()
            .map(|projects| {
                projects
                    .into_iter()
                    .filter_map(|p| p.workspace_path)
                    .flat_map(|root| {
                        [
                            root.join(".aleph/plugins"),
                            root.join(".aleph/plugins.local"),
                        ]
                    })
                    .collect()
            })
            .unwrap_or_default();

        #[cfg(test)]
        let project_plugin_parents = {
            let mut parents = project_plugin_parents;
            parents.extend(self.extra_plugin_parents.iter().cloned());
            parents
        };

        // Collect from all discovery sources: skills, commands, agents, plugins
        let all_dirs = [
            self.discovery.discover_skill_dirs(),
            self.discovery.discover_command_dirs(),
            self.discovery.discover_agent_dirs(),
            self.discovery
                .discover_plugins_with_extra(&project_plugin_parents),
        ];

        for dirs in all_dirs.into_iter().flatten() {
            for d in dirs {
                let canonical = match d.path.canonicalize() {
                    Ok(path) => path,
                    Err(e) => {
                        tracing::debug!("Failed to canonicalize path {:?}: {}", d.path, e);
                        d.path.clone()
                    }
                };
                if seen.insert(canonical) {
                    result.push(d.path);
                }
            }
        }

        Ok(result)
    }

    /// Layer user-level hook configs (`~/.aleph/hooks.json`, project files)
    /// on top of the plugin-registered hooks. Runs after
    /// [`Self::sync_hooks_from_registry`] so user entries are evaluated in
    /// the same executor pass — priority + matcher determine ordering, not
    /// load order.
    ///
    /// Idempotent: every prior entry tagged with the `user:` plugin prefix
    /// is dropped before the freshly-parsed config is appended. This lets the
    /// hot-reload watcher call this method on every `hooks.json` change
    /// without leaking duplicate registrations.
    pub(crate) async fn sync_user_hooks(&self) {
        let cwd = std::env::current_dir().ok();
        // App mode: the daemon CWD is meaningless, so also load hooks from
        // every registered project folder. The executor gates each project
        // hook so it only fires while that project is the active workspace —
        // discovery is union-of-all (daemon = one process), firing is scoped.
        // A failure to read the registry degrades to global + CWD only.
        let project_roots: Vec<PathBuf> = crate::projects::ProjectStore::shared()
            .list()
            .map(|projects| {
                projects
                    .into_iter()
                    .filter_map(|p| p.workspace_path)
                    .collect()
            })
            .unwrap_or_default();
        let user_hooks = crate::extension::hooks::load_user_hooks(cwd.as_deref(), &project_roots);
        let mut executor = self.hook_executor.write().await;
        let removed = executor.remove_by_plugin_prefix("user:");
        if user_hooks.is_empty() {
            if removed > 0 {
                tracing::info!(removed, "Cleared user-level hook configs");
            }
            return;
        }
        let count = user_hooks.len();
        for h in user_hooks {
            executor.add_hook(h);
        }
        tracing::info!(count, removed, "Loaded user-level hook configs");
    }

    /// Sync hooks from `PluginRegistry` to `HookExecutor`.
    ///
    /// Reads `HookRegistration` entries from the registry and converts them
    /// to `HookConfig` entries that `HookExecutor` understands.
    async fn sync_hooks_from_registry(&self) {
        let hook_regs: Vec<HookRegistration> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_hooks()
                .into_iter()
                .filter(|hook| {
                    registry
                        .get_plugin(&hook.plugin_id)
                        .is_some_and(|plugin| plugin.status.is_active())
                })
                .cloned()
                .collect()
        };

        let mut executor = self.hook_executor.write().await;
        // HookExecutor was already reset (via load_all's clear path or reload).
        // Convert HookRegistration → HookConfig for the executor, consuming
        // each registration by value so its fields move into the config.
        for hr in hook_regs {
            let HookRegistration {
                event,
                priority,
                handler,
                plugin_id,
                kind,
                matcher,
                actions,
                plugin_root,
                timeout_secs,
                ..
            } = hr;
            // Registrations carrying concrete actions (plugin-shipped
            // hooks.json shell hooks) dispatch those directly — through the
            // consent gate for command/http. Registrations without actions
            // are runtime (WASM) hooks: emit a live Plugin dispatch action so
            // the executor invokes the plugin's exported `handler` via the
            // process-global ExtensionManager when the event fires. The
            // `handler` field is kept for diagnostics display either way.
            let runtime_hook = actions.is_empty();
            let actions = if runtime_hook {
                vec![HookAction::Plugin {
                    plugin_id: plugin_id.clone(),
                    handler: handler.clone(),
                }]
            } else {
                actions
            };
            // Kind: explicit registration wins. Otherwise file-based action
            // hooks get the same per-event default the user-hooks loader
            // applies (blocking-capable events → interceptor, so a plugin
            // PreToolUse command hook can actually block), while runtime
            // (WASM) handler hooks keep their historical Observer default —
            // flipping them implicitly would turn a handler error into a
            // fail-closed tool block existing plugins never signed up for.
            let kind = kind.unwrap_or_else(|| {
                if runtime_hook {
                    HookKind::default()
                } else {
                    hooks::default_kind_for_event(event)
                }
            });
            let hook_config = HookConfig {
                event,
                kind,
                priority: match priority {
                    i if i <= HookPriority::System.as_i32() => HookPriority::System,
                    i if i <= HookPriority::High.as_i32() => HookPriority::High,
                    i if i >= HookPriority::Low.as_i32() => HookPriority::Low,
                    _ => HookPriority::Normal,
                },
                matcher,
                actions,
                plugin_name: plugin_id,
                plugin_root: plugin_root.unwrap_or_default(),
                handler: Some(handler),
                timeout_secs,
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
    pub const fn adapter_registry(&self) -> &AdapterRegistry {
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
                .ok_or_else(|| anyhow::anyhow!("Plugin not found: {plugin_id}"))?
        };

        // Re-parse manifest via adapter registry
        let output = self.adapter_registry.parse_dir(&root_dir)?;

        // Use permissions from adapter output (consistent with load_all path)
        let permissions = output.permissions.clone();

        // Build updated plugin record
        let mut record = PluginRecord::from_adapter_output(&output, root_dir.clone());
        if let Ok(manifest) = manifest::parse_manifest_from_dir_cached_global(&root_dir) {
            record.kind = manifest.kind;
        }

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
#[must_use]
pub fn default_plugins_dir() -> std::path::PathBuf {
    crate::discovery::aleph_plugins_dir().map_or_else(
        |_| {
            dirs::home_dir().map_or_else(
                || std::path::PathBuf::from(".aleph/plugins/installed"),
                |h| h.join(".aleph/plugins/installed"),
            )
        },
        |p| p.join("installed"),
    )
}

/// Per-plugin persistent data directory: `<plugins_root>/data/<plugin_id>/`.
///
/// Deliberately **outside** the install tree, so `plugin update` (which swaps
/// the install directory atomically) and `plugin uninstall` leave it alone.
///
/// This is the value behind the documented `${CLAUDE_PLUGIN_DATA}` /
/// `${ALEPH_PLUGIN_DATA}` manifest variables. Until 2026-08-16 those had
/// **no producer at all**: `mcp_config.rs` carried a comment saying they were
/// expanded "in the higher-level `McpManagerConfig::env` substitution path",
/// and that path did not exist — so a plugin author who used the variable got
/// the literal string `${ALEPH_PLUGIN_DATA}` handed to their process. The
/// comment was the only thing in the repo that mentioned it, which is why a
/// grep for the name found the bug's own alibi and nothing else.
#[must_use]
pub fn plugin_data_dir(plugin_id: &str) -> std::path::PathBuf {
    default_plugins_dir()
        .parent()
        .map_or_else(
            || std::path::PathBuf::from(".aleph/plugins"),
            std::path::Path::to_path_buf,
        )
        .join("data")
        .join(plugin_id)
}

/// Reject a plugin install destination whose leaf is a symlink (including
/// dangling ones). `git2::Repository::clone` will dereference symlinks at
/// the leaf and clone inside the target, which lets a pre-planted symlink
/// escape the authoritative plugins root.
///
/// We intentionally do NOT walk past the immediate parent to check for
/// symlinks above. System paths (e.g. macOS's `/var` → `/private/var`)
/// legitimately resolve through symlinks; bounding the walk at `root`
/// (the caller's authoritative plugins root) keeps the check targeted at
/// the install boundary. See `ensure_plugin_root_within_authoritative`
/// for the post-clone canonicalization check that catches parent escapes.
///
/// This is a snapshot check at the time of install. Concurrent attackers
/// can still race the filesystem between the check and the clone. Per
/// project policy, we do not attempt to defend every local TOCTOU; we make
/// the common case (pre-planted symlink) fail closed.
pub fn ensure_plugin_destination_is_safe(
    root: &std::path::Path,
    dest_path: &std::path::Path,
) -> Result<(), String> {
    if let Ok(meta) = std::fs::symlink_metadata(dest_path) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "refusing to clone into symlink at {}",
                dest_path.display()
            ));
        }
    }
    if let Some(parent) = dest_path.parent() {
        let mut current = parent.to_path_buf();
        loop {
            if !current.starts_with(root) || current == *root {
                break;
            }
            if let Ok(meta) = std::fs::symlink_metadata(&current) {
                if meta.file_type().is_symlink() {
                    return Err(format!(
                        "refusing to clone through symlinked parent {}",
                        current.display()
                    ));
                }
            }
            match current.parent() {
                Some(p) if !p.as_os_str().is_empty() => current = p.to_path_buf(),
                _ => break,
            }
        }
    }
    Ok(())
}

/// Verify a post-clone directory resolves inside the authoritative plugins
/// root (after canonicalizing both). Catches a clone whose destination was
/// reachable through a symlink we missed at install time, or whose root was
/// later re-linked by some other process.
pub fn ensure_plugin_root_within_authoritative(
    root: &std::path::Path,
    dest: &std::path::Path,
) -> Result<(), String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize plugins root {}: {e}", root.display()))?;
    let canonical_dest = dest.canonicalize().map_err(|e| {
        format!(
            "cannot canonicalize plugin destination {}: {e}",
            dest.display()
        )
    })?;
    if !canonical_dest.starts_with(&canonical_root) {
        return Err(format!(
            "plugin destination {} is outside authoritative plugins root {}",
            dest.display(),
            root.display()
        ));
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_agent_to_def_maps_subagent_and_drops_body() {
        use crate::extension::types::AgentMode as ExtMode;
        use crate::extension::AgentRegistration;

        let mut reg = AgentRegistration {
            name: "deployer".into(),
            description: Some("Ship the app".into()),
            content: "SYSTEM PROMPT BODY — must be dropped like disk agents".into(),
            mode: ExtMode::Subagent,
            ..Default::default()
        };
        reg.steps = Some(12);
        reg.model = Some("fast".into());
        reg.tools = Some(std::collections::HashMap::from([
            ("bash".to_string(), true),
            ("file_write".to_string(), false),
        ]));

        let def = plugin_agent_to_def(&reg).expect("subagent converts");
        assert_eq!(def.id, "deployer");
        assert_eq!(def.description, "Ship the app");
        assert_eq!(def.source, crate::agents::AgentSource::Plugin);
        assert_eq!(def.mode, crate::agents::AgentMode::SubAgent);
        assert_eq!(def.max_iterations, Some(12));
        assert_eq!(def.model_hint.as_deref(), Some("fast"));
        assert!(def.is_tool_allowed("bash"), "declared-allowed tool");
        assert!(!def.is_tool_allowed("file_write"), "declared-denied tool");

        // A primary-only plugin agent is not a delegatable sub-agent.
        let primary = AgentRegistration {
            name: "ui".into(),
            mode: ExtMode::Primary,
            ..Default::default()
        };
        assert!(plugin_agent_to_def(&primary).is_none());

        // A blank name has no id to delegate to → rejected.
        let blank = AgentRegistration {
            name: "   ".into(),
            mode: ExtMode::Subagent,
            ..Default::default()
        };
        assert!(plugin_agent_to_def(&blank).is_none());
    }

    /// Build an isolated manager whose only project plugin root is `dir`, with
    /// the durable plugin document redirected to a temp file.
    ///
    /// `ALEPH_HOME` is deliberately **not** touched: it is a process-global
    /// switch and libtest runs tests in parallel, so two tests redirecting it
    /// would silently fight. The config path is a constructor parameter for
    /// exactly this reason.
    async fn isolated_manager(dir: &std::path::Path) -> (ExtensionManager, PathBuf) {
        let cfg_path = dir.join("plugins.toml");
        let manager = ExtensionManager::new(ExtensionConfig {
            discovery: DiscoveryConfig {
                working_dir: dir.to_path_buf(),
                scan_claude_dirs: false,
                scan_project_dirs: false,
                max_upward_depth: 0,
            },
            plugins_config_path: Some(cfg_path.clone()),
            extra_plugin_parents: vec![dir.join("plugins")],
            owner_trust: None,
        })
        .await
        .unwrap();
        (manager, cfg_path)
    }

    fn write_project_plugin(root: &std::path::Path, id: &str) {
        let plugin_dir = root.join("plugins").join(id);
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            plugin_dir.join(".claude-plugin/plugin.toml"),
            format!("name = \"{id}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
    }

    /// The regression this whole round exists for.
    ///
    /// `aleph plugin disable X` used to write a `<plugin>/.disabled` marker
    /// that **nothing ever read** (four writers, zero readers), so the disable
    /// lasted exactly as long as the process. This asserts the load path
    /// consults the durable document: delete the `is_enabled` check in
    /// `load_all` and this fails by name.
    #[tokio::test]
    async fn a_disabled_plugin_stays_inactive_across_a_fresh_load() {
        let tmp = tempfile::tempdir().unwrap();
        write_project_plugin(tmp.path(), "quiet-plugin");
        let (manager, cfg_path) = isolated_manager(tmp.path()).await;

        // Control group: with no preference recorded it loads active. Without
        // this half, "not active" could just mean "never discovered".
        manager.load_all().await.unwrap();
        let before = manager.get_plugin_record("quiet-plugin").await;
        assert!(
            before.as_ref().is_some_and(|r| r.status.is_active()),
            "control: an untouched plugin must load active, got {:?}",
            before.map(|r| r.status)
        );

        // Record the operator's preference the way every toggle face does.
        assert!(manager.set_plugin_enabled("quiet-plugin", false).await);
        assert!(
            !crate::extension::plugin_state::PluginsConfig::load(&cfg_path)
                .is_enabled("quiet-plugin"),
            "the preference must reach disk, not just the in-memory registry"
        );

        // A brand-new manager = the restart the marker file never survived.
        let (restarted, _) = isolated_manager(tmp.path()).await;
        restarted.load_all().await.unwrap();
        let after = restarted.get_plugin_record("quiet-plugin").await;
        assert!(
            after.is_some(),
            "a disabled plugin must still be listable so it can be re-enabled"
        );
        assert!(
            !after.unwrap().status.is_active(),
            "the disable did not survive the restart — is `load_all` still \
             reading plugins.toml?"
        );
    }

    /// A disabled plugin is registered *with* its capabilities so a runtime
    /// re-enable has something behind it; only the active-status filter keeps
    /// it away from the model. Skipping registration instead would make
    /// `set_plugin_enabled(id, true)` flip a status pointing at nothing.
    #[tokio::test]
    async fn re_enabling_needs_no_reload() {
        let tmp = tempfile::tempdir().unwrap();
        write_project_plugin(tmp.path(), "wakeable");
        let (manager, cfg_path) = isolated_manager(tmp.path()).await;

        crate::extension::plugin_state::PluginsConfig {
            entries: [(
                "wakeable".to_string(),
                crate::extension::plugin_state::PluginEntryConfig {
                    enabled: Some(false),
                },
            )]
            .into_iter()
            .collect(),
        }
        .save(&cfg_path)
        .await
        .unwrap();

        let (manager2, _) = isolated_manager(tmp.path()).await;
        drop(manager);
        manager2.load_all().await.unwrap();
        assert!(!manager2
            .get_plugin_record("wakeable")
            .await
            .unwrap()
            .status
            .is_active());

        manager2.set_plugin_enabled("wakeable", true).await;
        assert!(
            manager2
                .get_plugin_record("wakeable")
                .await
                .unwrap()
                .status
                .is_active(),
            "re-enable must take effect without a full reload"
        );
    }

    /// A `.disabled` marker written by an older build must not be silently
    /// ignored — the operator's intent predates this change.
    #[tokio::test]
    async fn a_legacy_disabled_marker_is_migrated_then_removed() {
        let tmp = tempfile::tempdir().unwrap();
        write_project_plugin(tmp.path(), "old-timer");
        let marker = tmp.path().join("plugins/old-timer/.disabled");
        std::fs::write(&marker, "").unwrap();

        let (manager, cfg_path) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();

        assert!(
            !manager
                .get_plugin_record("old-timer")
                .await
                .unwrap()
                .status
                .is_active(),
            "the legacy marker's intent must be honoured on the migrating load"
        );
        assert!(
            !crate::extension::plugin_state::PluginsConfig::load(&cfg_path).is_enabled("old-timer"),
            "and be written into the durable document"
        );
        assert!(
            !marker.exists(),
            "the marker must be removed once migrated, or it is a second source"
        );
    }

    #[tokio::test]
    async fn sync_mcp_plugin_servers_is_noop_without_handle() {
        // With no MCP manager attached (the default for CLI/test paths), the
        // sync must short-circuit to 0 *before* touching the loader/registry —
        // so it never even forces an extension load.
        let manager = ExtensionManager::with_defaults().await.unwrap();
        assert_eq!(manager.sync_mcp_plugin_servers().await, 0);
    }

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

    #[cfg(unix)]
    #[test]
    fn safe_destination_rejects_existing_symlink_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let dest = dir.path().join("plugin");
        std::os::unix::fs::symlink("/tmp/anywhere", &dest).unwrap();
        let err = ensure_plugin_destination_is_safe(&root, &dest).unwrap_err();
        assert!(err.contains("symlink"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn safe_destination_rejects_dangling_symlink_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let dest = dir.path().join("plugin");
        std::os::unix::fs::symlink("/definitely/does/not/exist/here", &dest).unwrap();
        assert!(ensure_plugin_destination_is_safe(&root, &dest).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn safe_destination_rejects_symlinked_parent_within_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let target = root.join("real");
        std::fs::create_dir(&target).unwrap();
        let linked_parent = root.join("linked");
        std::os::unix::fs::symlink(&target, &linked_parent).unwrap();
        let dest = linked_parent.join("plugin");
        let err = ensure_plugin_destination_is_safe(&root, &dest).unwrap_err();
        assert!(err.contains("symlinked parent"), "got: {err}");
    }

    #[test]
    fn safe_destination_accepts_nonexistent_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let dest = dir.path().join("nope");
        let res = ensure_plugin_destination_is_safe(&root, &dest);
        assert!(res.is_ok(), "got: {res:?}");
    }

    #[test]
    fn safe_destination_accepts_real_directory_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let dest = dir.path().join("real");
        std::fs::create_dir(&dest).unwrap();
        let res = ensure_plugin_destination_is_safe(&root, &dest);
        assert!(res.is_ok(), "got: {res:?}");
    }

    #[test]
    fn root_within_authoritative_accepts_path_inside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let inside = root.join("child");
        std::fs::create_dir(&inside).unwrap();
        assert!(
            ensure_plugin_root_within_authoritative(&root, &inside).is_ok(),
            "a directory inside the authoritative root must be accepted"
        );
    }

    #[test]
    fn root_within_authoritative_rejects_outside_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let err = ensure_plugin_root_within_authoritative(&root, &outside).unwrap_err();
        assert!(err.contains("outside authoritative"), "got: {err}");
    }
}
