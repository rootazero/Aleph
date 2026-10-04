//! The single place where plugin state becomes a **process-global projection**
//! — the *view* half of the plugin lifecycle.
//!
//! # Effects and views (spec 2026-09-20 §3.1, ruling U8 "C")
//!
//! A plugin's footprint on the running process splits by one question: **does
//! it have an inverse?**
//!
//! * **Effects** have one, and live in the plugin's `EffectScope`
//!   (`effects/scope.rs`): the registry row, the WASM module, transient MCP
//!   servers, background services, the memory extension, slash entries. Each
//!   registration returns a `Disposer`; unmount (`lifecycle.rs`) disposes
//!   the scope, which runs the list in reverse (`effects/scope.rs`). Guard:
//!   `effects::census` (G1) and `lifecycle::tests::g2_*` (G2).
//! * **Views** have none but can be recomputed from the registry, and that is
//!   what this module does:
//!   - `utils::paths::PLUGIN_SKILL_DIRS` — read by `get_all_skills_dirs`, i.e.
//!     the search set of the `skill_read` / `skill_list` tools, and by
//!     `SkillSystem::scan_roots`, i.e. the scan below: it is the ONE source
//!     of plugin skill dirs for both (D-5).
//!   - `agents::PLUGIN_SUBAGENTS` — read by `AgentRegistry::resolve`
//!     (delegation) and the harness prompt builder (`<available_agents>`).
//!   - `SkillSystem` — the scan that feeds the model's `<available_skills>`
//!     index; `init` receives the base dirs only.
//!   - `ExtensionManager::active_plugin_tools` — the tool-name index.
//!     (The hook executor is the fifth view; see `Views::after_transition`
//!     for how it is rebuilt.)
//!
//! **One function derives the whole view set from the registry, and exactly
//! one trigger calls it: `lifecycle.rs::after_transition`**, once at the end of
//! every public lifecycle primitive (`mount` / `unmount` / `reload_plugin` /
//! `reload` / `load_all`) and once from the server-start watcher when a
//! readiness write changes a plugin's activity (`Pending` → `Error`) — the
//! one activation change that happens after a primitive returned. Both run
//! under `load_guard`. The trigger's inputs are [`Views`], so the watcher (a
//! spawned task, no `&ExtensionManager`) runs the same function. Guard:
//! `tests::publishing_plugin_projections_has_exactly_one_author` (G3) pins
//! both the publish calls to this file and the trigger to `lifecycle.rs`.
//!
//! # What was narrowed, and why
//!
//! Three rounds (2026-08-15 dsh, 08-16 plugin-system, 08-19 compat) ruled
//! "对照 Cordis 但架构不移植", and this comment used to say the derivation was
//! the cheaper equivalent of a fiber's `dispose()` because "every path that
//! can change plugin activation calls it". That was a list (判据 §5): the
//! derivation covered three surfaces, and four effects outside it leaked
//! past a disable — memory extensions (no unregister), slash entries
//! (boot-only), MCP servers on re-enable (only `reload()` re-added them), and
//! the narrow `reload_plugin` twin. The 2026-09-20 round kept the rulings'
//! substance (no DI container, no Proxy context, no cascade restart, no HMR)
//! and absorbed the one thing Cordis actually enforces: **a registration
//! returns its disposer and the plugin handle owns it**
//! (scan-dsh-cordis.md Top-8 #1). `src/harness/` is untouched.
//!
//! # The bug the single derivation fixed (still true)
//!
//! Before this module there were two authors of the derivation — `load_all`
//! and `set_plugin_enabled` — and **they disagreed about the predicate**:
//!
//! | | skill dirs | sub-agents |
//! |---|---|---|
//! | `load_all` | `list_plugins()` — every status | `list_agents()` — unfiltered |
//! | `set_plugin_enabled` | `list_active_plugins()` | filtered by `status.is_active()` |
//!
//! So a boot (or any `reload()`, which the file watcher triggers) published the
//! skills and sub-agents of plugins that were **disabled, shadowed, or failed to
//! load** while a runtime toggle used the correct predicate. The predicate is
//! stated once, in [`Views::derive_plugin_projection`].

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::RwLock;

use super::hooks::HookExecutor;
use super::visibility::ScopeKey;
use super::{ExtensionManager, PluginRegistry, ToolRegistration};
use crate::discovery::DiscoveryManager;
use crate::sync_primitives::{Arc, AtomicU64, RwLock as StdRwLock};

/// The handles the views are derived from and published to — exactly what
/// `lifecycle.rs::after_transition` touches, and nothing else of the manager.
/// Cloned out of the [`ExtensionManager`] (every field is a shared handle, so
/// a clone is the same state, not a copy of it) so the server-start watcher,
/// a spawned task that holds no `&ExtensionManager`, can run the same
/// recomputation when its readiness write changes a plugin's activity.
#[derive(Clone)]
pub(super) struct Views {
    pub(super) hook_executor: Arc<RwLock<HookExecutor>>,
    pub(super) plugin_registry: Arc<RwLock<PluginRegistry>>,
    pub(super) discovery: Arc<DiscoveryManager>,
    pub(super) skill_system: crate::skill::SkillSystem,
    pub(super) active_plugin_tools: Arc<StdRwLock<HashMap<String, ToolRegistration>>>,
    pub(super) plugin_scope_keys: Arc<StdRwLock<HashMap<String, ScopeKey>>>,
    pub(super) plugin_tool_revision: Arc<AtomicU64>,
}

impl ExtensionManager {
    /// This manager's [`Views`] handles.
    pub(super) fn views(&self) -> Views {
        Views {
            hook_executor: Arc::clone(&self.hook_executor),
            plugin_registry: Arc::clone(&self.plugin_registry),
            discovery: Arc::clone(&self.discovery),
            skill_system: self.skill_system.clone(),
            active_plugin_tools: Arc::clone(&self.active_plugin_tools),
            plugin_scope_keys: Arc::clone(&self.plugin_scope_keys),
            plugin_tool_revision: Arc::clone(&self.plugin_tool_revision),
        }
    }
}

/// Everything a plugin set projects onto process-global state.
///
/// Derived from the registry in one pass so the *same* activation predicate
/// decides every surface; publishing is a separate step so the derivation can
/// be unit-tested without touching global state.
#[derive(Debug, Default, Clone)]
pub(crate) struct PluginProjection {
    /// `<root_dir>/skills` of every **active** plugin, with its owner's
    /// REGISTRY id and `ScopeKey`, de-duplicated and in registry order. Feeds
    /// both the `SkillSystem` scan (bare dirs — it indexes everything the
    /// process can see; per-request narrowing happens at read time via
    /// `retain_visible_plugin_skills`) and `publish_plugin_skill_dirs`
    /// (dir + id + key — the `skill_read` search set filters by key directly,
    /// and `skill::guess_source` classifies a skill under the id published
    /// with the dir that covers it, so the owner the faces look up is the one
    /// the registry names).
    pub(crate) plugin_skill_dirs: Vec<crate::utils::paths::PublishedPluginSkillDir>,
    /// Sub-agents contributed by **active** plugins, each with its owner's
    /// `ScopeKey` — [`crate::agents::visible_plugin_subagents`] filters by it.
    pub(crate) subagents: Vec<crate::agents::PluginSubagent>,
}

impl Views {
    /// Derive the projection from the registry under one read lock.
    ///
    /// Every active plugin's existing `<root>/skills` is published — there is
    /// no "discovery already found this dir" exemption. An unpublished plugin
    /// dir is one whose skills `guess_source` can only classify by directory
    /// name, i.e. a silent D-4 recurrence for any plugin whose dir name is not
    /// its id. Whether the `SkillSystem` scan also reached the dir by another
    /// route is the scan's concern (`SkillSystem::scan_roots` dedups against
    /// its base dirs), not the projection's.
    ///
    /// **The activation predicate lives here and nowhere else.** `is_active()`
    /// is true for [`PluginStatus::Loaded`](crate::extension::PluginStatus) and
    /// `Pending` (a pending plugin's non-dependent capabilities are already
    /// live), so only `Disabled` / `Blocked` / `Error` plugins contribute
    /// nothing — which is the whole point: an inactive plugin must be
    /// invisible to the model, not merely absent from the management list.
    async fn derive_plugin_projection(&self) -> PluginProjection {
        let registry = self.plugin_registry.read().await;

        // The id published with each dir is `record.id` — the registry id —
        // copied in the same loop as `record.scope_key`, so "whose dir" and
        // "which key" cannot come from two derivations (D-4: the directory
        // name is not the id).
        let mut plugin_skill_dirs: Vec<crate::utils::paths::PublishedPluginSkillDir> = Vec::new();
        for record in registry.list_active_plugins() {
            let dir = record.root_dir.join("skills");
            if dir.is_dir() && !plugin_skill_dirs.iter().any(|d| d.dir == dir) {
                plugin_skill_dirs.push(crate::utils::paths::PublishedPluginSkillDir {
                    dir,
                    plugin_id: record.id.clone(),
                    scope_key: record.scope_key.clone(),
                    plugin_root: record.root_dir.clone(),
                });
            }
        }

        // An agent row survives only while its owning plugin is active. The
        // registry keys agents by plugin id, so this is the same predicate as
        // above applied one level down — not a second rule. Each surviving
        // def is paired with its owning plugin's `scope_key` so
        // `visible_plugin_subagents` can filter it per-session.
        let subagents = registry
            .list_agents()
            .into_iter()
            .filter_map(|agent| {
                let plugin = registry.get_plugin(&agent.plugin_id)?;
                if !plugin.status.is_active() {
                    return None;
                }
                super::plugin_agent_to_def(agent).map(|def| crate::agents::PluginSubagent {
                    scope_key: plugin.scope_key.clone(),
                    def,
                })
            })
            .collect();

        PluginProjection {
            plugin_skill_dirs,
            subagents,
        }
    }

    /// Re-derive **all** plugin-owned process-global projections and install
    /// them, replacing whatever was published before.
    ///
    /// Publish is replace-semantics (not append), so this doubles as the
    /// retraction path: a plugin that stopped being active simply is not in the
    /// new vector. Called from exactly one place,
    /// `lifecycle.rs::after_transition` (G3 pins it); do not add a second
    /// caller — route the new path through `after_transition` instead.
    ///
    /// Returns the projection that was installed, for logging and tests.
    pub(crate) async fn republish_plugin_projections(&self) -> PluginProjection {
        // The BASE skill dirs only. Plugin dirs are deliberately not folded
        // in: `SkillSystem::init` merges (nothing ever leaves that set), so a
        // plugin dir pushed once would be rescanned after the plugin was
        // disabled, and its skills would stay in `<available_skills>` until
        // restart (D-5). The scan reads the plugin dirs from the published
        // list below instead — one source for both the index and the
        // `skill_read` search set.
        let skill_dirs: Vec<PathBuf> = self
            .discovery
            .discover_skill_dirs()
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.path)
            .collect();

        let projection = self.derive_plugin_projection().await;

        // The plugin roots are published as one process-global with two
        // readers: `get_all_skills_dirs` (the `skill_read` / `skill_list`
        // search set) and `SkillSystem::scan_roots` (the scan that feeds the
        // prompt index). Replace-semantics, so a plugin that is no longer
        // active is absent from both on the next read.
        // This publish must precede `skill_system.init` below: the rescan it
        // triggers reads the plugin dirs from the published list and
        // classifies each plugin skill's owner through `skill::guess_source`,
        // which reads the id published here — publish after and the scan
        // would neither see the dirs nor name their owner.
        crate::utils::paths::publish_plugin_skill_dirs(projection.plugin_skill_dirs.clone());
        crate::agents::publish_plugin_subagents(projection.subagents.clone());

        self.skill_system.init(skill_dirs).await;
        self.refresh_active_plugin_tools().await;

        projection
    }
}

#[cfg(test)]
mod tests {
    //! The census below is the guard that keeps this module a chokepoint.

    /// Source-level census: five needles are pinned each to exactly one
    /// owner file — `publish_plugin_skill_dirs(` and
    /// `publish_plugin_subagents(` may appear only in `projection.rs` (the
    /// two process-global publish calls); `republish_plugin_projections(`,
    /// `sync_hooks_from_registry(`, and `after_transition(` may appear only
    /// in `lifecycle.rs` (after P1.9 the view recomputation has exactly one
    /// trigger, `after_transition`: every public primitive ends with it, and
    /// the server-start watcher calls it when a readiness write changes a
    /// plugin's activity — spec §3.2). The pins are per FILE (`>= 1` hit in
    /// the owner, zero elsewhere); they do not count call sites.
    ///
    /// This has to be a source scan, not a runtime assertion: at runtime a
    /// second author looks exactly like the first one running twice. The
    /// failure mode it prevents is the one that shipped — `load_all` and
    /// `set_plugin_enabled` each deriving the set with a different predicate.
    ///
    /// Comment lines, `#[cfg(test)]` items, and every string/char literal's
    /// payload are stripped before matching (via
    /// `utils::source_scan::production_code_text`, shared with the G1
    /// census in `effects/census.rs`), because a doc comment or a test
    /// fixture that *names* a needle is documentation, not a call. Without
    /// that, this very `G3_PINNED` array — whose tuples are string literals
    /// containing the needle text itself — would self-match its own
    /// `lifecycle.rs`-owned entries as offenders: it lives inside this
    /// `mod tests`, which is itself `#[cfg(test)]`-attributed, so
    /// `production_code_text`'s item-blanking step alone (before it ever
    /// reaches literal payloads) already removes the whole block.
    ///
    /// (needle, the only file allowed to contain a non-definition line with
    /// it). A module-level const on purpose: P6 appends
    /// `("notify_tools_list_changed(", "lifecycle.rs")` when
    /// `after_transition` gains the MCP-face broadcast — one line here, no
    /// test-body edit.
    pub(super) const G3_PINNED: &[(&str, &str)] = &[
        ("publish_plugin_skill_dirs(", "projection.rs"),
        ("publish_plugin_subagents(", "projection.rs"),
        // The view recomputation has one trigger: lifecycle.rs::after_transition.
        ("republish_plugin_projections(", "lifecycle.rs"),
        ("sync_hooks_from_registry(", "lifecycle.rs"),
        ("after_transition(", "lifecycle.rs"),
        ("notify_tools_list_changed(", "lifecycle.rs"),
    ];

    #[test]
    fn publishing_plugin_projections_has_exactly_one_author() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/extension");
        let mut offenders: Vec<String> = Vec::new();
        let mut checked_files = 0usize;
        let mut found: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();

        for (rel, src) in crate::utils::source_scan::rust_sources_under(&root) {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&rel);
            let text = crate::utils::source_scan::production_code_text(&path, &src);
            checked_files += 1;
            let file_name = rel.rsplit('/').next().unwrap_or(&rel);
            for (lineno, line) in text.lines().enumerate() {
                let code = line.trim_start();
                for &(needle, owner) in G3_PINNED {
                    if !code.contains(needle) {
                        continue;
                    }
                    // A definition is not a call.
                    if code.contains(&format!("fn {}", needle.trim_end_matches('('))) {
                        continue;
                    }
                    if file_name == owner {
                        *found.entry(needle).or_default() += 1;
                    } else {
                        offenders.push(format!(
                            "{rel}:{} — `{}` (only {owner} may call this)",
                            lineno + 1,
                            code.trim()
                        ));
                    }
                }
            }
        }

        assert!(
            checked_files > 10,
            "census scanned only {checked_files} files — it is not looking where it thinks it is"
        );
        // Self-check: every pinned call must exist in its owner, or the
        // census is blind to a rename.
        for &(needle, owner) in G3_PINNED {
            assert!(
                found.get(needle).copied().unwrap_or(0) >= 1,
                "expected `{needle}` inside {owner}, found none — did it move? the census is now blind"
            );
        }
        assert!(
            offenders.is_empty(),
            "plugin projections are published only from projection.rs and re-derived only \
             from lifecycle.rs::after_transition (spec §3.2: one trigger after every \
             transition). Second author(s) found:\n  {}",
            offenders.join("\n  ")
        );
    }
}
