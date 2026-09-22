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
mod lifecycle;
mod loader;
pub mod marketplace;
pub mod runtime;
pub mod validation;
pub mod visibility;

pub mod capability;
pub mod effects;
pub mod registrar;

mod error;
pub(crate) mod manager_global;
pub mod manifest;
pub mod mcp_config;
mod plugin_ops;
pub mod plugin_secrets;
pub mod plugin_state;
pub mod plugin_trust;
pub mod plugin_vars;
mod projection;
pub mod registry;
mod service_manager;
mod service_ops;
mod skill_ops;
mod skill_tool;
mod slash_effect;
mod template;
mod types;
pub mod watcher;

pub use effects::{
    async_disposer, sync_disposer, DisposeOutcome, DisposeReport, Disposer, EffectScope, PluginId,
};
pub use error::*;
pub use lifecycle::{MountError, ReloadReport, UnmountError};
pub use loader::PluginLoader;
pub use manager_global::{
    decline_extension_manager, init_extension_manager, is_extension_manager_initialized,
    try_extension_manager,
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
    pub extra_plugin_parents: Vec<crate::discovery::ProjectPluginParent>,
    // NOTE: there is deliberately no `owner_trust` field here.
    //
    // One existed, alongside an `OwnerTrustPolicyConfig` DTO, and had zero
    // producers: the only production constructor is `with_defaults()`, which
    // passes `ExtensionConfig::default()`, so the policy was permissive on
    // every install and `PluginStatus::Blocked` was unreachable. The durable
    // answer now lives in `plugins.toml` (`[trust] enforce` plus per-entry
    // `trusted`), which the manager already loads two statements above. Adding
    // an override here as well would give "may this plugin load" two sources —
    // and the one an operator edits would be the one that loses.
}

/// One directory found by discovery, with the facts the registry walk needs
/// and the scan is the only place that knows.
struct DiscoveredExtensionDir {
    path: PathBuf,
    /// Where it came from, per [`PluginOrigin::classify`].
    origin: PluginOrigin,
    /// Who may see it, per [`visibility::ScopeKey::from_discovery`].
    scope_key: visibility::ScopeKey,
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

    /// `plugin_id → ScopeKey` for every registered row, refreshed in the same
    /// registry read as [`Self::active_plugin_tools`]. Read by every visibility
    /// face through [`Self::plugin_visible`]; `Option::None` for an id the
    /// registry never saw, which the predicate reads as "not visible".
    plugin_scope_keys: Arc<StdRwLock<HashMap<String, visibility::ScopeKey>>>,

    /// Monotonic revision for active plugin tool snapshot changes.
    plugin_tool_revision: Arc<AtomicU64>,

    /// Guard to serialize concurrent `load_all()` calls
    load_guard: Mutex<()>,

    /// Memory extension registry (Spec 4 Task 11).
    /// When set, `mount` registers a plugin's `[memory]` section as a
    /// `McpMemoryExtension` (`memory_extension` step); `None` records a skip.
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
    extra_plugin_parents: Vec<crate::discovery::ProjectPluginParent>,

    /// Live MCP manager handle, used by `mount` to register plugin-owned MCP
    /// servers as **transient** (runtime-only) servers (`mcp_server` step).
    /// `None` until [`Self::set_mcp_handle`] at boot; CLI/test paths leave it
    /// unset and the step is recorded as skipped. Wrapped in `RwLock<Option<_>>`
    /// because the manager is behind an `Arc` by the time the MCP actor
    /// materialises, mirroring the `memory_registry` injection pattern.
    mcp_handle: crate::sync_primitives::RwLock<Option<crate::mcp::McpManagerHandle>>,

    /// Live tool catalog, so `mount` can register a plugin's `commands/*.md`
    /// as slash entries and `unmount` can remove them. `None` until
    /// [`Self::set_tool_catalog`] is called at server boot (CLI/test paths
    /// leave it unset, and the `slash_command` step is recorded as skipped).
    /// Same injection shape as `mcp_handle` and `memory_registry`, for the
    /// same reason: the manager is behind an `Arc` before the catalog exists.
    tool_catalog: crate::sync_primitives::RwLock<Option<Arc<crate::tool_metadata::ToolCatalog>>>,

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
    /// assert at-most-once reload behaviour on adjacent watcher events.
    reload_count: AtomicU64,

    /// One [`EffectScope`] per mounted plugin — everything `mount` put into
    /// the runtime, owned here so `unmount` can take it all back out. A std
    /// mutex, never held across an `await`: `lifecycle.rs` removes the scope
    /// under the lock and disposes it after.
    scopes: StdMutex<HashMap<String, effects::EffectScope>>,

    /// Join handles of the per-plugin server-start watchers
    /// (`lifecycle.rs::watch_server_starts`), so `activation_settled()` can
    /// wait for the MCP half of a mount to reach its verdict. Std mutex,
    /// never held across an `await`.
    activation_watchers: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
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
        let loaded_plugins_config =
            crate::extension::plugin_state::PluginsConfig::load(&plugins_config_path);
        // Derived here, once, from the document just read. The policy has a
        // live handle (`set_owner_trust_policy`) because the trust tool has to
        // change it without a restart, but its *initial* value has exactly one
        // source — re-deriving it anywhere else would be a second answer to
        // "may this plugin load".
        let owner_trust_policy = loaded_plugins_config.owner_trust_policy();
        let plugins_config = Arc::new(RwLock::new(loaded_plugins_config));

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
            plugin_scope_keys: Arc::new(StdRwLock::new(HashMap::new())),
            plugin_tool_revision: Arc::new(AtomicU64::new(0)),
            load_guard: Mutex::new(()),
            memory_registry: crate::sync_primitives::RwLock::new(None),
            mcp_handle: crate::sync_primitives::RwLock::new(None),
            tool_catalog: crate::sync_primitives::RwLock::new(None),
            watcher: StdMutex::new(None),
            internal_writes: Arc::new(InternalWriteTracker::default()),
            reload_count: AtomicU64::new(0),
            scopes: StdMutex::new(HashMap::new()),
            activation_watchers: StdMutex::new(Vec::new()),
            owner_trust_policy: Arc::new(crate::sync_primitives::RwLock::new(owner_trust_policy)),
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

    /// Inject the memory extension registry after construction (Spec 4 Task 11).
    ///
    /// Safe to call on `&Arc<ExtensionManager>`. Subsequent mounts register
    /// manifests declaring a `[memory]` section as `McpMemoryExtension`
    /// entries (`memory_extension` step). Call it at boot BEFORE the first
    /// `load_all` (see `agent_init::boot_order_tests`).
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

    /// Inject the live MCP manager handle. Call it at boot BEFORE the first
    /// `load_all` (see `agent_init::boot_order_tests`); a mount that runs
    /// without it records `mcp_server` as skipped. Servers a plugin starts
    /// before the MCP tool bridge is spawned are picked up by the bridge's
    /// boot-time reconcile against the servers already running, so the early
    /// install does not lose tool registrations.
    pub fn set_mcp_handle(&self, handle: crate::mcp::McpManagerHandle) {
        *self.mcp_handle.write().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }

    /// Inject the live tool catalog after construction. Call once at server
    /// boot BEFORE the first `load_all`, so every plugin's slash entries are
    /// registered by its own mount rather than by a boot-time catch-up.
    pub fn set_tool_catalog(&self, catalog: Arc<crate::tool_metadata::ToolCatalog>) {
        *self.tool_catalog.write().unwrap_or_else(|e| e.into_inner()) = Some(catalog);
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

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

        // `load_guard` is held above; `load_all` would take it again.
        self.load_all_locked().await?;
        Ok(())
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
    /// Used by integration tests to assert at-most-once reload behaviour
    /// on adjacent watcher events.
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
                        match mgr.reload().await {
                            Ok(report) if !report.failed.is_empty() => tracing::warn!(
                                failed = ?report.failed.iter().map(|(id, e)| format!("{id}: {e}")).collect::<Vec<_>>(),
                                "Extension hot reload: some plugins did not mount"
                            ),
                            Ok(_) => {}
                            Err(e) => tracing::warn!(error = %e, "Extension hot reload failed"),
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

    /// Collect all extension directories from discovery, deduplicating by
    /// canonical path.
    ///
    /// Each entry carries its origin and whether the owner trust policy applies
    /// to it. This used to return bare paths, and the doc here used to say
    /// origin was derived later "from the `AdapterOutput::source` field set
    /// during manifest parsing" — true in form, empty in effect: all six
    /// manifest adapters hardcode `PluginOrigin::Global` at their construction
    /// sites, so the only origin any plugin ever had was `Global`.
    ///
    /// That was harmless while `PluginOrigin`'s only consumer was `priority()`,
    /// where a constant answer looks like a working shadowing rule. It stopped
    /// being harmless the moment the owner trust policy gained a producer.
    ///
    /// # What this walk covers (D-2, 2026-09-21)
    ///
    /// This used to also union the skill, command and agent directories, so
    /// on a stock install ~88 of the ~91 entries were bundled *skills*, not
    /// plugins — no manifest adapter can match a skill/agent/command dir, so
    /// every one of them became an `error` row in `plugins.list`
    /// ("`No manifest adapter matched directory`"). A `trust_gated` flag was
    /// briefly added to exempt those rows from the owner trust policy; it was
    /// a no-op (the rows were errors *before* the trust gate ran, gate or no
    /// gate) and was retracted (R10). The union itself carried no plugin the
    /// plugin scanner didn't already find, so it has been removed: this walk
    /// now returns plugin roots only, from `discover_plugins_with_extra`.
    fn collect_plugin_dirs(&self) -> ExtensionResult<Vec<DiscoveredExtensionDir>> {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        let mut result = Vec::new();

        // Project-local plugin parents: every registered project's
        // `.aleph/plugins` (+ `.aleph/plugins.local`), so project-local installs
        // are discovered alongside the global `~/.aleph/plugins` (Claude-Code
        // style). The daemon serves all registered projects from one process,
        // so discovery is union-of-all; each parent is handed to the scanner
        // WITH the project it belongs to, and that root becomes the row's
        // `scope_key` — the fact the per-face visibility gate reads. A failure
        // to read the registry degrades to global-only discovery.
        let project_plugin_parents: Vec<crate::discovery::ProjectPluginParent> =
            crate::projects::ProjectStore::shared()
                .list()
                .map(|projects| {
                    projects
                        .into_iter()
                        .filter_map(|p| p.workspace_path)
                        .flat_map(|root| {
                            [
                                crate::discovery::ProjectPluginParent {
                                    project_root: root.clone(),
                                    dir: root.join(".aleph/plugins"),
                                },
                                crate::discovery::ProjectPluginParent {
                                    project_root: root.clone(),
                                    dir: root.join(".aleph/plugins.local"),
                                },
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

        // Plugin roots come from the plugin scanner only. A skill, agent or
        // command found by the component scanners is a component, not a
        // plugin root — no manifest adapter can match one — so unioning them
        // here only manufactured `Error` rows in `plugins.list` (88 of 89 on
        // the first real-machine run of the 2026-09-20 round, D-2).
        for d in self
            .discovery
            .discover_plugins_with_extra(&project_plugin_parents)?
        {
            let canonical = match d.path.canonicalize() {
                Ok(path) => path,
                Err(e) => {
                    tracing::debug!("Failed to canonicalize path {:?}: {}", d.path, e);
                    d.path.clone()
                }
            };
            if seen.insert(canonical) {
                result.push(DiscoveredExtensionDir {
                    origin: PluginOrigin::classify(d.source()),
                    scope_key: visibility::ScopeKey::from_discovery(&d),
                    path: d.path,
                });
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
    /// to `HookConfig` entries that `HookExecutor` understands. Each hook is
    /// stamped with the OWNING ROW's `scope_key`: a hook shipped by a
    /// project-local plugin is visible only inside that project, exactly
    /// like the plugin itself.
    async fn sync_hooks_from_registry(&self) {
        let hook_regs: Vec<(HookRegistration, visibility::ScopeKey)> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_hooks()
                .into_iter()
                .filter_map(|hook| {
                    registry
                        .get_plugin(&hook.plugin_id)
                        .filter(|plugin| plugin.status.is_active())
                        .map(|plugin| (hook.clone(), plugin.scope_key.clone()))
                })
                .collect()
        };

        let mut executor = self.hook_executor.write().await;
        // HookExecutor was already reset by `lifecycle.rs::after_transition`.
        // Convert HookRegistration → HookConfig for the executor, consuming
        // each registration by value so its fields move into the config.
        for (hr, scope_key) in hook_regs {
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
                scope_key,
            };
            executor.add_hook(hook_config);
        }
    }

    fn build_active_plugin_tool_index(
        registry: &PluginRegistry,
    ) -> (
        HashMap<String, ToolRegistration>,
        HashMap<String, visibility::ScopeKey>,
    ) {
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

        let scope_keys = registry
            .list_plugins()
            .into_iter()
            .map(|p| (p.id.clone(), p.scope_key.clone()))
            .collect();

        (active_tools, scope_keys)
    }

    async fn refresh_active_plugin_tools(&self) {
        let (active_tools, scope_keys) = {
            let registry = self.plugin_registry.read().await;
            Self::build_active_plugin_tool_index(&registry)
        };

        *self
            .active_plugin_tools
            .write()
            .unwrap_or_else(|e| e.into_inner()) = active_tools;
        *self
            .plugin_scope_keys
            .write()
            .unwrap_or_else(|e| e.into_inner()) = scope_keys;
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

    /// The visibility key of a registered plugin, any status. `None` = unknown id.
    pub fn plugin_scope_key(&self, plugin_id: &str) -> Option<visibility::ScopeKey> {
        self.plugin_scope_keys
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(plugin_id)
            .cloned()
    }

    /// May a session in `ctx` see anything this plugin contributes?
    ///
    /// Fail-closed on an unknown id: the registry is the only authority on
    /// where a plugin came from, and a plugin it cannot name has no key to
    /// compare. Every capability face (tool index, skills, sub-agents, slash
    /// list, MCP bridge) asks this; hooks compare their own stamped key with
    /// the same `visible_to`.
    pub fn plugin_visible(&self, plugin_id: &str, ctx: &visibility::VisibilityCtx) -> bool {
        self.plugin_scope_key(plugin_id)
            .is_some_and(|key| visibility::visible_to(&key, ctx))
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
            extra_plugin_parents: vec![crate::discovery::ProjectPluginParent {
                project_root: dir.to_path_buf(),
                dir: dir.join("plugins"),
            }],
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

    /// D-2: a component found by the skill / agent / command scanners is not a
    /// plugin root — no manifest adapter can match it, so under the old union
    /// every `<scan>/skills/<x>` became an `Error` row in `plugins.list`
    /// (88 of 89 rows on the first real-machine run). The project `.aleph/`
    /// is reached through the upward walk (`max_upward_depth: 1` = the
    /// working dir itself); the real plugin is the positive control.
    #[tokio::test]
    async fn component_dirs_are_not_plugin_candidates() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join(".aleph/skills/planted-skill");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: planted-skill\n---\n").unwrap();
        write_project_plugin(dir.path(), "real-plugin");

        let manager = ExtensionManager::new(ExtensionConfig {
            discovery: DiscoveryConfig {
                working_dir: dir.path().to_path_buf(),
                scan_claude_dirs: false,
                scan_project_dirs: true,
                max_upward_depth: 1,
            },
            plugins_config_path: Some(dir.path().join("plugins.toml")),
            extra_plugin_parents: vec![crate::discovery::ProjectPluginParent {
                project_root: dir.path().to_path_buf(),
                dir: dir.path().join("plugins"),
            }],
        })
        .await
        .unwrap();

        let found: Vec<PathBuf> = manager
            .collect_plugin_dirs()
            .unwrap()
            .into_iter()
            .map(|d| d.path)
            .collect();
        assert!(
            found.iter().any(|p| p.ends_with("plugins/real-plugin")),
            "positive control: the real plugin must be discovered: {found:?}"
        );
        assert!(
            !found.iter().any(|p| p.ends_with("skills/planted-skill")),
            "a skill dir is a component, not a plugin root: {found:?}"
        );
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
        // `Config::load()` writes a default config when none exists, so an
        // un-isolated run of this test targets the real `~/.aleph/config.toml`
        // — invisible on a developer box that already has one, fatal on CI.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
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

    /// A disabled plugin keeps a listable `Disabled` row (no capability rows),
    /// and `set_plugin_enabled(id, true)` goes through `mount`, which registers
    /// the capabilities itself — so a runtime re-enable has something behind
    /// it without a full reload.
    #[tokio::test]
    async fn re_enabling_needs_no_reload() {
        // `Config::load()` writes a default config when none exists, so an
        // un-isolated run of this test targets the real `~/.aleph/config.toml`
        // — invisible on a developer box that already has one, fatal on CI.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_project_plugin(tmp.path(), "wakeable");
        let (manager, cfg_path) = isolated_manager(tmp.path()).await;

        crate::extension::plugin_state::PluginsConfig {
            entries: [(
                "wakeable".to_string(),
                crate::extension::plugin_state::PluginEntryConfig {
                    enabled: Some(false),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
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
        // `Config::load()` writes a default config when none exists, so an
        // un-isolated run of this test targets the real `~/.aleph/config.toml`
        // — invisible on a developer box that already has one, fatal on CI.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_project_plugin(tmp.path(), "old-timer");
        let marker = tmp.path().join("plugins/old-timer/.disabled");
        tokio::fs::write(&marker, "").await.unwrap();

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
        assert!(!loader.is_wasm_runtime_active());
        assert!(loader.loaded_plugin_ids().is_empty());
    }

    #[tokio::test]
    async fn test_extension_manager_has_service_manager() {
        let manager = ExtensionManager::with_defaults().await.unwrap();
        let service_manager = manager.get_service_manager().await;
        assert!(service_manager.list_services().is_empty());
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

    #[tokio::test]
    async fn plugin_visible_answers_from_the_rows_key_and_fails_closed_on_unknown_ids() {
        // `Config::load()` writes a default config when none exists, so an
        // un-isolated run of this test targets the real `~/.aleph/config.toml`
        // — invisible on a developer box that already has one, fatal on CI.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        use crate::extension::visibility::{ScopeKey, VisibilityCtx};
        let dir = tempfile::tempdir().unwrap();
        write_project_plugin(dir.path(), "p2-vis");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();

        let here = VisibilityCtx {
            project_root: Some(crate::extension::visibility::canonical_root(dir.path())),
        };
        let elsewhere = VisibilityCtx { project_root: None };
        assert_eq!(
            manager.plugin_scope_key("p2-vis"),
            Some(ScopeKey::project(dir.path()))
        );
        assert!(manager.plugin_visible("p2-vis", &here));
        assert!(!manager.plugin_visible("p2-vis", &elsewhere));
        // Never registered → not visible anywhere. `Some(true)` here would let
        // a face show a tool whose owner the registry cannot name.
        assert!(!manager.plugin_visible("never-registered", &here));
        assert_eq!(manager.plugin_scope_key("never-registered"), None);
    }
}
